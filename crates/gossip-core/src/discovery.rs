//! Discovery: what is in the cluster, so that names need not be typed from
//! memory. Nothing is cached.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use k8s_openapi::api::apps::v1::{Deployment, ReplicaSet};
use k8s_openapi::api::core::v1::{ContainerState, ContainerStatus, Namespace, Pod};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use kube::api::{Api, ListParams};
use kube::core::Selector;
use kube::{Resource, ResourceExt};
use serde::Serialize;

use crate::cluster::Cluster;
use crate::error::Error;

/// A namespace of the cluster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NamespaceInfo {
    pub name: String,
    /// `Active` or `Terminating`.
    pub status: String,
    pub created: Option<DateTime<Utc>>,
}

/// A deployment with its replicas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeploymentInfo {
    pub name: String,
    pub namespace: String,
    /// The replicas that are ready.
    pub ready: i32,
    /// The replicas the deployment asks for.
    pub desired: i32,
    pub created: Option<DateTime<Utc>>,
}

/// A pod with a little status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PodInfo {
    pub name: String,
    pub namespace: String,
    /// `Pending`, `Running`, `Succeeded`, `Failed` or `Unknown`.
    pub phase: String,
    /// The restarts of its containers, added up.
    pub restarts: i32,
    pub created: Option<DateTime<Utc>>,
    /// Its containers, init containers first.
    pub containers: Vec<ContainerInfo>,
}

/// A container of a pod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContainerInfo {
    pub name: String,
    /// Whether this is an init container.
    pub init: bool,
    pub ready: bool,
    pub restarts: i32,
    /// `running`, `waiting` or `terminated`, with the reason when the
    /// cluster gives one, as in `waiting: CrashLoopBackOff`; `unknown` when
    /// the pod reports no status for the container yet.
    pub state: String,
}

impl Cluster {
    /// The namespaces of the cluster.
    pub async fn namespaces(&self) -> Result<Vec<NamespaceInfo>, Error> {
        let api: Api<Namespace> = Api::all(self.client());
        let list = api
            .list(&ListParams::default())
            .await
            .map_err(|error| Error::api("list the namespaces", &error))?;
        Ok(list.items.iter().map(namespace_info).collect())
    }

    /// The deployments of a namespace.
    pub async fn deployments(&self, namespace: &str) -> Result<Vec<DeploymentInfo>, Error> {
        let api: Api<Deployment> = Api::namespaced(self.client(), namespace);
        let list = api.list(&ListParams::default()).await.map_err(|error| {
            Error::api(
                format!("list the deployments of the namespace `{namespace}`"),
                &error,
            )
        })?;
        Ok(list.items.iter().map(deployment_info).collect())
    }

    /// The pods of a namespace, or of one deployment in it. A deployment is
    /// resolved through its replica sets to the pods they own.
    pub async fn pods(
        &self,
        namespace: &str,
        deployment: Option<&str>,
    ) -> Result<Vec<PodInfo>, Error> {
        let pods: Api<Pod> = Api::namespaced(self.client(), namespace);
        let list_error = |error: &kube::Error| {
            Error::api(
                format!("list the pods of the namespace `{namespace}`"),
                error,
            )
        };

        let Some(name) = deployment else {
            let list = pods
                .list(&ListParams::default())
                .await
                .map_err(|error| list_error(&error))?;
            return Ok(list.items.iter().map(pod_info).collect());
        };

        let deployments: Api<Deployment> = Api::namespaced(self.client(), namespace);
        let deployment = deployments
            .get_opt(name)
            .await
            .map_err(|error| {
                Error::api(
                    format!("read the deployment `{name}` of the namespace `{namespace}`"),
                    &error,
                )
            })?
            .ok_or_else(|| Error::NotFound {
                kind: "deployment",
                namespace: namespace.to_string(),
                name: name.to_string(),
            })?;
        let selector = Selector::try_from(deployment.spec.selector.clone()).map_err(|error| {
            Error::Selector {
                kind: "deployment",
                name: name.to_string(),
                reason: error.to_string(),
            }
        })?;
        let by_labels = ListParams::default().labels_from(&selector);

        let replica_sets: Api<ReplicaSet> = Api::namespaced(self.client(), namespace);
        let owned_sets: BTreeSet<String> = replica_sets
            .list(&by_labels)
            .await
            .map_err(|error| {
                Error::api(
                    format!("list the replica sets of the deployment `{name}`"),
                    &error,
                )
            })?
            .items
            .iter()
            .filter(|set| is_controlled_by(*set, &deployment))
            .filter_map(|set| set.uid())
            .collect();

        let list = pods
            .list(&by_labels)
            .await
            .map_err(|error| list_error(&error))?;
        Ok(list
            .items
            .iter()
            .filter(|pod| controller_uid(*pod).is_some_and(|uid| owned_sets.contains(uid)))
            .map(pod_info)
            .collect())
    }
}

/// The uid of the controller that owns an object, if it has one.
fn controller_uid<K: Resource>(object: &K) -> Option<&str> {
    object
        .meta()
        .owner_references
        .iter()
        .flatten()
        .find(|owner| owner.controller == Some(true))
        .map(|owner| owner.uid.as_str())
}

