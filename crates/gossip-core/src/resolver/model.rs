//! Plain views of the API objects the resolver works with: only what target
//! resolution and the log streams need to know about a pod or a workload.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet};
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::{ContainerStatus, Pod};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use kube::core::Selector;

use crate::discovery::to_utc;
use crate::target::WorkloadKind;

/// The kinds of object the resolver watches besides pods: the five workload
/// kinds a target can name, and the replica sets between a deployment and
/// its pods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OwnerKind {
    Deployment,
    ReplicaSet,
    StatefulSet,
    DaemonSet,
    Job,
    CronJob,
}

impl OwnerKind {
    /// The kind as Kubernetes writes it.
    pub fn kind_name(self) -> &'static str {
        match self {
            OwnerKind::Deployment => "Deployment",
            OwnerKind::ReplicaSet => "ReplicaSet",
            OwnerKind::StatefulSet => "StatefulSet",
            OwnerKind::DaemonSet => "DaemonSet",
            OwnerKind::Job => "Job",
            OwnerKind::CronJob => "CronJob",
        }
    }

    /// The workload kind a target can name, if this is one.
    pub fn workload(self) -> Option<WorkloadKind> {
        match self {
            OwnerKind::Deployment => Some(WorkloadKind::Deployment),
            OwnerKind::ReplicaSet => None,
            OwnerKind::StatefulSet => Some(WorkloadKind::StatefulSet),
            OwnerKind::DaemonSet => Some(WorkloadKind::DaemonSet),
            OwnerKind::Job => Some(WorkloadKind::Job),
            OwnerKind::CronJob => Some(WorkloadKind::CronJob),
        }
    }
}

impl From<WorkloadKind> for OwnerKind {
    fn from(kind: WorkloadKind) -> Self {
        match kind {
            WorkloadKind::Deployment => OwnerKind::Deployment,
            WorkloadKind::StatefulSet => OwnerKind::StatefulSet,
            WorkloadKind::DaemonSet => OwnerKind::DaemonSet,
            WorkloadKind::Job => OwnerKind::Job,
            WorkloadKind::CronJob => OwnerKind::CronJob,
        }
    }
}

/// The controller that owns an object.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OwnerRef {
    pub kind: String,
    pub name: String,
    pub uid: String,
}

fn controller_of(meta: &ObjectMeta) -> Option<OwnerRef> {
    meta.owner_references
        .iter()
        .flatten()
        .find(|owner| owner.controller == Some(true))
        .map(|owner| OwnerRef {
            kind: owner.kind.clone(),
            name: owner.name.clone(),
            uid: owner.uid.clone(),
        })
}

/// A workload, or a replica set, as the resolver sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadView {
    pub kind: OwnerKind,
    pub name: String,
    pub uid: String,
    /// The controller above it: the deployment of a replica set, the cron
    /// job of a job.
    pub controller: Option<OwnerRef>,
    /// The labels its pods carry. A cron job has none of its own.
    pub selector: Option<Selector>,
}

impl WorkloadView {
    fn new(kind: OwnerKind, meta: &ObjectMeta, selector: Option<&LabelSelector>) -> Self {
        WorkloadView {
            kind,
            name: meta.name.clone().unwrap_or_default(),
            uid: meta.uid.clone().unwrap_or_default(),
            controller: controller_of(meta),
            // A selector that cannot be read selects nothing, rather than
            // everything.
            selector: selector.and_then(|selector| Selector::try_from(selector.clone()).ok()),
        }
    }
}

impl From<&Deployment> for WorkloadView {
    fn from(object: &Deployment) -> Self {
        WorkloadView::new(
            OwnerKind::Deployment,
            &object.metadata,
            Some(&object.spec.selector),
        )
    }
}

impl From<&ReplicaSet> for WorkloadView {
    fn from(object: &ReplicaSet) -> Self {
        WorkloadView::new(
            OwnerKind::ReplicaSet,
            &object.metadata,
            Some(&object.spec.selector),
        )
    }
}

impl From<&StatefulSet> for WorkloadView {
    fn from(object: &StatefulSet) -> Self {
        WorkloadView::new(
            OwnerKind::StatefulSet,
            &object.metadata,
            Some(&object.spec.selector),
        )
    }
}

impl From<&DaemonSet> for WorkloadView {
    fn from(object: &DaemonSet) -> Self {
        WorkloadView::new(
            OwnerKind::DaemonSet,
            &object.metadata,
            Some(&object.spec.selector),
        )
    }
}

impl From<&Job> for WorkloadView {
    fn from(object: &Job) -> Self {
        let selector = object.spec.as_ref().and_then(|spec| spec.selector.as_ref());
        WorkloadView::new(OwnerKind::Job, &object.metadata, selector)
    }
}

