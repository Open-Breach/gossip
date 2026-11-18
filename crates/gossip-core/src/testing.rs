//! A stand-in for the API server, for the tests of everything that talks to
//! a cluster.
//!
//! It listens on a local port and speaks just enough of the Kubernetes API
//! for `kube` to list and watch against it: collections of objects that a
//! test fills and changes, with label and field selectors. It stores
//! whatever JSON a test gives it and knows nothing about what the objects
//! mean. It is a reading aid for the code under test, not a model of a
//! cluster; the real thing is checked by hand.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::cluster::Cluster;

/// The last path segment of every collection the stand-in serves.
const RESOURCES: [&str; 9] = [
    "namespaces",
    "pods",
    "deployments",
    "replicasets",
    "statefulsets",
    "daemonsets",
    "jobs",
    "cronjobs",
    "events",
];

pub fn namespaces() -> String {
    "/api/v1/namespaces".to_string()
}

pub fn pods(namespace: &str) -> String {
    format!("/api/v1/namespaces/{namespace}/pods")
}

pub fn deployments(namespace: &str) -> String {
    format!("/apis/apps/v1/namespaces/{namespace}/deployments")
}

pub fn replica_sets(namespace: &str) -> String {
    format!("/apis/apps/v1/namespaces/{namespace}/replicasets")
}

pub fn stateful_sets(namespace: &str) -> String {
    format!("/apis/apps/v1/namespaces/{namespace}/statefulsets")
}

pub fn jobs(namespace: &str) -> String {
    format!("/apis/batch/v1/namespaces/{namespace}/jobs")
}

pub fn cron_jobs(namespace: &str) -> String {
    format!("/apis/batch/v1/namespaces/{namespace}/cronjobs")
}

/// A stand-in API server on a local port. It stops when it is dropped.
pub struct FakeCluster {
    address: SocketAddr,
    shared: Arc<Mutex<Shared>>,
    server: JoinHandle<()>,
}

#[derive(Default)]
struct Shared {
    /// The resource version, counted up with every change.
    version: u64,
    collections: BTreeMap<String, Collection>,
    /// The collections that answer 403.
    forbidden: BTreeSet<String>,
    /// Every request so far, as `path?query`.
    requests: Vec<String>,
}

#[derive(Default)]
struct Collection {
    objects: BTreeMap<String, Value>,
    /// Every change so far, for a watch that starts at an older version.
    history: Vec<Change>,
    watchers: Vec<Watcher>,
}

#[derive(Clone)]
struct Change {
    version: u64,
    deleted: bool,
    object: Value,
}

struct Watcher {
    filter: Filter,
    lines: mpsc::UnboundedSender<String>,
}

impl FakeCluster {
    pub async fn start() -> FakeCluster {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a local port should be free");
        let address = listener.local_addr().expect("the listener has an address");
        let shared = Arc::new(Mutex::new(Shared::default()));
        let server = tokio::spawn({
            let shared = shared.clone();
            async move {
                while let Ok((stream, _)) = listener.accept().await {
                    tokio::spawn(serve(stream, shared.clone()));
                }
            }
        });
        FakeCluster {
            address,
            shared,
            server,
        }
    }

    /// A cluster handle whose client talks to the stand-in.
    pub fn cluster(&self) -> Cluster {
        let url = format!("http://{}", self.address)
            .parse()
            .expect("the address is a valid URL");
        let client = kube::Client::try_from(kube::Config::new(url))
            .expect("a client for a plain HTTP address can be built");
        Cluster::from_client("fake", client)
    }

    /// Creates or replaces an object of a collection. The object must have
    /// a `metadata.name`.
    pub fn apply(&self, collection: &str, mut object: Value) {
        let mut shared = self.shared.lock().unwrap();
        shared.version += 1;
        let version = shared.version;
        object["metadata"]["resourceVersion"] = json!(version.to_string());
        let name = object["metadata"]["name"]
            .as_str()
            .expect("the object needs a metadata.name")
            .to_string();

        let collection = shared
            .collections
            .entry(collection.to_string())
            .or_default();
        let before = collection.objects.insert(name, object.clone());
        collection.watchers.retain(|watcher| {
            let was_selected = before
                .as_ref()
                .is_some_and(|before| watcher.filter.selects(before));
            let kind = match (was_selected, watcher.filter.selects(&object)) {
                (false, true) => "ADDED",
                (true, true) => "MODIFIED",
                // It no longer carries the labels the watch asks for.
                (true, false) => "DELETED",
                (false, false) => return !watcher.lines.is_closed(),
            };
            watcher.lines.send(watch_line(kind, &object)).is_ok()
        });
        collection.history.push(Change {
            version,
            deleted: false,
            object,
        });
    }

