//! The stage the walkthrough of the README is played on: a stand-in API
//! server with one made-up cluster, a directory for the files the program
//! reads and writes, and a clock that the walkthrough moves.
//!
//! Every name is made up: the namespace `shop` with the deployments `api`
//! and `web`. No name of a real cluster is anywhere in here.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use chrono::{DateTime, SecondsFormat, TimeDelta, TimeZone, Utc};
use gossip_core::testing::{FakeCluster, deployments, events, namespaces, pods, replica_sets};
use serde_json::{Value, json};

use super::terminal::Terminal;

/// The namespace of the examples.
pub const NAMESPACE: &str = "shop";

/// Where the files of the walkthrough are. Always the same place, so that
/// a path the program prints is the same on every run and every machine.
const DIRECTORY: &str = "/tmp/gossip-readme";

/// The moment the walkthrough begins.
pub fn start() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 18, 14, 2, 0).unwrap()
}

/// A time `millis` after the walkthrough began.
pub fn at(millis: i64) -> DateTime<Utc> {
    start() + TimeDelta::milliseconds(millis)
}

/// The stand-in cluster and the files around it.
pub struct Stage {
    pub cluster: FakeCluster,
    binary: PathBuf,
    directory: PathBuf,
    /// Held for as long as the stage stands: two walkthroughs at once would
    /// write the same files.
    _lock: File,
}

