//! The release (SPEC-release.md): the check the release workflow runs
//! before it builds anything, and the files of the release that have to
//! name the same targets.

use std::path::{Path, PathBuf};

/// A file of the repository, by its path from the root.
fn repository(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path)
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repository(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// The four targets of the release, with the name each has in an archive.
const TARGETS: [(&str, &str); 4] = [
    ("linux-x64", "x86_64-unknown-linux-musl"),
    ("linux-arm64", "aarch64-unknown-linux-musl"),
    ("macos-arm64", "aarch64-apple-darwin"),
    ("windows-x64", "x86_64-pc-windows-msvc"),
];

#[test]
fn the_workflow_builds_the_four_targets_and_tests_each_archive() {
    let workflow = read(".github/workflows/release.yml");
    for (platform, target) in TARGETS {
        let build = format!("- {{ platform: {platform}, target: {target}, os: ");
        assert_eq!(workflow.matches(&build).count(), 1, "{build}");
        // The smoke test has a row for the platform too.
        let smoke = format!("- {{ platform: {platform}, os: ");
        assert_eq!(workflow.matches(&smoke).count(), 1, "{smoke}");
    }
    // No other target is built.
    assert_eq!(workflow.matches(", target: ").count(), TARGETS.len());
}

#[test]
fn the_licences_are_collected_for_the_targets_that_are_built() {
    let about: toml::Table = toml::from_str(&read("about.toml")).unwrap();
    let mut listed: Vec<&str> = about["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|target| target.as_str().unwrap())
        .collect();
    let mut built: Vec<&str> = TARGETS.iter().map(|(_, target)| *target).collect();
    listed.sort_unstable();
    built.sort_unstable();
    assert_eq!(listed, built);
}

#[test]
fn the_licences_are_those_of_the_crate_of_the_binary() {
    // Not of the workspace: a crate that is in the workspace and not in
    // the binary has no place in the archive's list.
    let workflow = read(".github/workflows/release.yml");
    assert!(workflow.contains(
        "cargo about generate --manifest-path crates/gossip/Cargo.toml -c about.toml about.hbs"
    ));
    assert_eq!(workflow.matches("cargo about generate").count(), 1);
}

#[test]
fn only_the_release_job_may_write_to_the_repository() {
    let workflow = read(".github/workflows/release.yml");
    // Read-only for the workflow, and one job that asks for more.
    assert!(workflow.contains("\npermissions:\n  contents: read\n"));
    assert_eq!(workflow.matches("contents: write").count(), 1);
    let release = workflow.split("\n  release:\n").nth(1).unwrap();
    assert!(release.contains("    permissions:\n      contents: write\n"));
}

/// The script the first job of the release workflow runs.
#[cfg(unix)]
mod check_version {
    use super::repository;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// Runs the script; its exit code and what it printed.
    fn check(tag: &str, manifest: &Path) -> (i32, String) {
        let output = Command::new("bash")
            .arg(repository(".github/scripts/check-version.sh"))
            .arg(tag)
            .arg(manifest)
            .output()
            .expect("bash runs");
        let mut text = String::from_utf8(output.stdout).unwrap();
        text.push_str(&String::from_utf8(output.stderr).unwrap());
        (output.status.code().expect("an exit code"), text)
    }

    /// A manifest of its own, in the temporary directory.
    fn manifest(test: &str, text: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gossip-check-version-{}-{test}.toml",
            std::process::id()
        ));
        std::fs::write(&path, text).unwrap();
        path
    }

    const WORKSPACE: &str = "\
[workspace]
members = [\"crates/gossip\"]

[workspace.package]
edition = \"2024\"
version = \"0.3.0\"

[workspace.dependencies]
other = { path = \"other\", version = \"9.9.9\" }
";

    #[test]
    fn the_tag_of_the_version_of_this_repository_passes() {
        let own = repository("Cargo.toml");
        let version = env!("CARGO_PKG_VERSION");
        let (code, text) = check(&format!("v{version}"), &own);
        assert_eq!(code, 0, "{text}");
        assert!(text.contains(&format!("is version {version} of")), "{text}");
        // So does a pre-release of it: the dry run of the workflow.
        let (code, text) = check(&format!("v{version}-rc1"), &own);
        assert_eq!(code, 0, "{text}");
    }

    #[test]
    fn a_tag_of_another_version_fails_and_names_both() {
        let manifest = manifest("another", WORKSPACE);
        for tag in ["v0.3.1", "v0.4.0", "v1.3.0", "v0.2.0-rc1", "v9.9.9"] {
            let (code, text) = check(tag, &manifest);
            assert_eq!(code, 1, "{tag}: {text}");
            assert!(
                text.starts_with(&format!("the tag {tag} is not version 0.3.0, ")),
                "{text}"
            );
        }
        for tag in ["v0.3.0", "v0.3.0-rc1", "v0.3.0-rc.2"] {
            let (code, text) = check(tag, &manifest);
            assert_eq!(code, 0, "{tag}: {text}");
        }
        std::fs::remove_file(manifest).unwrap();
    }

    #[test]
    fn a_pre_release_in_the_manifest_is_matched_by_its_own_tag_only() {
        let manifest = manifest("pre", "[workspace.package]\nversion = \"0.3.0-rc1\"\n");
        assert_eq!(check("v0.3.0-rc1", &manifest).0, 0);
        assert_eq!(check("v0.3.0", &manifest).0, 1);
        assert_eq!(check("v0.3.0-rc2", &manifest).0, 1);
        std::fs::remove_file(manifest).unwrap();
    }

    #[test]
    fn what_is_not_a_release_tag_fails_before_anything_is_compared() {
        let manifest = manifest("form", WORKSPACE);
        for tag in [
            "0.3.0",
            "v0.3",
            "v0.3.0-",
            "v0.3.0 ",
            "v0.3.0-rc1-x",
            "vv0.3.0",
            "v0.3.0;ls",
            "v0.3.0\nv0.3.0",
            "main",
        ] {
            let (code, text) = check(tag, &manifest);
            assert_eq!(code, 1, "{tag:?}: {text}");
            assert!(text.contains("is not a release tag"), "{tag:?}: {text}");
        }
        std::fs::remove_file(manifest).unwrap();
    }

    #[test]
    fn a_manifest_without_a_workspace_version_fails() {
        // A version elsewhere in the file is not the version of the
        // workspace.
        let manifest = manifest(
            "none",
            "[package]\nversion = \"0.3.0\"\n\n[workspace.package]\nedition = \"2024\"\n",
        );
        let (code, text) = check("v0.3.0", &manifest);
        assert_eq!(code, 1, "{text}");
        assert!(
            text.contains("has no version under [workspace.package]"),
            "{text}"
        );
        std::fs::remove_file(manifest).unwrap();
    }
}
