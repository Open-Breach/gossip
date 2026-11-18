//! `gossip list`, run as a program. None of these needs a cluster: the
//! contexts come from a kubeconfig file, and the cluster in it is a port
//! where nothing listens.

use std::path::PathBuf;
use std::process::{Command, Output};

const KUBECONFIG: &str = r#"
apiVersion: v1
kind: Config
current-context: nowhere
clusters:
  - name: nowhere
    cluster:
      server: https://127.0.0.1:1
contexts:
  - name: nowhere
    context:
      cluster: nowhere
      user: nobody
      namespace: flowcpt
  - name: elsewhere
    context:
      cluster: nowhere
      user: nobody
users:
  - name: nobody
    user:
      token: not-a-real-token
"#;

/// A kubeconfig file of its own for one test.
fn kubeconfig(test: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("gossip-{}-{test}.kubeconfig", std::process::id()));
    std::fs::write(&path, KUBECONFIG).unwrap();
    path
}

fn gossip(kubeconfig: &PathBuf, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gossip"));
    command
        .args(args)
        .env("KUBECONFIG", kubeconfig)
        .env_remove("GOSSIP_CONFIG");
    // A proxy of the machine the tests run on must not get between gossip
    // and the port where nothing listens.
    for variable in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        command.env_remove(variable);
    }
    command.output().expect("gossip should run")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn list_contexts_marks_the_current_one() {
    let kubeconfig = kubeconfig("contexts");
    let output = gossip(&kubeconfig, &["list", "contexts"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "CURRENT   NAME        CLUSTER   AUTHINFO   NAMESPACE\n\
         *         nowhere     nowhere   nobody     flowcpt\n\
         \u{20}         elsewhere   nowhere   nobody\n"
    );
    std::fs::remove_file(kubeconfig).unwrap();
}

#[test]
fn list_contexts_as_json() {
    let kubeconfig = kubeconfig("contexts-json");
    let output = gossip(&kubeconfig, &["list", "contexts", "--json"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value[0]["name"], "nowhere");
    assert_eq!(value[0]["current"], true);
    assert_eq!(value[0]["namespace"], "flowcpt");
    assert_eq!(value[1]["name"], "elsewhere");
    assert_eq!(value[1]["current"], false);
    std::fs::remove_file(kubeconfig).unwrap();
}

#[test]
fn a_cluster_that_cannot_be_reached_exits_1_with_the_error() {
    let kubeconfig = kubeconfig("unreachable");
    for args in [
        &["list", "namespaces"][..],
        &["list", "flowcpt"],
        &["list", "flowcpt/flowcpt-api"],
        &["list", "namespaces", "--context", "elsewhere"],
    ] {
        let output = gossip(&kubeconfig, args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let stderr = text(&output.stderr);
        assert!(stderr.starts_with("gossip: cannot "), "{args:?}: {stderr}");
        assert!(
            stderr.contains("namespace") || stderr.contains("deployment"),
            "{args:?}: {stderr}"
        );
        assert_eq!(stderr.lines().count(), 1, "{args:?}: {stderr}");
    }
    std::fs::remove_file(kubeconfig).unwrap();
}

#[test]
fn a_context_the_kubeconfig_does_not_have_exits_1() {
    let kubeconfig = kubeconfig("no-such-context");
    let output = gossip(&kubeconfig, &["list", "namespaces", "--context", "missing"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "gossip: the kubeconfig has no context `missing`; its contexts are: nowhere, elsewhere\n"
    );
    std::fs::remove_file(kubeconfig).unwrap();
}

#[test]
fn a_kubeconfig_that_is_not_there_exits_1() {
    let missing = std::env::temp_dir().join("gossip-no-such-kubeconfig");
    let output = gossip(&missing, &["list", "contexts"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).starts_with("gossip: cannot read the kubeconfig"));
}

#[test]
fn list_refuses_what_it_cannot_show() {
    let kubeconfig = kubeconfig("refuse");
    for what in ["a/b/c", "flow*", "flowcpt/"] {
        let output = gossip(&kubeconfig, &["list", what]);
        assert_eq!(output.status.code(), Some(1), "{what}");
        assert!(text(&output.stderr).starts_with("gossip: "), "{what}");
    }
    std::fs::remove_file(kubeconfig).unwrap();
}

#[test]
fn help_runs() {
    let kubeconfig = kubeconfig("help");
    for args in [&["--help"][..], &["list", "--help"], &["--version"]] {
        let output = gossip(&kubeconfig, args);
        assert!(output.status.success(), "{args:?}");
        assert!(!output.stdout.is_empty(), "{args:?}");
    }
    let help = text(&gossip(&kubeconfig, &["list", "--help"]).stdout);
    assert!(
        help.contains("gossip list <namespace>/<deployment>"),
        "{help}"
    );
    std::fs::remove_file(kubeconfig).unwrap();
}