impl Stage {
    /// Sets the stage for the program at this path.
    pub async fn set(binary: &Path) -> anyhow::Result<Stage> {
        let directory = PathBuf::from(DIRECTORY);
        std::fs::create_dir_all(&directory)
            .with_context(|| format!("{DIRECTORY} could not be made"))?;
        let lock = File::create(directory.join("lock"))?;
        lock.lock()
            .context("another walkthrough holds the directory")?;
        // Nothing of an earlier run is left.
        for entry in std::fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.file_name().is_some_and(|name| name == "lock") {
                continue;
            }
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        }
        let stage = Stage {
            cluster: FakeCluster::start().await,
            binary: binary.to_path_buf(),
            directory,
            _lock: lock,
        };
        std::fs::create_dir_all(stage.work())?;
        std::fs::create_dir_all(stage.config().parent().expect("a directory"))?;
        // The config file is named to the program, so it has to be there.
        std::fs::write(stage.config(), "")?;
        std::fs::write(stage.kubeconfig(), stage.kubeconfig_text())?;
        stage.set_clock(start())?;
        Ok(stage)
    }

    /// The directory the program is started in.
    pub fn work(&self) -> PathBuf {
        self.directory.join("work")
    }

    /// The config file, where a home directory has it on Linux.
    pub fn config(&self) -> PathBuf {
        self.directory.join(".config/gossip/gossip.toml")
    }

    fn kubeconfig(&self) -> PathBuf {
        self.directory.join("kubeconfig")
    }

    fn clock(&self) -> PathBuf {
        self.directory.join("clock")
    }

    /// A kubeconfig with two contexts that both point at the stand-in, the
    /// first of them the current one.
    fn kubeconfig_text(&self) -> String {
        // The server of the stand-in's own kubeconfig.
        let own = self.cluster.kubeconfig();
        let server = own
            .lines()
            .find_map(|line| line.trim().strip_prefix("server: "))
            .expect("the kubeconfig of the stand-in names its server");
        format!(
            "apiVersion: v1\n\
             kind: Config\n\
             current-context: demo\n\
             clusters:\n\
             - name: demo\n\
             \x20 cluster:\n\
             \x20   server: {server}\n\
             - name: demo-test\n\
             \x20 cluster:\n\
             \x20   server: {server}\n\
             contexts:\n\
             - name: demo\n\
             \x20 context:\n\
             \x20   cluster: demo\n\
             \x20   user: dev\n\
             - name: demo-test\n\
             \x20 context:\n\
             \x20   cluster: demo-test\n\
             \x20   user: dev\n\
             \x20   namespace: shop\n\
             users:\n\
             - name: dev\n\
             \x20 user:\n\
             \x20   token: not-a-real-token\n"
        )
    }

    /// Sets the clock the program reads. The file is replaced in one step,
    /// so the program never reads half a time.
    pub fn set_clock(&self, time: DateTime<Utc>) -> anyhow::Result<()> {
        let draft = self.directory.join("clock.next");
        std::fs::write(&draft, time.to_rfc3339_opts(SecondsFormat::Millis, true))?;
        std::fs::rename(&draft, self.clock())?;
        Ok(())
    }

    /// The program with these arguments, and everything around it pinned
    /// down: the cluster, the config file, the clock, the time zone and the
    /// locale.
    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .args(arguments)
            .current_dir(self.work())
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.directory)
            .env("KUBECONFIG", self.kubeconfig())
            .env(gossip_core::config::CONFIG_ENV, self.config())
            .env(gossip_core::clock::CLOCK_FILE, self.clock())
            // Times are printed in the local zone.
            .env("TZ", "UTC")
            // Emoji are for a terminal in a UTF-8 locale.
            .env("LANG", "C.UTF-8")
            .env("TERM", "xterm-256color");
        command
    }

    /// Starts the program on a terminal.
    pub fn start(&self, arguments: &[&str]) -> anyhow::Result<Terminal> {
        Terminal::start(self.command(arguments))
    }

    /// The command line of the program with these arguments, as a reader
    /// types it.
    pub fn typed(arguments: &[&str]) -> String {
        let mut line = String::from("$ gossip");
        for argument in arguments {
            line.push(' ');
            line.push_str(argument);
        }
        line
    }

    // ---- what the cluster holds -------------------------------------------

    /// The cluster as the walkthrough finds it: three namespaces, and in
    /// `shop` the deployments `api`, with two pods, and `web`, with one.
    pub fn fill(&self) {
        self.fill_namespaces();
        self.deployment("api", API_SET, 2, 41);
        self.deployment("web", WEB_SET, 1, 12);
        self.pod(API_POD_1, "api", API_SET, start() - TimeDelta::hours(52));
        self.pod(API_POD_2, "api", API_SET, start() - TimeDelta::hours(52));
        self.pod(WEB_POD, "web", WEB_SET, start() - TimeDelta::hours(29));
    }

    /// The namespaces of the cluster.
    pub fn fill_namespaces(&self) {
        for (name, days) in [("default", 96), ("kube-system", 96), (NAMESPACE, 41)] {
            self.cluster.apply(
                &namespaces(),
                json!({
                    "metadata": {
                        "name": name, "uid": format!("uid-{name}"),
                        "creationTimestamp": stamp(start() - TimeDelta::days(days)),
                    },
                    "status": { "phase": "Active" }
                }),
            );
        }
    }

    /// A deployment and its replica set, created so many days ago.
    pub fn deployment(&self, name: &str, set: &str, replicas: i32, days: i64) {
        let created = stamp(start() - TimeDelta::days(days));
        self.cluster.apply(
            &deployments(NAMESPACE),
            json!({
                "metadata": {
                    "namespace": NAMESPACE, "name": name, "uid": format!("uid-{name}"),
                    "labels": { "app": name },
                    "creationTimestamp": created,
                },
                "spec": {
                    "replicas": replicas,
                    "selector": { "matchLabels": { "app": name } },
                    "template": {}
                },
                "status": { "replicas": replicas, "readyReplicas": replicas }
            }),
        );
        self.cluster.apply(
            &replica_sets(NAMESPACE),
            json!({
                "metadata": {
                    "namespace": NAMESPACE, "name": set, "uid": format!("uid-{set}"),
                    "labels": { "app": name },
                    "creationTimestamp": created,
                    "ownerReferences": [owner("Deployment", name)],
                },
                "spec": { "selector": { "matchLabels": { "app": name } }, "template": {} }
            }),
        );
    }

    /// A running pod of a deployment, with one container named like the
    /// deployment.
    pub fn pod(&self, name: &str, app: &str, set: &str, created: DateTime<Utc>) {
        self.cluster.apply(
            &pods(NAMESPACE),
            pod_object(name, app, set, created, "Running", true),
        );
    }

    /// A pod that was scheduled and has not started its container yet.
    pub fn pending_pod(&self, name: &str, app: &str, set: &str, created: DateTime<Utc>) {
        self.cluster.apply(
            &pods(NAMESPACE),
            pod_object(name, app, set, created, "Pending", false),
        );
    }

    /// Takes a pod out of the cluster.
    pub fn delete_pod(&self, name: &str) {
        self.cluster.delete(&pods(NAMESPACE), name);
    }

    /// An event about a pod, as the kubelet and the scheduler write them.
    pub fn event(
        &self,
        name: &str,
        pod: &str,
        container: Option<&str>,
        reason: &str,
        note: &str,
        time: DateTime<Utc>,
    ) {
        let mut regarding = json!({
            "kind": "Pod", "namespace": NAMESPACE, "name": pod, "uid": format!("uid-{pod}")
        });
        if let Some(container) = container {
            regarding["fieldPath"] = json!(format!("spec.containers{{{container}}}"));
        }
        self.cluster.apply(
            &events(NAMESPACE),
            json!({
                "metadata": {
                    "namespace": NAMESPACE, "name": name, "uid": format!("uid-event-{name}")
                },
                "regarding": regarding,
                "reason": reason,
                "note": note,
                "type": "Normal",
                "eventTime": null,
                "deprecatedCount": 1,
                "deprecatedLastTimestamp": stamp(time),
            }),
        );
    }

    /// A line in the log of a pod's container.
    pub fn log(&self, pod: &str, container: &str, time: DateTime<Utc>, line: &str) {
        self.cluster.log(NAMESPACE, pod, container, time, line);
    }

    /// Ends the log of a pod's container: its process has stopped.
    pub fn end_log(&self, pod: &str, container: &str) {
        self.cluster.end_run(NAMESPACE, pod, container);
    }
}