fn is_controlled_by<K: Resource, O: Resource>(object: &K, owner: &O) -> bool {
    match (controller_uid(object), owner.meta().uid.as_deref()) {
        (Some(controller), Some(owner)) => controller == owner,
        _ => false,
    }
}

/// A time of the API as the record's kind of time.
pub(crate) fn to_utc(time: &Time) -> Option<DateTime<Utc>> {
    stamp_to_utc(&time.0)
}

pub(crate) fn stamp_to_utc(stamp: &k8s_openapi::jiff::Timestamp) -> Option<DateTime<Utc>> {
    let nanos = u32::try_from(stamp.subsec_nanosecond()).unwrap_or(0);
    DateTime::from_timestamp(stamp.as_second(), nanos)
}

fn created(object: &impl ResourceExt) -> Option<DateTime<Utc>> {
    object.creation_timestamp().as_ref().and_then(to_utc)
}

fn namespace_info(namespace: &Namespace) -> NamespaceInfo {
    NamespaceInfo {
        name: namespace.name_any(),
        status: namespace
            .status
            .as_ref()
            .and_then(|status| status.phase.clone())
            .unwrap_or_else(|| "Unknown".to_string()),
        created: created(namespace),
    }
}

fn deployment_info(deployment: &Deployment) -> DeploymentInfo {
    DeploymentInfo {
        name: deployment.name_any(),
        namespace: deployment.namespace().unwrap_or_default(),
        ready: deployment
            .status
            .as_ref()
            .and_then(|status| status.ready_replicas)
            .unwrap_or(0),
        // A deployment that does not say how many replicas it wants gets one.
        desired: deployment.spec.replicas.unwrap_or(1),
        created: created(deployment),
    }
}

fn pod_info(pod: &Pod) -> PodInfo {
    let spec = pod.spec.as_ref();
    let status = pod.status.as_ref();
    let init_statuses = status
        .and_then(|status| status.init_container_statuses.as_deref())
        .unwrap_or_default();
    let statuses = status
        .and_then(|status| status.container_statuses.as_deref())
        .unwrap_or_default();

    let container = |name: &str, init: bool, statuses: &[ContainerStatus]| {
        let status = statuses.iter().find(|status| status.name == name);
        ContainerInfo {
            name: name.to_string(),
            init,
            ready: status.is_some_and(|status| status.ready),
            restarts: status.map_or(0, |status| status.restart_count),
            state: status
                .and_then(|status| status.state.as_ref())
                .map_or_else(|| "unknown".to_string(), state_text),
        }
    };
    let mut containers = Vec::new();
    for init in spec
        .and_then(|spec| spec.init_containers.as_deref())
        .unwrap_or_default()
    {
        containers.push(container(&init.name, true, init_statuses));
    }
    for main in spec
        .map(|spec| spec.containers.as_slice())
        .unwrap_or_default()
    {
        containers.push(container(&main.name, false, statuses));
    }

    PodInfo {
        name: pod.name_any(),
        namespace: pod.namespace().unwrap_or_default(),
        phase: status
            .and_then(|status| status.phase.clone())
            .unwrap_or_else(|| "Unknown".to_string()),
        restarts: statuses.iter().map(|status| status.restart_count).sum(),
        created: created(pod),
        containers,
    }
}

