//! `gossip list`: the contexts of the kubeconfig, and the namespaces,
//! deployments and pods of a cluster.

use std::io::Write;
use std::str::FromStr;

use anyhow::{Context, bail};
use chrono::{DateTime, Utc};
use gossip_core::{Cluster, ContextInfo, DeploymentInfo, NamespaceInfo, PodInfo};
use serde::Serialize;

use crate::age::age_since;
use crate::cli::ListArgs;
use crate::table::Table;

/// What `gossip list` is asked to show.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Subject {
    Contexts,
    Namespaces,
    Deployments {
        namespace: String,
    },
    Pods {
        namespace: String,
        deployment: String,
    },
}

impl FromStr for Subject {
    type Err = anyhow::Error;

    fn from_str(what: &str) -> Result<Self, Self::Err> {
        const FORMS: &str =
            "list takes `contexts`, `namespaces`, a namespace, or `namespace/deployment`";
        if what.contains(['*', '?', '[', '{']) {
            bail!("`{what}` holds a glob, and list takes exact names");
        }
        let levels: Vec<&str> = what.split('/').collect();
        if levels.iter().any(|level| level.trim().is_empty()) {
            bail!("`{what}` has an empty name; {FORMS}");
        }
        match levels[..] {
            ["contexts"] => Ok(Subject::Contexts),
            ["namespaces"] => Ok(Subject::Namespaces),
            [namespace] => Ok(Subject::Deployments {
                namespace: namespace.to_string(),
            }),
            [namespace, deployment] => Ok(Subject::Pods {
                namespace: namespace.to_string(),
                deployment: deployment.to_string(),
            }),
            _ => bail!("`{what}` has too many levels; {FORMS}"),
        }
    }
}

pub async fn run(args: &ListArgs, out: &mut impl Write) -> anyhow::Result<()> {
    let subject: Subject = args.what.parse()?;
    let now = Utc::now();

    if subject == Subject::Contexts {
        let contexts = Cluster::contexts()?;
        return print(out, args.json, &contexts, contexts_table(&contexts), || {
            "The kubeconfig holds no contexts.".to_string()
        });
    }

    let cluster = Cluster::open(args.context.as_deref()).await?;
    match subject {
        Subject::Contexts => unreachable!("handled above"),
        Subject::Namespaces => {
            let namespaces = cluster.namespaces().await?;
            print(
                out,
                args.json,
                &namespaces,
                namespaces_table(&namespaces, now),
                || format!("The cluster `{}` shows no namespaces.", cluster.name()),
            )
        }
        Subject::Deployments { namespace } => {
            let deployments = cluster.deployments(&namespace).await?;
            let table = deployments_table(&deployments, now);
            print(out, args.json, &deployments, table, || {
                format!("No deployments found in the namespace `{namespace}`.")
            })
        }
        Subject::Pods {
            namespace,
            deployment,
        } => {
            let pods = cluster.pods(&namespace, Some(&deployment)).await?;
            print(out, args.json, &pods, pods_table(&pods, now), || {
                format!("The deployment `{deployment}` in the namespace `{namespace}` has no pods.")
            })
        }
    }
}

/// Prints the items as JSON or as a table. An empty table is replaced by a
/// sentence on stderr, so that stdout stays empty for scripts.
fn print<T: Serialize>(
    out: &mut impl Write,
    json: bool,
    items: &[T],
    table: Table,
    nothing: impl FnOnce() -> String,
) -> anyhow::Result<()> {
    if json {
        let text = serde_json::to_string_pretty(items).context("cannot write the list as JSON")?;
        writeln!(out, "{text}")?;
    } else if table.is_empty() {
        eprintln!("{}", nothing());
    } else {
        write!(out, "{}", table.render())?;
    }
    Ok(())
}

fn contexts_table(contexts: &[ContextInfo]) -> Table {
    let mut table = Table::new(&["CURRENT", "NAME", "CLUSTER", "AUTHINFO", "NAMESPACE"]);
    for context in contexts {
        table.row(vec![
            if context.current { "*" } else { "" }.to_string(),
            context.name.clone(),
            context.cluster.clone().unwrap_or_default(),
            context.user.clone().unwrap_or_default(),
            context.namespace.clone().unwrap_or_default(),
        ]);
    }
    table
}

fn namespaces_table(namespaces: &[NamespaceInfo], now: DateTime<Utc>) -> Table {
    let mut table = Table::new(&["NAME", "STATUS", "AGE"]);
    for namespace in namespaces {
        table.row(vec![
            namespace.name.clone(),
            namespace.status.clone(),
            age_since(namespace.created, now),
        ]);
    }
    table
}

fn deployments_table(deployments: &[DeploymentInfo], now: DateTime<Utc>) -> Table {
    let mut table = Table::new(&["NAME", "READY", "AGE"]);
    for deployment in deployments {
        table.row(vec![
            deployment.name.clone(),
            format!("{}/{}", deployment.ready, deployment.desired),
            age_since(deployment.created, now),
        ]);
    }
    table
}