    /// Deletes an object of a collection, if it is there.
    pub fn delete(&self, collection: &str, name: &str) {
        let mut shared = self.shared.lock().unwrap();
        shared.version += 1;
        let version = shared.version;
        let Some(collection) = shared.collections.get_mut(collection) else {
            return;
        };
        let Some(mut object) = collection.objects.remove(name) else {
            return;
        };
        object["metadata"]["resourceVersion"] = json!(version.to_string());
        collection.watchers.retain(|watcher| {
            if !watcher.filter.selects(&object) {
                return !watcher.lines.is_closed();
            }
            watcher.lines.send(watch_line("DELETED", &object)).is_ok()
        });
        collection.history.push(Change {
            version,
            deleted: true,
            object,
        });
    }

    /// Makes a collection answer 403 to every list and watch from now on.
    pub fn forbid(&self, collection: &str) {
        let mut shared = self.shared.lock().unwrap();
        shared.forbidden.insert(collection.to_string());
    }

    /// Every request so far, as `path?query` with the query decoded.
    pub fn requests(&self) -> Vec<String> {
        self.shared.lock().unwrap().requests.clone()
    }
}

impl Drop for FakeCluster {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn watch_line(kind: &str, object: &Value) -> String {
    json!({ "type": kind, "object": object }).to_string()
}

/// What a list or a watch asks for: a name, labels, or both.
#[derive(Debug, Default)]
struct Filter {
    name: Option<String>,
    labels: Vec<Requirement>,
}

#[derive(Debug, PartialEq)]
enum Requirement {
    Equal(String, String),
    NotEqual(String, String),
    In(String, Vec<String>),
    NotIn(String, Vec<String>),
    Exists(String),
    DoesNotExist(String),
}

impl Filter {
    fn from_query(query: &HashMap<String, String>) -> Filter {
        let name = query
            .get("fieldSelector")
            .and_then(|selector| selector.strip_prefix("metadata.name="))
            .map(str::to_string);
        let labels = query
            .get("labelSelector")
            .map(|selector| parse_label_selector(selector))
            .unwrap_or_default();
        Filter { name, labels }
    }

    fn selects(&self, object: &Value) -> bool {
        if let Some(name) = &self.name
            && object["metadata"]["name"].as_str() != Some(name)
        {
            return false;
        }
        let label = |key: &str| object["metadata"]["labels"][key].as_str();
        self.labels.iter().all(|requirement| match requirement {
            Requirement::Equal(key, value) => label(key) == Some(value),
            Requirement::NotEqual(key, value) => label(key) != Some(value),
            Requirement::In(key, values) => {
                label(key).is_some_and(|found| values.iter().any(|value| value == found))
            }
            Requirement::NotIn(key, values) => {
                !label(key).is_some_and(|found| values.iter().any(|value| value == found))
            }
            Requirement::Exists(key) => label(key).is_some(),
            Requirement::DoesNotExist(key) => label(key).is_none(),
        })
    }
}

fn parse_label_selector(selector: &str) -> Vec<Requirement> {
    // Split on the commas that are not inside the brackets of `in (a,b)`.
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0, 0);
    for (index, c) in selector.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&selector[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&selector[start..]);

    let set = |text: &str| -> Vec<String> {
        text.trim()
            .trim_start_matches('(')
            .trim_end_matches(')')
            .split(',')
            .map(|value| value.trim().to_string())
            .collect()
    };
    parts
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            if let Some((key, values)) = part.split_once(" notin ") {
                Requirement::NotIn(key.trim().to_string(), set(values))
            } else if let Some((key, values)) = part.split_once(" in ") {
                Requirement::In(key.trim().to_string(), set(values))
            } else if let Some((key, value)) = part.split_once("!=") {
                Requirement::NotEqual(key.trim().to_string(), value.trim().to_string())
            } else if let Some((key, value)) = part.split_once("==") {
                Requirement::Equal(key.trim().to_string(), value.trim().to_string())
            } else if let Some((key, value)) = part.split_once('=') {
                Requirement::Equal(key.trim().to_string(), value.trim().to_string())
            } else if let Some(key) = part.strip_prefix('!') {
                Requirement::DoesNotExist(key.trim().to_string())
            } else {
                Requirement::Exists(part.to_string())
            }
        })
        .collect()
}

/// What the stand-in answers to one request.
enum Answer {
    Json(u16, Value),
    /// Lines that are sent as they come, until the sender is dropped.
    Lines(mpsc::UnboundedReceiver<String>),
}