impl From<&CronJob> for WorkloadView {
    fn from(object: &CronJob) -> Self {
        WorkloadView::new(OwnerKind::CronJob, &object.metadata, None)
    }
}

/// A pod as the resolver sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct PodView {
    pub name: String,
    pub uid: String,
    pub labels: BTreeMap<String, String>,
    /// The controller that owns the pod: a replica set, a job, a stateful
    /// set, a daemon set, or nothing for a bare pod.
    pub controller: Option<OwnerRef>,
    /// Its containers: init containers first, then the containers, then
    /// ephemeral ones.
    pub containers: Vec<ContainerView>,
}

/// What kind of container of a pod this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerRole {
    Init,
    Main,
    Ephemeral,
}

/// A container of a pod with the state the pod reports for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerView {
    pub name: String,
    pub role: ContainerRole,
    /// How often the container has been restarted. It goes up by one with
    /// every new run of the container in this pod.
    pub restart_count: i32,
    pub state: ContainerState,
    /// Whether a run before the current one has ended and left its output.
    pub has_previous: bool,
}

/// The state of a container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerState {
    /// The pod reports nothing for the container yet.
    Unknown,
    /// Not running yet, or not running again yet: image pull, crash loop.
    Waiting {
        reason: Option<String>,
        message: Option<String>,
    },
    Running {
        started_at: Option<DateTime<Utc>>,
    },
    Terminated {
        exit_code: i32,
        reason: Option<String>,
        finished_at: Option<DateTime<Utc>>,
    },
}

impl ContainerState {
    /// Whether the container has output to read: it runs, or it ran.
    pub fn has_output(&self) -> bool {
        matches!(
            self,
            ContainerState::Running { .. } | ContainerState::Terminated { .. }
        )
    }

    pub fn is_running(&self) -> bool {
        matches!(self, ContainerState::Running { .. })
    }

    /// The reason of a waiting container, as in `CrashLoopBackOff`.
    pub fn waiting_reason(&self) -> Option<&str> {
        match self {
            ContainerState::Waiting { reason, .. } => reason.as_deref(),
            _ => None,
        }
    }
}

impl std::fmt::Display for ContainerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContainerState::Unknown => f.write_str("not reported yet"),
            ContainerState::Waiting { reason, .. } => match reason {
                Some(reason) => write!(f, "waiting: {reason}"),
                None => f.write_str("waiting"),
            },
            ContainerState::Running { .. } => f.write_str("running"),
            ContainerState::Terminated {
                exit_code, reason, ..
            } => match reason {
                Some(reason) => write!(f, "terminated: {reason}, exit code {exit_code}"),
                None => write!(f, "terminated: exit code {exit_code}"),
            },
        }
    }
}

impl From<&Pod> for PodView {
    fn from(pod: &Pod) -> Self {
        let spec = pod.spec.as_ref();
        let status = pod.status.as_ref();
        let statuses = |role: ContainerRole| -> &[ContainerStatus] {
            let list = status.and_then(|status| match role {
                ContainerRole::Init => status.init_container_statuses.as_deref(),
                ContainerRole::Main => status.container_statuses.as_deref(),
                ContainerRole::Ephemeral => status.ephemeral_container_statuses.as_deref(),
            });
            list.unwrap_or_default()
        };

        let mut containers = Vec::new();
        let mut add = |name: &str, role: ContainerRole| {
            let status = statuses(role).iter().find(|status| status.name == name);
            containers.push(container_view(name, role, status));
        };
        if let Some(spec) = spec {
            for container in spec.init_containers.iter().flatten() {
                add(&container.name, ContainerRole::Init);
            }
            for container in &spec.containers {
                add(&container.name, ContainerRole::Main);
            }
            for container in spec.ephemeral_containers.iter().flatten() {
                add(&container.name, ContainerRole::Ephemeral);
            }
        }

        PodView {
            name: pod.metadata.name.clone().unwrap_or_default(),
            uid: pod.metadata.uid.clone().unwrap_or_default(),
            labels: pod.metadata.labels.clone().unwrap_or_default(),
            controller: controller_of(&pod.metadata),
            containers,
        }
    }
}

