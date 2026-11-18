//! A stand-in for the API server, for the tests of everything that talks to
//! a cluster.
//!
//! It listens on a local port and speaks just enough of the Kubernetes API
//! for `kube` to list and watch against it: collections of objects that a
//! test fills and changes, with label and field selectors, and the logs of
//! containers, which a test writes line by line. It stores whatever a test
//! gives it and knows nothing about what the objects mean. It is a reading aid for the code under test, not a model of a
//! cluster; the real thing is checked by hand.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
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

pub fn events(namespace: &str) -> String {
    format!("/apis/events.k8s.io/v1/namespaces/{namespace}/events")
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
    /// The logs of containers, by namespace, pod and container.
    logs: HashMap<(String, String, String), ContainerLog>,
    /// How many of the next log requests are answered with a 500.
    failing_log_requests: usize,
}

/// The log of one container: its runs, the last one being the current one.
#[derive(Default)]
struct ContainerLog {
    runs: Vec<Vec<(DateTime<Utc>, String)>>,
    /// The current run has ended; its log is complete.
    ended: bool,
    /// The container waits to be started again, as in a crash loop. The
    /// kubelet then serves the run that ended as the previous one, and as
    /// the current one too.
    backing_off: bool,
    /// The open streams that follow the current run.
    followers: Vec<mpsc::UnboundedSender<Chunk>>,
}

/// A piece of a response that is sent as it comes.
enum Chunk {
    Line(String),
    /// Cut the connection without ending the response in good order.
    Break,
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
    lines: mpsc::UnboundedSender<Chunk>,
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

