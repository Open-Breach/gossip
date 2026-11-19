//! The version the binary reports (SPEC-release.md, section 3).
//!
//! The tag is the version: the release workflow passes it in
//! `GOSSIP_VERSION`, and `gossip --version` prints `gossip 0.1.0`. A build
//! without it, from a checkout or with `cargo install --path`, prints the
//! version of `Cargo.toml` followed by `-dev`, and by the short commit when
//! it runs inside a git checkout of this repository:
//! `gossip 0.1.0-dev (9b3fafc)`.
//!
//! The text goes to the compiler as `GOSSIP_BUILD_VERSION`, which
//! `src/cli.rs` hands to `clap`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=GOSSIP_VERSION");
    let given = std::env::var("GOSSIP_VERSION").ok();
    let package = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let commit = match given.as_deref().map(str::trim) {
        // A release build does not ask git for anything.
        Some(given) if !given.is_empty() => None,
        _ => commit(),
    };
    match version_text(given.as_deref(), &package, commit.as_deref()) {
        Ok(version) => println!("cargo:rustc-env=GOSSIP_BUILD_VERSION={version}"),
        Err(error) => {
            // Cargo shows the line as the reason the build stopped.
            println!("cargo::error={error}");
            std::process::exit(1);
        }
    }
}

/// The version to report: what `GOSSIP_VERSION` gives, or the version of the
/// package as a development build.
pub fn version_text(
    given: Option<&str>,
    package: &str,
    commit: Option<&str>,
) -> Result<String, String> {
    match given.map(str::trim) {
        Some(given) if !given.is_empty() => {
            if given.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err(format!(
                    "GOSSIP_VERSION is `{}`, which is not a version: it holds a space",
                    given.escape_debug()
                ));
            }
            Ok(given.to_string())
        }
        // Not set, or set to nothing: a development build.
        _ => Ok(match commit {
            Some(commit) => format!("{package}-dev ({commit})"),
            None => format!("{package}-dev"),
        }),
    }
}

/// The short commit of the checkout this build runs in. None when git is
/// not there, or when the sources are not a git checkout of their own: a
/// directory that merely lies inside another repository says nothing about
/// these sources.
fn commit() -> Option<String> {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?);
    let workspace = manifest.parent()?.parent()?.canonicalize().ok()?;
    let top = PathBuf::from(git(&manifest, &["rev-parse", "--show-toplevel"])?);
    if top.canonicalize().ok()? != workspace {
        return None;
    }
    let commit = git(&manifest, &["rev-parse", "--short=7", "HEAD"])?;
    watch_head(&manifest);
    Some(commit)
}

/// Has cargo run this script again when the checkout moves to another
/// commit, so that the commit in the version is never an older one.
fn watch_head(manifest: &Path) {
    let absolute = |path: String| {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            path
        } else {
            manifest.join(path)
        }
    };
    let mut files = Vec::new();
    if let Some(dir) = git(manifest, &["rev-parse", "--git-dir"]).map(absolute) {
        // The log of HEAD grows with every commit, checkout, reset and pull.
        files.push(dir.join("HEAD"));
        files.push(dir.join("logs").join("HEAD"));
    }
    if let Some(dir) = git(manifest, &["rev-parse", "--git-common-dir"]).map(absolute) {
        if let Some(branch) = git(manifest, &["symbolic-ref", "-q", "HEAD"]) {
            files.push(dir.join(branch));
        }
        files.push(dir.join("packed-refs"));
    }
    // Only what is there: cargo runs the script every time for a file that
    // is missing.
    for file in files.iter().filter(|file| file.exists()) {
        println!("cargo:rerun-if-changed={}", file.display());
    }
}

/// What a git command prints, on one line. None when it fails.
fn git(directory: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}