fn state_text(state: &ContainerState) -> String {
    let with_reason = |word: &str, reason: Option<&String>| match reason {
        Some(reason) if !reason.is_empty() => format!("{word}: {reason}"),
        _ => word.to_string(),
    };
    if state.running.is_some() {
        "running".to_string()
    } else if let Some(waiting) = &state.waiting {
        with_reason("waiting", waiting.reason.as_ref())
    } else if let Some(terminated) = &state.terminated {
        with_reason("terminated", terminated.reason.as_ref())
    } else {
        "unknown".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn from_json<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> T {
        serde_json::from_value(value).expect("the object should deserialise")
    }

    #[test]
    fn a_namespace() {
        let namespace: Namespace = from_json(json!({
            "metadata": { "name": "flowcpt", "creationTimestamp": "2026-11-06T09:30:00Z" },
            "status": { "phase": "Active" }
        }));
        assert_eq!(
            namespace_info(&namespace),
            NamespaceInfo {
                name: "flowcpt".into(),
                status: "Active".into(),
                created: Some(Utc.with_ymd_and_hms(2026, 11, 6, 9, 30, 0).unwrap()),
            }
        );
    }

    #[test]
    fn a_deployment_with_its_ready_and_desired_replicas() {
        let deployment: Deployment = from_json(json!({
            "metadata": {
                "name": "flowcpt-api",
                "namespace": "flowcpt",
                "creationTimestamp": "2026-11-13T08:00:00Z"
            },
            "spec": {
                "replicas": 3,
                "selector": { "matchLabels": { "app": "flowcpt-api" } },
                "template": {}
            },
            "status": { "replicas": 3, "readyReplicas": 2 }
        }));
        let info = deployment_info(&deployment);
        assert_eq!(info.name, "flowcpt-api");
        assert_eq!(info.namespace, "flowcpt");
        assert_eq!((info.ready, info.desired), (2, 3));
        assert_eq!(
            info.created,
            Some(Utc.with_ymd_and_hms(2026, 11, 13, 8, 0, 0).unwrap())
        );
    }

    #[test]
    fn a_deployment_scaled_to_zero_has_no_ready_replicas() {
        let deployment: Deployment = from_json(json!({
            "metadata": { "name": "idle", "namespace": "flowcpt" },
            "spec": {
                "replicas": 0,
                "selector": { "matchLabels": { "app": "idle" } },
                "template": {}
            },
            "status": {}
        }));
        let info = deployment_info(&deployment);
        assert_eq!((info.ready, info.desired), (0, 0));
        assert_eq!(info.created, None);
    }

    fn pod() -> Pod {
        from_json(json!({
            "metadata": {
                "name": "flowcpt-api-7d9f-xk2p1",
                "namespace": "flowcpt",
                "uid": "pod-1",
                "creationTimestamp": "2026-11-18T13:00:00Z",
                "ownerReferences": [
                    { "apiVersion": "v1", "kind": "Something", "name": "x", "uid": "other" },
                    {
                        "apiVersion": "apps/v1", "kind": "ReplicaSet",
                        "name": "flowcpt-api-7d9f", "uid": "rs-1", "controller": true
                    }
                ]
            },
            "spec": {
                "initContainers": [ { "name": "migrate" } ],
                "containers": [ { "name": "api" }, { "name": "envoy" } ]
            },
            "status": {
                "phase": "Running",
                "initContainerStatuses": [ {
                    "name": "migrate", "ready": true, "restartCount": 0,
                    "image": "", "imageID": "",
                    "state": { "terminated": { "exitCode": 0, "reason": "Completed" } }
                } ],
                "containerStatuses": [
                    {
                        "name": "envoy", "ready": true, "restartCount": 1,
                        "image": "", "imageID": "",
                        "state": { "running": { "startedAt": "2026-11-18T13:00:05Z" } }
                    },
                    {
                        "name": "api", "ready": false, "restartCount": 4,
                        "image": "", "imageID": "",
                        "state": { "waiting": { "reason": "CrashLoopBackOff" } }
                    }
                ]
            }
        }))
    }

    #[test]
    fn a_pod_with_phase_restarts_and_containers() {
        let info = pod_info(&pod());
        assert_eq!(info.name, "flowcpt-api-7d9f-xk2p1");
        assert_eq!(info.namespace, "flowcpt");
        assert_eq!(info.phase, "Running");
        // The restarts of the containers add up; init containers do not count.
        assert_eq!(info.restarts, 5);
        assert_eq!(
            info.created,
            Some(Utc.with_ymd_and_hms(2026, 11, 18, 13, 0, 0).unwrap())
        );
        // Init containers first, then the containers in the order of the spec.
        assert_eq!(
            info.containers,
            [
                ContainerInfo {
                    name: "migrate".into(),
                    init: true,
                    ready: true,
                    restarts: 0,
                    state: "terminated: Completed".into(),
                },
                ContainerInfo {
                    name: "api".into(),
                    init: false,
                    ready: false,
                    restarts: 4,
                    state: "waiting: CrashLoopBackOff".into(),
                },
                ContainerInfo {
                    name: "envoy".into(),
                    init: false,
                    ready: true,
                    restarts: 1,
                    state: "running".into(),
                },
            ]
        );
    }

    #[test]
    fn a_pod_that_reports_no_status_yet() {
        let pod: Pod = from_json(json!({
            "metadata": { "name": "fresh", "namespace": "flowcpt" },
            "spec": { "containers": [ { "name": "api" } ] }
        }));
        let info = pod_info(&pod);
        assert_eq!(info.phase, "Unknown");
        assert_eq!(info.restarts, 0);
        assert_eq!(info.containers.len(), 1);
        assert_eq!(info.containers[0].state, "unknown");
        assert!(!info.containers[0].ready);
    }

    #[test]
    fn the_controller_among_the_owners() {
        let pod = pod();
        assert_eq!(controller_uid(&pod), Some("rs-1"));

        let set: ReplicaSet = from_json(json!({
            "metadata": {
                "name": "flowcpt-api-7d9f", "uid": "rs-1",
                "ownerReferences": [ {
                    "apiVersion": "apps/v1", "kind": "Deployment",
                    "name": "flowcpt-api", "uid": "deploy-1", "controller": true
                } ]
            }
        }));
        let deployment: Deployment = from_json(json!({
            "metadata": { "name": "flowcpt-api", "uid": "deploy-1" }
        }));
        let other: Deployment = from_json(json!({
            "metadata": { "name": "flowcpt-web-api", "uid": "deploy-2" }
        }));
        assert!(is_controlled_by(&set, &deployment));
        assert!(!is_controlled_by(&set, &other));
        assert!(is_controlled_by(&pod, &set));
        assert!(!is_controlled_by(&deployment, &set));
    }
}