    /// A kubeconfig whose one context, `fake`, points at the stand-in, for
    /// tests that run a program against it.
    pub fn kubeconfig(&self) -> String {
        format!(
            "apiVersion: v1\n\
             kind: Config\n\
             current-context: fake\n\
             clusters:\n\
             - name: fake\n\
             \x20 cluster:\n\
             \x20   server: http://{}\n\
             contexts:\n\
             - name: fake\n\
             \x20 context:\n\
             \x20   cluster: fake\n\
             \x20   user: fake\n\
             users:\n\
             - name: fake\n\
             \x20 user:\n\
             \x20   token: not-a-real-token\n",
            self.address
        )
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

    /// Writes a line to the log of the current run of a container.
    pub fn log(
        &self,
        namespace: &str,
        pod: &str,
        container: &str,
        time: DateTime<Utc>,
        line: &str,
    ) {
        let mut shared = self.shared.lock().unwrap();
        let log = shared
            .logs
            .entry(log_key(namespace, pod, container))
            .or_default();
        if log.runs.is_empty() {
            log.runs.push(Vec::new());
        }
        log.runs
            .last_mut()
            .expect("there is a run")
            .push((time, line.to_string()));
        let sent = log_line(time, line);
        log.followers
            .retain(|follower| follower.send(Chunk::Line(sent.clone())).is_ok());
    }

    /// Ends the current run of a container: the streams that follow it end
    /// in good order.
    pub fn end_run(&self, namespace: &str, pod: &str, container: &str) {
        let mut shared = self.shared.lock().unwrap();
        let log = shared
            .logs
            .entry(log_key(namespace, pod, container))
            .or_default();
        log.ended = true;
        log.followers.clear();
    }

    /// Ends the current run of a container and leaves it waiting to be
    /// started again, as in a crash loop.
    pub fn back_off(&self, namespace: &str, pod: &str, container: &str) {
        self.end_run(namespace, pod, container);
        let mut shared = self.shared.lock().unwrap();
        let log = shared
            .logs
            .entry(log_key(namespace, pod, container))
            .or_default();
        log.backing_off = true;
    }

    /// Starts a new run of a container; the one before becomes the
    /// previous run.
    pub fn start_run(&self, namespace: &str, pod: &str, container: &str) {
        let mut shared = self.shared.lock().unwrap();
        let log = shared
            .logs
            .entry(log_key(namespace, pod, container))
            .or_default();
        log.runs.push(Vec::new());
        log.ended = false;
        log.backing_off = false;
        log.followers.clear();
    }

    /// Cuts every open log stream, as a dropped connection would.
    pub fn break_log_streams(&self) {
        let mut shared = self.shared.lock().unwrap();
        for log in shared.logs.values_mut() {
            for follower in log.followers.drain(..) {
                let _ = follower.send(Chunk::Break);
            }
        }
    }

    /// Answers the next `count` log requests with a 500.
    pub fn fail_log_requests(&self, count: usize) {
        self.shared.lock().unwrap().failing_log_requests = count;
    }
}

fn log_key(namespace: &str, pod: &str, container: &str) -> (String, String, String) {
    (
        namespace.to_string(),
        pod.to_string(),
        container.to_string(),
    )
}

/// A log line as the kubelet sends it with `timestamps`: the time, a space,
/// the line.
fn log_line(time: DateTime<Utc>, line: &str) -> String {
    format!(
        "{} {line}",
        time.to_rfc3339_opts(SecondsFormat::Nanos, true)
    )
}

impl Drop for FakeCluster {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn watch_line(kind: &str, object: &Value) -> Chunk {
    Chunk::Line(json!({ "type": kind, "object": object }).to_string())
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
    Lines(mpsc::UnboundedReceiver<Chunk>),
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
    if last == "log" {
        return route_log(&mut shared, &path, &query);
    }
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

/// The log of a container: `/api/v1/namespaces/{namespace}/pods/{pod}/log`.
fn route_log(shared: &mut Shared, path: &str, query: &HashMap<String, String>) -> Answer {
    if shared.failing_log_requests > 0 {
        shared.failing_log_requests -= 1;
        return status(500, "InternalError", "the kubelet does not answer");
    }
    let segments: Vec<&str> = path.split('/').collect();
    let [.., namespace, "pods", pod, "log"] = segments[..] else {
        return status(404, "NotFound", "no such path");
    };
    let container = query
        .get("container")
        .map(String::as_str)
        .unwrap_or_default();
    let flag = |name: &str| query.get(name).map(String::as_str) == Some("true");

    let waiting = || {
        status(
            400,
            "BadRequest",
            &format!(
                "container \"{container}\" in pod \"{pod}\" is waiting to start: ContainerCreating"
            ),
        )
    };
    let Some(log) = shared.logs.get_mut(&log_key(namespace, pod, container)) else {
        return waiting();
    };
    let run = if log.backing_off && !log.runs.is_empty() {
        log.runs.len() - 1
    } else if flag("previous") {
        if log.runs.len() < 2 {
            return status(
                400,
                "BadRequest",
                &format!(
                    "previous terminated container \"{container}\" in pod \"{pod}\" not found"
                ),
            );
        }
        log.runs.len() - 2
    } else if log.runs.is_empty() {
        return waiting();
    } else {
        log.runs.len() - 1
    };

    // The lines since a time, then the last so many of them.
    let since = query
        .get("sinceTime")
        .and_then(|time| DateTime::parse_from_rfc3339(time).ok())
        .map(|time| time.with_timezone(&Utc))
        .or_else(|| {
            let seconds: i64 = query.get("sinceSeconds")?.parse().ok()?;
            Some(Utc::now() - TimeDelta::seconds(seconds))
        });
    let mut lines: Vec<&(DateTime<Utc>, String)> = log.runs[run]
        .iter()
        .filter(|(time, _)| since.is_none_or(|since| *time >= since))
        .collect();
    if let Some(tail) = query
        .get("tailLines")
        .and_then(|tail| tail.parse::<usize>().ok())
    {
        let skip = lines.len().saturating_sub(tail);
        lines.drain(..skip);
    }

    let (sender, receiver) = mpsc::unbounded_channel();
    for (time, line) in lines {
        let _ = sender.send(Chunk::Line(log_line(*time, line)));
    }
    let is_current = run == log.runs.len() - 1;
    if flag("follow") && is_current && !log.ended {
        log.followers.push(sender);
    }
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
                            Some(Chunk::Line(line)) => {
                                let chunk = format!("{:x}\r\n{line}\n\r\n", line.len() + 1);
                                if stream.write_all(chunk.as_bytes()).await.is_err() {
                                    return;
                                }
                            }
                            Some(Chunk::Break) => return,
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

/// A cluster event of the older kind, as the kubelet writes it: about an
/// object given as kind and name, optionally about one of its containers,
/// with a count and the time it happened last.
#[allow(clippy::too_many_arguments)]
pub fn event_object(
    namespace: &str,
    name: &str,
    regarding: (&str, &str),
    container: Option<&str>,
    reason: &str,
    note: &str,
    kind: &str,
    count: i32,
    time: DateTime<Utc>,
) -> Value {
    let (regarding_kind, regarding_name) = regarding;
    let mut object = json!({
        "kind": regarding_kind, "namespace": namespace, "name": regarding_name,
        "uid": format!("uid-{regarding_name}")
    });
    if let Some(container) = container {
        object["fieldPath"] = json!(format!("spec.containers{{{container}}}"));
    }
    json!({
        "metadata": { "namespace": namespace, "name": name, "uid": format!("uid-event-{name}") },
        "regarding": object,
        "reason": reason,
        "note": note,
        "type": kind,
        "eventTime": null,
        "deprecatedCount": count,
        "deprecatedLastTimestamp": time.to_rfc3339_opts(SecondsFormat::Secs, true)
    })
}

/// The status of a container as a pod reports it: `state` and `last_state`
/// are objects such as `{ "running": {} }` or
/// `{ "waiting": { "reason": "CrashLoopBackOff" } }`.
pub fn container_status(
    name: &str,
    restarts: i32,
    state: Value,
    last_state: Option<Value>,
) -> Value {
    let mut status = json!({
        "name": name, "ready": false, "restartCount": restarts, "image": "", "imageID": "",
        "state": state
    });
    if let Some(last_state) = last_state {
        status["lastState"] = last_state;
    }
    status
}

/// A pod whose containers are in the states given, one status per container.
pub fn pod_with_statuses(namespace: &str, name: &str, statuses: Vec<Value>) -> Value {
    let specs: Vec<Value> = statuses
        .iter()
        .map(|status| json!({ "name": status["name"] }))
        .collect();
    json!({
        "metadata": metadata(namespace, name, &[], None),
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
