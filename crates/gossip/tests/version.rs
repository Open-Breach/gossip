//! The version the binary reports (SPEC-release.md, section 3, and
//! acceptance 4): the tag's in a release build, the version of `Cargo.toml`
//! with `-dev` and the commit otherwise.

use std::path::Path;
use std::process::Command;

/// The build script, for the function that puts the version together. Its
/// `main` is not run here.
#[allow(dead_code)]
#[path = "../build.rs"]
mod build;

use build::version_text;

/// What `gossip --version` prints.
fn reported() -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_gossip"))
        .arg("--version")
        .output()
        .expect("gossip runs");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    String::from_utf8(output.stdout).unwrap()
}

/// What a git command prints in the sources of this crate, on one line.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    (output.status.success() && !text.trim().is_empty()).then(|| text.trim().to_string())
}

/// The short commit, when these sources are a git checkout of their own.
fn commit_of_this_checkout() -> Option<String> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .parent()?
        .canonicalize()
        .ok()?;
    let top = Path::new(&git(&["rev-parse", "--show-toplevel"])?)
        .canonicalize()
        .ok()?;
    (top == workspace)
        .then(|| git(&["rev-parse", "--short=7", "HEAD"]))
        .flatten()
}

#[test]
fn the_binary_reports_the_version_it_was_built_as() {
    let package = env!("CARGO_PKG_VERSION");
    let expected = match option_env!("GOSSIP_VERSION").map(str::trim) {
        // Built as a release: the version that was handed in, as it is.
        Some(given) if !given.is_empty() => format!("gossip {given}\n"),
        // Built from a checkout: the version of Cargo.toml as a development
        // build, with the commit the checkout is at.
        _ => match commit_of_this_checkout() {
            Some(commit) => format!("gossip {package}-dev ({commit})\n"),
            None => format!("gossip {package}-dev\n"),
        },
    };
    assert_eq!(reported(), expected);
}

#[test]
fn a_build_from_a_checkout_names_its_commit() {
    // The test runs in the repository; were the sources unpacked from an
    // archive, there would be no commit to name.
    let Some(commit) = commit_of_this_checkout() else {
        return;
    };
    assert!(commit.len() >= 7, "{commit}");
    assert!(commit.chars().all(|c| c.is_ascii_hexdigit()), "{commit}");
    if option_env!("GOSSIP_VERSION").is_none_or(|given| given.trim().is_empty()) {
        assert!(
            reported().ends_with(&format!("-dev ({commit})\n")),
            "{}",
            reported()
        );
    }
}

#[test]
fn the_version_handed_in_is_the_version() {
    assert_eq!(
        version_text(Some("0.1.0"), "0.1.0", None).as_deref(),
        Ok("0.1.0")
    );
    // The tag is the version, whatever Cargo.toml says, and a pre-release
    // suffix is part of it.
    assert_eq!(
        version_text(Some("0.2.0-rc1"), "0.1.0", Some("9b3fafc")).as_deref(),
        Ok("0.2.0-rc1")
    );
    // A line end from the shell is not part of it.
    assert_eq!(
        version_text(Some("0.1.0\n"), "0.1.0", None).as_deref(),
        Ok("0.1.0")
    );
}

#[test]
fn without_a_version_handed_in_it_is_a_development_build() {
    assert_eq!(
        version_text(None, "0.1.0", Some("9b3fafc")).as_deref(),
        Ok("0.1.0-dev (9b3fafc)")
    );
    // Outside a git checkout there is no commit to name.
    assert_eq!(
        version_text(None, "0.1.0", None).as_deref(),
        Ok("0.1.0-dev")
    );
    // A variable that is set to nothing is a variable that is not set.
    for empty in ["", "  ", "\n"] {
        assert_eq!(
            version_text(Some(empty), "0.1.0", Some("9b3fafc")).as_deref(),
            Ok("0.1.0-dev (9b3fafc)"),
            "{empty:?}"
        );
    }
}

#[test]
fn a_version_with_a_space_in_it_stops_the_build() {
    for wrong in ["0.1.0 beta", "0.1.0\n0.2.0", "0.1.0\tx"] {
        let error = version_text(Some(wrong), "0.1.0", None).unwrap_err();
        assert!(error.starts_with("GOSSIP_VERSION is `"), "{error}");
        assert!(!error.contains('\n'), "{error}");
    }
}