fn container_view(
    name: &str,
    role: ContainerRole,
    status: Option<&ContainerStatus>,
) -> ContainerView {
    let state = match status.and_then(|status| status.state.as_ref()) {
        None => ContainerState::Unknown,
        Some(state) => {
            if let Some(running) = &state.running {
                ContainerState::Running {
                    started_at: running.started_at.as_ref().and_then(to_utc),
                }
            } else if let Some(terminated) = &state.terminated {
                ContainerState::Terminated {
                    exit_code: terminated.exit_code,
                    reason: terminated.reason.clone(),
                    finished_at: terminated.finished_at.as_ref().and_then(to_utc),
                }
            } else if let Some(waiting) = &state.waiting {
                ContainerState::Waiting {
                    reason: waiting.reason.clone(),
                    message: waiting.message.clone(),
                }
            } else {
                ContainerState::Unknown
            }
        }
    };
    ContainerView {
        name: name.to_string(),
        role,
        restart_count: status.map_or(0, |status| status.restart_count),
        state,
        has_previous: status
            .and_then(|status| status.last_state.as_ref())
            .is_some_and(|last| last.terminated.is_some()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use kube::core::SelectorExt;
    use serde_json::json;

    fn from_json<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> T {
        serde_json::from_value(value).expect("the object should deserialise")
    }

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn a_pod_with_its_owner_labels_and_containers() {
        let pod: Pod = from_json(json!({
            "metadata": {
                "name": "flowcpt-api-7d9f-xk2p1",
                "namespace": "flowcpt",
                "uid": "pod-1",
                "labels": { "app": "flowcpt-api", "pod-template-hash": "7d9f" },
                "ownerReferences": [ {
                    "apiVersion": "apps/v1", "kind": "ReplicaSet",
                    "name": "flowcpt-api-7d9f", "uid": "rs-1", "controller": true
                } ]
            },
            "spec": {
                "initContainers": [ { "name": "migrate" } ],
                "containers": [ { "name": "api" }, { "name": "envoy" } ],
                "ephemeralContainers": [ { "name": "debugger" } ]
            },
            "status": {
                "phase": "Running",
                "initContainerStatuses": [ {
                    "name": "migrate", "ready": true, "restartCount": 0, "image": "", "imageID": "",
                    "state": { "terminated": {
                        "exitCode": 0, "reason": "Completed", "finishedAt": "2026-11-18T13:00:03Z"
                    } }
                } ],
                "containerStatuses": [
                    {
                        "name": "api", "ready": false, "restartCount": 4, "image": "", "imageID": "",
                        "state": { "waiting": {
                            "reason": "CrashLoopBackOff",
                            "message": "back-off 40s restarting failed container"
                        } },
                        "lastState": { "terminated": { "exitCode": 101, "reason": "Error" } }
                    },
                    {
                        "name": "envoy", "ready": true, "restartCount": 0, "image": "", "imageID": "",
                        "state": { "running": { "startedAt": "2026-11-18T13:00:05Z" } }
                    }
                ]
            }
        }));
        let view = PodView::from(&pod);
        assert_eq!(view.name, "flowcpt-api-7d9f-xk2p1");
        assert_eq!(view.uid, "pod-1");
        assert_eq!(
            view.labels,
            labels(&[("app", "flowcpt-api"), ("pod-template-hash", "7d9f")])
        );
        assert_eq!(
            view.controller,
            Some(OwnerRef {
                kind: "ReplicaSet".into(),
                name: "flowcpt-api-7d9f".into(),
                uid: "rs-1".into(),
            })
        );
        assert_eq!(
            view.containers,
            [
                ContainerView {
                    name: "migrate".into(),
                    role: ContainerRole::Init,
                    restart_count: 0,
                    state: ContainerState::Terminated {
                        exit_code: 0,
                        reason: Some("Completed".into()),
                        finished_at: Some(Utc.with_ymd_and_hms(2026, 11, 18, 13, 0, 3).unwrap()),
                    },
                    has_previous: false,
                },
                ContainerView {
                    name: "api".into(),
                    role: ContainerRole::Main,
                    restart_count: 4,
                    state: ContainerState::Waiting {
                        reason: Some("CrashLoopBackOff".into()),
                        message: Some("back-off 40s restarting failed container".into()),
                    },
                    has_previous: true,
                },
                ContainerView {
                    name: "envoy".into(),
                    role: ContainerRole::Main,
                    restart_count: 0,
                    state: ContainerState::Running {
                        started_at: Some(Utc.with_ymd_and_hms(2026, 11, 18, 13, 0, 5).unwrap()),
                    },
                    has_previous: false,
                },
                // The pod reports nothing for the ephemeral container yet.
                ContainerView {
                    name: "debugger".into(),
                    role: ContainerRole::Ephemeral,
                    restart_count: 0,
                    state: ContainerState::Unknown,
                    has_previous: false,
                },
            ]
        );
    }

    #[test]
    fn a_bare_pod_has_no_controller() {
        let pod: Pod = from_json(json!({
            "metadata": {
                "name": "shell", "uid": "pod-2",
                "ownerReferences": [
                    { "apiVersion": "v1", "kind": "Something", "name": "x", "uid": "u" }
                ]
            },
            "spec": { "containers": [ { "name": "sh" } ] }
        }));
        let view = PodView::from(&pod);
        assert_eq!(view.controller, None);
        assert!(view.labels.is_empty());
        assert_eq!(view.containers.len(), 1);
        assert_eq!(view.containers[0].state, ContainerState::Unknown);
    }

    #[test]
    fn container_states_in_words() {
        let waiting = ContainerState::Waiting {
            reason: Some("ImagePullBackOff".into()),
            message: None,
        };
        assert_eq!(waiting.to_string(), "waiting: ImagePullBackOff");
        assert_eq!(waiting.waiting_reason(), Some("ImagePullBackOff"));
        assert!(!waiting.has_output());

        let running = ContainerState::Running { started_at: None };
        assert_eq!(running.to_string(), "running");
        assert!(running.has_output() && running.is_running());

        let terminated = ContainerState::Terminated {
            exit_code: 137,
            reason: Some("OOMKilled".into()),
            finished_at: None,
        };
        assert_eq!(
            terminated.to_string(),
            "terminated: OOMKilled, exit code 137"
        );
        assert!(terminated.has_output() && !terminated.is_running());
        assert!(!ContainerState::Unknown.has_output());
    }

    #[test]
    fn a_deployment_with_its_selector() {
        let deployment: Deployment = from_json(json!({
            "metadata": { "name": "flowcpt-api", "uid": "deploy-1" },
            "spec": {
                "selector": {
                    "matchLabels": { "app": "flowcpt-api" },
                    "matchExpressions": [
                        { "key": "tier", "operator": "In", "values": ["web", "api"] }
                    ]
                },
                "template": {}
            }
        }));
        let view = WorkloadView::from(&deployment);
        assert_eq!(view.kind, OwnerKind::Deployment);
        assert_eq!(view.name, "flowcpt-api");
        assert_eq!(view.uid, "deploy-1");
        assert_eq!(view.controller, None);
        let selector = view.selector.unwrap();
        assert!(selector.matches(&labels(&[("app", "flowcpt-api"), ("tier", "api")])));
        assert!(!selector.matches(&labels(&[("app", "flowcpt-api"), ("tier", "db")])));
        assert!(!selector.matches(&labels(&[("app", "other"), ("tier", "api")])));
    }

    #[test]
    fn a_replica_set_knows_its_deployment_and_a_job_its_cron_job() {
        let set: ReplicaSet = from_json(json!({
            "metadata": {
                "name": "flowcpt-api-7d9f", "uid": "rs-1",
                "ownerReferences": [ {
                    "apiVersion": "apps/v1", "kind": "Deployment",
                    "name": "flowcpt-api", "uid": "deploy-1", "controller": true
                } ]
            },
            "spec": { "selector": { "matchLabels": { "pod-template-hash": "7d9f" } } }
        }));
        let view = WorkloadView::from(&set);
        assert_eq!(view.kind, OwnerKind::ReplicaSet);
        assert_eq!(view.controller.unwrap().uid, "deploy-1");

        let job: Job = from_json(json!({
            "metadata": {
                "name": "nightly-sync-29391840", "uid": "job-1",
                "ownerReferences": [ {
                    "apiVersion": "batch/v1", "kind": "CronJob",
                    "name": "nightly-sync", "uid": "cron-1", "controller": true
                } ]
            },
            "spec": {
                "selector": { "matchLabels": { "batch.kubernetes.io/controller-uid": "job-1" } },
                "template": {}
            }
        }));
        let view = WorkloadView::from(&job);
        assert_eq!(view.kind, OwnerKind::Job);
        assert_eq!(view.controller.unwrap().name, "nightly-sync");
        assert!(view.selector.is_some());

        let cron: CronJob = from_json(json!({
            "metadata": { "name": "nightly-sync", "uid": "cron-1" },
            "spec": { "schedule": "0 2 * * *", "jobTemplate": {} }
        }));
        let view = WorkloadView::from(&cron);
        assert_eq!(view.kind, OwnerKind::CronJob);
        assert_eq!(view.selector, None);
    }

    #[test]
    fn owner_kinds_and_workload_kinds() {
        for kind in WorkloadKind::ALL {
            assert_eq!(OwnerKind::from(kind).workload(), Some(kind));
            assert_eq!(OwnerKind::from(kind).kind_name(), kind.kind_name());
        }
        assert_eq!(OwnerKind::ReplicaSet.workload(), None);
    }
}
