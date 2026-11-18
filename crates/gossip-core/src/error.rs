//! The errors of talking to a cluster.

use thiserror::Error;

/// Why something that needs the cluster or the kubeconfig did not work.
#[derive(Debug, Error)]
pub enum Error {
    #[error("cannot read the kubeconfig: {0}")]
    Kubeconfig(#[from] kube::config::KubeconfigError),
    #[error("the kubeconfig has no context `{context}`; its contexts are: {}", known.join(", "))]
    NoSuchContext { context: String, known: Vec<String> },
    #[error("cannot find a cluster to talk to: {0}")]
    NoCluster(#[from] kube::config::InferConfigError),
    #[error("cannot set up the connection to the cluster `{cluster}`: {reason}")]
    Client { cluster: String, reason: String },
    #[error("cannot {action}: {reason}")]
    Api { action: String, reason: String },
    #[error("there is no {kind} `{name}` in the namespace `{namespace}`")]
    NotFound {
        kind: &'static str,
        namespace: String,
        name: String,
    },
    #[error("cannot read the selector of the {kind} `{name}`: {reason}")]
    Selector {
        kind: &'static str,
        name: String,
        reason: String,
    },
}

impl Error {
    /// An error of the API server, or of the way to it, while doing `action`.
    pub(crate) fn api(action: impl Into<String>, error: &kube::Error) -> Self {
        Error::Api {
            action: action.into(),
            reason: describe(error),
        }
    }
}

/// A kube error in one line: the message of the API server when it sent
/// one, the chain of causes otherwise, without the names of kube's own error
/// variants in front.
pub(crate) fn describe(error: &kube::Error) -> String {
    match error {
        kube::Error::Api(status) if status.message.is_empty() => {
            format!("the API server answered {} {}", status.code, status.reason)
        }
        kube::Error::Api(status) => format!("{} ({})", status.message, status.code),
        kube::Error::Service(inner) => chain_text(inner.as_ref()),
        kube::Error::HyperError(inner) => chain_text(inner),
        other => chain_text(other),
    }
}

/// An error and its causes in one line. A cause that an outer error has
/// already spelt out is not repeated.
fn chain_text(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(inner) = source {
        let inner_text = inner.to_string();
        if !text.contains(&inner_text) {
            text.push_str(": ");
            text.push_str(&inner_text);
        }
        source = inner.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use kube::core::Status;

    #[test]
    fn an_answer_of_the_api_server_is_its_message_and_code() {
        let forbidden = Status::failure("pods is forbidden: User \"dev\" cannot list", "Forbidden")
            .with_code(403);
        assert_eq!(
            describe(&kube::Error::Api(forbidden.boxed())),
            "pods is forbidden: User \"dev\" cannot list (403)"
        );
        let silent = Status::failure("", "ServiceUnavailable").with_code(503);
        assert_eq!(
            describe(&kube::Error::Api(silent.boxed())),
            "the API server answered 503 ServiceUnavailable"
        );
    }

    #[test]
    fn a_failed_connection_is_its_chain_of_causes() {
        let refused =
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "connection refused");
        let error = kube::Error::Service(Box::new(refused));
        assert_eq!(describe(&error), "connection refused");
    }
}