fn status(code: u16, reason: &str, message: &str) -> Answer {
    Answer::Json(
        code,
        json!({
            "kind": "Status",
            "apiVersion": "v1",
            "metadata": {},
            "status": "Failure",
            "message": message,
            "reason": reason,
            "code": code
        }),
    )
}

fn route(shared: &Mutex<Shared>, target: &str) -> Answer {
    let (path, query) = split_target(target);
    let mut shared = shared.lock().unwrap();
    shared.requests.push(match target.split_once('?') {
        Some(_) => {
            let mut pairs: Vec<String> = query
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect();
            pairs.sort();
            format!("{path}?{}", pairs.join("&"))
        }
        None => path.clone(),
    });

    if path == "/version" {
        return Answer::Json(
            200,
            json!({
                "major": "1", "minor": "37", "gitVersion": "v1.37.0-fake",
                "gitCommit": "", "gitTreeState": "", "buildDate": "",
                "goVersion": "", "compiler": "", "platform": "linux/amd64"
            }),
        );
    }

    let last = path.rsplit('/').next().unwrap_or_default();
    if !RESOURCES.contains(&last) {
        // One object: the path is its collection and its name.
        let (collection, name) = path.rsplit_once('/').unwrap_or_default();
        let found = shared
            .collections
            .get(collection)
            .and_then(|collection| collection.objects.get(name));
        return match found {
            Some(object) => Answer::Json(200, object.clone()),
            None => status(404, "NotFound", &format!("\"{name}\" not found")),
        };
    }

    if shared.forbidden.contains(&path) {
        return status(
            403,
            "Forbidden",
            &format!("{last} is forbidden: User \"fake\" cannot list resource \"{last}\""),
        );
    }

    let filter = Filter::from_query(&query);
    let version = shared.version;
    let collection = shared.collections.entry(path).or_default();
    let watching = matches!(query.get("watch").map(String::as_str), Some("true" | "1"));
    if !watching {
        let items: Vec<&Value> = collection
            .objects
            .values()
            .filter(|object| filter.selects(object))
            .collect();
        return Answer::Json(
            200,
            json!({
                "kind": "List",
                "apiVersion": "v1",
                "metadata": { "resourceVersion": version.to_string() },
                "items": items
            }),
        );
    }

    // A watch: first what changed since the version it starts at, then
    // every change as it happens.
    let since: u64 = query
        .get("resourceVersion")
        .and_then(|version| version.parse().ok())
        .unwrap_or(0);
    let (lines, receiver) = mpsc::unbounded_channel();
    for change in &collection.history {
        if change.version > since && filter.selects(&change.object) {
            let kind = if change.deleted {
                "DELETED"
            } else {
                "MODIFIED"
            };
            let _ = lines.send(watch_line(kind, &change.object));
        }
    }
    collection.watchers.push(Watcher { filter, lines });
    Answer::Lines(receiver)
}

/// Splits `path?query` and decodes the query.
fn split_target(target: &str) -> (String, HashMap<String, String>) {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let pairs = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect();
    (path.to_string(), pairs)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' => {
                let hex = text.get(index + 1..index + 3);
                match hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                    Some(byte) => {
                        decoded.push(byte);
                        index += 2;
                    }
                    None => decoded.push(b'%'),
                }
            }
            byte => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Serves the requests of one connection, one after the other.
