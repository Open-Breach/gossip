//! `gossip templates` and the config file, run as a program. None of these
//! needs a cluster.

use std::path::PathBuf;
use std::process::{Command, Output};

/// The example of the spec, section 6.
const CONFIG: &str = r#"[defaults]
context = "aks-vulcan-01"
tail = 20
events = true

[templates.dev-api]
context = "aks-vulcan-01"
targets = ["flowcpt/flowcpt-api", "flowcpt/flowcpt-web-api"]
container = "api"
filter = { all = [
  { field = "level", op = ">=", value = "info" },
  { any = [
    { field = "f.tenant_id", op = "==", value = "t_123" },
    { field = "kind", op = "==", value = "event" },
  ] },
] }
fields = ["tenant_id", "request_id"]

[templates.staging-all]
context = "aks-vulcan-01"
targets = ["flowcpt-stg/*"]
since = "5m"
"#;

/// A config file of its own for one test.
fn config_file(test: &str, text: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("gossip-{}-{test}.toml", std::process::id()));
    std::fs::write(&path, text).unwrap();
    path
}

fn gossip(config: &PathBuf, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gossip"))
        .args(args)
        .env("GOSSIP_CONFIG", config)
        .output()
        .expect("gossip should run")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn templates_lists_the_templates_with_their_targets() {
    let config = config_file("list", CONFIG);
    let output = gossip(&config, &["templates"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "NAME          TARGETS\n\
         dev-api       flowcpt/flowcpt-api, flowcpt/flowcpt-web-api\n\
         staging-all   flowcpt-stg\n"
    );
    std::fs::remove_file(config).unwrap();
}

#[test]
fn templates_show_prints_one_as_the_config_file_writes_it() {
    let config = config_file("show", CONFIG);
    let output = gossip(&config, &["templates", "show", "dev-api"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        r#"[templates.dev-api]
context = "aks-vulcan-01"
targets = ["flowcpt/flowcpt-api", "flowcpt/flowcpt-web-api"]
container = "api"
filter = { all = [
  { field = "level", op = ">=", value = "info" },
  { any = [
    { field = "f.tenant_id", op = "==", value = "t_123" },
    { field = "kind", op = "==", value = "event" },
  ] },
] }
fields = ["tenant_id", "request_id"]
"#
    );
    std::fs::remove_file(config).unwrap();
}

#[test]
fn a_template_that_is_not_there_exits_1_and_names_the_ones_that_are() {
    let config = config_file("missing-template", CONFIG);
    for args in [
        &["templates", "show", "dev"][..],
        &["follow", "--template", "dev"],
    ] {
        let output = gossip(&config, args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(
            text(&output.stderr),
            "gossip: there is no template `dev`; the templates are: dev-api, staging-all\n",
            "{args:?}"
        );
    }
    std::fs::remove_file(config).unwrap();
}

#[test]
fn an_unknown_key_in_the_config_file_is_an_error_with_the_line_number() {
    let broken = CONFIG.replace(
        "targets = [\"flowcpt-stg/*\"]",
        "tragets = [\"flowcpt-stg/*\"]",
    );
    let config = config_file("unknown-key", &broken);
    for args in [
        &["templates"][..],
        &["templates", "show", "dev-api"],
        &["follow", "--template", "dev-api"],
        &["follow", "flowcpt"],
    ] {
        let output = gossip(&config, args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        let stderr = text(&output.stderr);
        assert!(stderr.starts_with("gossip: the config file "), "{stderr}");
        assert!(stderr.contains("line 21"), "{stderr}");
        assert!(stderr.contains("tragets"), "{stderr}");
    }
    std::fs::remove_file(config).unwrap();
}

#[test]
fn a_config_file_that_gossip_config_names_must_exist() {
    let missing = std::env::temp_dir().join("gossip-no-such-config.toml");
    let output = gossip(&missing, &["templates"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).starts_with("gossip: cannot read the config file "),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn without_a_config_file_there_are_no_templates() {
    // No GOSSIP_CONFIG, and a home directory with nothing in it.
    let home = std::env::temp_dir().join(format!("gossip-{}-empty-home", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_gossip"))
        .arg("templates")
        .env_remove("GOSSIP_CONFIG")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .output()
        .expect("gossip should run");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(output.stdout.is_empty());
    let stderr = text(&output.stderr);
    assert!(stderr.contains("holds no templates"), "{stderr}");
    assert!(stderr.contains("gossip.toml"), "{stderr}");
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn follow_takes_targets_or_a_template_not_both() {
    let config = config_file("both", CONFIG);
    let output = gossip(&config, &["follow", "flowcpt", "--template", "dev-api"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        text(&output.stderr).contains("cannot be used with"),
        "{}",
        text(&output.stderr)
    );
    let output = gossip(&config, &["follow"]);
    assert_eq!(output.status.code(), Some(2));
    std::fs::remove_file(config).unwrap();
}

#[test]
fn the_config_example_of_the_readme_is_a_valid_config() {
    let readme = include_str!("../../../README.md");
    // The example begins with the one key that stands outside every table.
    let start = readme
        .find("```toml\npause_buffer")
        .expect("the README has a config example");
    let example = &readme[start + "```toml\n".len()..];
    let example = &example[..example.find("```").expect("the example ends")];

    // It shows everything the file can hold beside the templates.
    for part in ["pause_buffer = ", "[defaults]", "[columns]", "[templates."] {
        assert!(example.contains(part), "the example has no {part}");
    }
    let config = config_file("readme", example);
    let output = gossip(&config, &["templates"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "NAME       TARGETS\n\
         api-json   shop/api\n\
         shop       shop/api, shop/web\n"
    );
    let output = gossip(&config, &["templates", "show", "api-json"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        text(&output.stdout).contains(r#"format = { api = "json" }"#),
        "{}",
        text(&output.stdout)
    );
    std::fs::remove_file(config).unwrap();
}