/// The replica sets and the pods of the two deployments.
pub const API_SET: &str = "api-6c9f7b8d54";
pub const API_POD_1: &str = "api-6c9f7b8d54-x7k2p";
pub const API_POD_2: &str = "api-6c9f7b8d54-m4q8z";
/// The pod that takes the place of the first when it is killed.
pub const API_POD_3: &str = "api-6c9f7b8d54-r9t2v";
pub const WEB_SET: &str = "web-5d8c7f6b9";
pub const WEB_POD: &str = "web-5d8c7f6b9-tq6wn";

fn stamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn owner(kind: &str, name: &str) -> Value {
    json!({
        "apiVersion": "apps/v1", "kind": kind, "name": name,
        "uid": format!("uid-{name}"), "controller": true
    })
}

fn pod_object(
    name: &str,
    app: &str,
    set: &str,
    created: DateTime<Utc>,
    phase: &str,
    running: bool,
) -> Value {
    let state = if running {
        json!({ "running": { "startedAt": stamp(created + TimeDelta::seconds(2)) } })
    } else {
        json!({ "waiting": { "reason": "ContainerCreating" } })
    };
    json!({
        "metadata": {
            "namespace": NAMESPACE, "name": name, "uid": format!("uid-{name}"),
            "labels": { "app": app },
            "creationTimestamp": stamp(created),
            "ownerReferences": [owner("ReplicaSet", set)],
        },
        "spec": { "containers": [{ "name": app }] },
        "status": {
            "phase": phase,
            "containerStatuses": [{
                "name": app, "ready": running, "restartCount": 0, "image": "", "imageID": "",
                "state": state
            }]
        }
    })
}