fn pods_table(pods: &[PodInfo], now: DateTime<Utc>) -> Table {
    let mut table = Table::new(&["NAME", "PHASE", "RESTARTS", "AGE", "CONTAINERS"]);
    for pod in pods {
        let containers: Vec<String> = pod
            .containers
            .iter()
            .map(|container| {
                if container.init {
                    format!("{} (init)", container.name)
                } else {
                    container.name.clone()
                }
            })
            .collect();
        table.row(vec![
            pod.name.clone(),
            pod.phase.clone(),
            pod.restarts.to_string(),
            age_since(pod.created, now),
            containers.join(", "),
        ]);
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use gossip_core::ContainerInfo;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 18, 14, 0, 0).unwrap()
    }

    #[test]
    fn what_list_can_show() {
        assert_eq!("contexts".parse::<Subject>().unwrap(), Subject::Contexts);
        assert_eq!(
            "namespaces".parse::<Subject>().unwrap(),
            Subject::Namespaces
        );
        assert_eq!(
            "flowcpt".parse::<Subject>().unwrap(),
            Subject::Deployments {
                namespace: "flowcpt".into()
            }
        );
        assert_eq!(
            "flowcpt/flowcpt-api".parse::<Subject>().unwrap(),
            Subject::Pods {
                namespace: "flowcpt".into(),
                deployment: "flowcpt-api".into()
            }
        );
    }

    #[test]
    fn what_list_cannot_show() {
        for what in [
            "",
            "flowcpt/",
            "/flowcpt-api",
            "a/b/c",
            "flow*",
            "flowcpt/api-?",
        ] {
            assert!(what.parse::<Subject>().is_err(), "`{what}`");
        }
    }

    #[test]
    fn the_contexts_table_marks_the_current_one() {
        let contexts = [
            ContextInfo {
                name: "aks-vulcan-01".into(),
                current: true,
                cluster: Some("aks-vulcan-01".into()),
                user: Some("clusterUser_vulcan".into()),
                namespace: Some("flowcpt".into()),
            },
            ContextInfo {
                name: "kind-dev".into(),
                current: false,
                cluster: Some("kind-dev".into()),
                user: Some("kind-dev".into()),
                namespace: None,
            },
        ];
        assert_eq!(
            contexts_table(&contexts).render(),
            "CURRENT   NAME            CLUSTER         AUTHINFO             NAMESPACE\n\
             *         aks-vulcan-01   aks-vulcan-01   clusterUser_vulcan   flowcpt\n\
             \u{20}         kind-dev        kind-dev        kind-dev\n"
        );
    }

    #[test]
    fn the_namespaces_table() {
        let namespaces = [NamespaceInfo {
            name: "flowcpt".into(),
            status: "Active".into(),
            created: Some(Utc.with_ymd_and_hms(2026, 11, 6, 14, 0, 0).unwrap()),
        }];
        assert_eq!(
            namespaces_table(&namespaces, now()).render(),
            "NAME      STATUS   AGE\n\
             flowcpt   Active   12d\n"
        );
    }

    #[test]
    fn the_deployments_table_shows_ready_over_desired() {
        let deployments = [
            DeploymentInfo {
                name: "flowcpt-api".into(),
                namespace: "flowcpt".into(),
                ready: 2,
                desired: 3,
                created: Some(Utc.with_ymd_and_hms(2026, 11, 13, 14, 0, 0).unwrap()),
            },
            DeploymentInfo {
                name: "idle".into(),
                namespace: "flowcpt".into(),
                ready: 0,
                desired: 0,
                created: None,
            },
        ];
        assert_eq!(
            deployments_table(&deployments, now()).render(),
            "NAME          READY   AGE\n\
             flowcpt-api   2/3     5d\n\
             idle          0/0     <unknown>\n"
        );
    }

    #[test]
    fn the_pods_table_shows_phase_restarts_age_and_containers() {
        let container = |name: &str, init: bool| ContainerInfo {
            name: name.into(),
            init,
            ready: true,
            restarts: 0,
            state: "running".into(),
        };
        let pods = [PodInfo {
            name: "flowcpt-api-7d9f-xk2p1".into(),
            namespace: "flowcpt".into(),
            phase: "Running".into(),
            restarts: 4,
            created: Some(Utc.with_ymd_and_hms(2026, 11, 18, 10, 48, 0).unwrap()),
            containers: vec![
                container("migrate", true),
                container("api", false),
                container("envoy", false),
            ],
        }];
        assert_eq!(
            pods_table(&pods, now()).render(),
            "NAME                     PHASE     RESTARTS   AGE     CONTAINERS\n\
             flowcpt-api-7d9f-xk2p1   Running   4          3h12m   migrate (init), api, envoy\n"
        );
    }

    #[test]
    fn json_prints_the_same_items() {
        let namespaces = [NamespaceInfo {
            name: "flowcpt".into(),
            status: "Active".into(),
            created: Some(Utc.with_ymd_and_hms(2026, 11, 6, 14, 0, 0).unwrap()),
        }];
        let mut out = Vec::new();
        print(
            &mut out,
            true,
            &namespaces,
            namespaces_table(&namespaces, now()),
            String::new,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            value,
            serde_json::json!([
                { "name": "flowcpt", "status": "Active", "created": "2026-11-06T14:00:00Z" }
            ])
        );

        // Nothing to show is an empty list, not an empty output.
        let mut out = Vec::new();
        let none: [NamespaceInfo; 0] = [];
        print(
            &mut out,
            true,
            &none,
            namespaces_table(&none, now()),
            String::new,
        )
        .unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "[]\n");
    }

    #[test]
    fn an_empty_table_prints_nothing_on_stdout() {
        let mut out = Vec::new();
        let none: [NamespaceInfo; 0] = [];
        print(
            &mut out,
            false,
            &none,
            namespaces_table(&none, now()),
            || "nothing".to_string(),
        )
        .unwrap();
        assert!(out.is_empty());
    }
}