async fn serve(mut stream: TcpStream, shared: Arc<Mutex<Shared>>) {
    let mut buffer = Vec::new();
    loop {
        // Read one request head. The requests of interest have no body.
        let head_end = loop {
            if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                break position + 4;
            }
            let mut chunk = [0u8; 4096];
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(read) => buffer.extend_from_slice(&chunk[..read]),
            }
        };
        let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
        buffer.drain(..head_end);
        let target = head.split_whitespace().nth(1).unwrap_or("/").to_string();

        match route(&shared, &target) {
            Answer::Json(code, body) => {
                let body = body.to_string();
                let response = format!(
                    "HTTP/1.1 {code} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    if code == 200 { "OK" } else { "Error" },
                    body.len()
                );
                if stream.write_all(response.as_bytes()).await.is_err() {
                    return;
                }
            }
            Answer::Lines(mut lines) => {
                let head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n";
                if stream.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                let mut scratch = [0u8; 64];
                loop {
                    tokio::select! {
                        line = lines.recv() => match line {
                            Some(line) => {
                                let chunk = format!("{:x}\r\n{line}\n\r\n", line.len() + 1);
                                if stream.write_all(chunk.as_bytes()).await.is_err() {
                                    return;
                                }
                            }
                            None => {
                                let _ = stream.write_all(b"0\r\n\r\n").await;
                                return;
                            }
                        },
                        // The client hung up.
                        read = stream.read(&mut scratch) => {
                            if matches!(read, Ok(0) | Err(_)) {
                                return;
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---- objects for tests ---------------------------------------------------

/// A controller owner reference, as `metadata.ownerReferences` holds it.
pub fn owner(kind: &str, name: &str) -> Value {
    json!({
        "apiVersion": "v1", "kind": kind, "name": name,
        "uid": format!("uid-{name}"), "controller": true
    })
}

fn metadata(
    namespace: &str,
    name: &str,
    labels: &[(&str, &str)],
    controller: Option<Value>,
) -> Value {
    let labels: BTreeMap<&str, &str> = labels.iter().copied().collect();
    let mut metadata = json!({
        "namespace": namespace, "name": name, "uid": format!("uid-{name}"), "labels": labels
    });
    if let Some(controller) = controller {
        metadata["ownerReferences"] = json!([controller]);
    }
    metadata
}

/// A namespace object.
pub fn namespace_object(name: &str) -> Value {
    json!({ "metadata": { "name": name, "uid": format!("uid-{name}") }, "status": { "phase": "Active" } })
}

/// A running pod with these containers.
pub fn pod_object(
    namespace: &str,
    name: &str,
    labels: &[(&str, &str)],
    controller: Option<Value>,
    containers: &[&str],
) -> Value {
    let specs: Vec<Value> = containers
        .iter()
        .map(|name| json!({ "name": name }))
        .collect();
    let statuses: Vec<Value> = containers
        .iter()
        .map(|name| {
            json!({
                "name": name, "ready": true, "restartCount": 0, "image": "", "imageID": "",
                "state": { "running": { "startedAt": "2026-11-18T13:00:00Z" } }
            })
        })
        .collect();
    json!({
        "metadata": metadata(namespace, name, labels, controller),
        "spec": { "containers": specs },
        "status": { "phase": "Running", "containerStatuses": statuses }
    })
}

/// A workload with a `matchLabels` selector: a deployment, a replica set, a
/// stateful set, a daemon set or a job.
pub fn workload_object(
    namespace: &str,
    name: &str,
    selector: &[(&str, &str)],
    controller: Option<Value>,
) -> Value {
    let match_labels: BTreeMap<&str, &str> = selector.iter().copied().collect();
    json!({
        "metadata": metadata(namespace, name, &[], controller),
        "spec": { "selector": { "matchLabels": match_labels }, "template": {} }
    })
}

/// A cron job, which has no selector of its own.
pub fn cron_job_object(namespace: &str, name: &str) -> Value {
    json!({
        "metadata": metadata(namespace, name, &[], None),
        "spec": { "schedule": "0 2 * * *", "jobTemplate": {} }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_selectors_are_read_as_kube_writes_them() {
        assert_eq!(
            parse_label_selector("app=flowcpt-api,tier in (web,api),!canary,stage!=dev,owner"),
            [
                Requirement::Equal("app".into(), "flowcpt-api".into()),
                Requirement::In("tier".into(), vec!["web".into(), "api".into()]),
                Requirement::DoesNotExist("canary".into()),
                Requirement::NotEqual("stage".into(), "dev".into()),
                Requirement::Exists("owner".into()),
            ]
        );
        assert_eq!(
            parse_label_selector("tier notin (db)"),
            [Requirement::NotIn("tier".into(), vec!["db".into()])]
        );
        assert_eq!(parse_label_selector(""), []);
    }

    #[test]
    fn a_filter_selects_by_name_and_labels() {
        let pod = pod_object(
            "flowcpt",
            "a",
            &[("app", "api"), ("tier", "web")],
            None,
            &["api"],
        );
        let filter = |labels: &str, name: Option<&str>| Filter {
            name: name.map(str::to_string),
            labels: parse_label_selector(labels),
        };
        assert!(filter("", None).selects(&pod));
        assert!(filter("app=api,tier in (web,api)", None).selects(&pod));
        assert!(!filter("app=other", None).selects(&pod));
        assert!(!filter("!tier", None).selects(&pod));
        assert!(filter("", Some("a")).selects(&pod));
        assert!(!filter("", Some("b")).selects(&pod));
    }

    #[test]
    fn a_query_is_decoded() {
        let (path, query) = split_target(
            "/api/v1/namespaces/flowcpt/pods?&watch=true&labelSelector=tier+in+%28web%2Capi%29",
        );
        assert_eq!(path, "/api/v1/namespaces/flowcpt/pods");
        assert_eq!(query["watch"], "true");
        assert_eq!(query["labelSelector"], "tier in (web,api)");
    }
}
