//! Cluster access: the contexts of the kubeconfig, and a client for one.

use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Client, Config};
use serde::Serialize;

use crate::error::{Error, describe};

/// The name a cluster goes by when Gossip runs inside it and there is no
/// kubeconfig context to name it after.
const IN_CLUSTER: &str = "in-cluster";

/// One context of the kubeconfig.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContextInfo {
    pub name: String,
    /// Whether this is the current context.
    pub current: bool,
    /// The cluster entry the context points at.
    pub cluster: Option<String>,
    pub user: Option<String>,
    /// The default namespace of the context.
    pub namespace: Option<String>,
}

/// One cluster: a context of the kubeconfig and a client for it.
///
/// Gossip is a client only. Everything it does with a cluster is a read.
#[derive(Clone)]
pub struct Cluster {
    name: String,
    client: Client,
}

impl Cluster {
    /// The contexts of the kubeconfig (`KUBECONFIG` or the default file),
    /// with the current one marked.
    pub fn contexts() -> Result<Vec<ContextInfo>, Error> {
        Ok(contexts_of(&Kubeconfig::read()?))
    }

    /// Builds a client for a context of the kubeconfig, or for the current
    /// context when none is named. Credentials come from the kubeconfig as
    /// `kubectl` would use them, exec plugins included.
    ///
    /// This does not talk to the cluster yet; [`check`](Cluster::check) does.
    pub async fn open(context: Option<&str>) -> Result<Cluster, Error> {
        let (name, config) = match context {
            Some(context) => {
                let known = contexts_of(&Kubeconfig::read()?);
                if !known.iter().any(|known| known.name == context) {
                    return Err(Error::NoSuchContext {
                        context: context.to_string(),
                        known: known.into_iter().map(|known| known.name).collect(),
                    });
                }
                let options = KubeConfigOptions {
                    context: Some(context.to_string()),
                    ..KubeConfigOptions::default()
                };
                let config = Config::from_kubeconfig(&options).await?;
                (context.to_string(), config)
            }
            None => {
                let name = Kubeconfig::read()
                    .ok()
                    .and_then(|kubeconfig| kubeconfig.current_context)
                    .unwrap_or_else(|| IN_CLUSTER.to_string());
                (name, Config::infer().await?)
            }
        };
        let client = Client::try_from(config).map_err(|error| Error::Client {
            cluster: name.clone(),
            reason: describe(&error),
        })?;
        Ok(Cluster { name, client })
    }

    /// The name of the cluster: the name of its kubeconfig context.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Asks the API server for its version, to find out whether the cluster
    /// can be reached with these credentials.
    pub async fn check(&self) -> Result<(), Error> {
        self.client
            .apiserver_version()
            .await
            .map(|_| ())
            .map_err(|error| Error::api(format!("reach the cluster `{}`", self.name), &error))
    }

    pub(crate) fn client(&self) -> Client {
        self.client.clone()
    }
}

impl std::fmt::Debug for Cluster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cluster")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

fn contexts_of(kubeconfig: &Kubeconfig) -> Vec<ContextInfo> {
    kubeconfig
        .contexts
        .iter()
        .map(|named| {
            let context = named.context.as_ref();
            ContextInfo {
                name: named.name.clone(),
                current: kubeconfig.current_context.as_deref() == Some(named.name.as_str()),
                cluster: context.map(|context| context.cluster.clone()),
                user: context.and_then(|context| context.user.clone()),
                namespace: context.and_then(|context| context.namespace.clone()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KUBECONFIG: &str = r#"
apiVersion: v1
kind: Config
current-context: aks-vulcan-01
clusters:
  - name: aks-vulcan-01
    cluster:
      server: https://vulcan.example.invalid:443
  - name: kind-dev
    cluster:
      server: https://127.0.0.1:6443
contexts:
  - name: aks-vulcan-01
    context:
      cluster: aks-vulcan-01
      user: clusterUser_vulcan
      namespace: flowcpt
  - name: kind-dev
    context:
      cluster: kind-dev
      user: kind-dev
users:
  - name: clusterUser_vulcan
    user:
      token: not-a-real-token
  - name: kind-dev
    user:
      token: not-a-real-token
"#;

    #[test]
    fn the_contexts_of_a_kubeconfig_with_the_current_one_marked() {
        let kubeconfig = Kubeconfig::from_yaml(KUBECONFIG).unwrap();
        assert_eq!(
            contexts_of(&kubeconfig),
            [
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
            ]
        );
    }

    #[test]
    fn a_kubeconfig_without_a_current_context_marks_none() {
        let text = KUBECONFIG.replace("current-context: aks-vulcan-01\n", "");
        let kubeconfig = Kubeconfig::from_yaml(&text).unwrap();
        let contexts = contexts_of(&kubeconfig);
        assert_eq!(contexts.len(), 2);
        assert!(contexts.iter().all(|context| !context.current));
    }
}
