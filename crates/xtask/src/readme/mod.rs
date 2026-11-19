//! The generator of the README, and the check that holds the README
//! against the code (SPEC-release.md, sections 4.2 to 4.5).
//!
//! `README.md` has two kinds of text. The fixed parts are written by hand
//! and are not reworded without Serge: the tagline, the paragraph of what
//! Gossip is, the status text, the warning, the sentences of the
//! walkthrough, the known limits. The generated parts stand between
//! `<!-- generated:name -->` and `<!-- /generated:name -->` and are written
//! by `cargo xtask readme`: the status line with the version, the sample of
//! the output, the Install section, what every step of the walkthrough
//! prints, and the Reference.
//!
//! [`check`] is what `cargo test` and `cargo xtask readme --check` run: it
//! says everything in a README that is not what the code says.

pub mod reference;
#[cfg(unix)]
pub mod stage;
#[cfg(unix)]
pub mod terminal;
#[cfg(unix)]
pub mod walkthrough;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, bail};
use gossip::reference::{COMMANDS, KEYS};

use self::reference::{FLAG_COMMANDS, PLACES};

/// The version the README describes: the one `Cargo.toml` carries.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where the repository is, and where a release has its archives.
const REPOSITORY: &str = "https://github.com/Open-Breach/gossip";

// ---- the fixed texts of SPEC-release.md 4.2, word for word ------------------

pub const TAGLINE: &str = "Everything your cluster says behind your back.";

pub const WHAT_IT_IS: &str = "Gossip is a terminal tool that follows the logs of Kubernetes \
    workloads. It keeps following them across pod restarts and rollouts, merges the cluster's \
    own events into the same stream, reads JSON and logfmt lines as data, and can be changed \
    while it runs: filter, pause, add a service, save the session.";

pub const STATUS: &str = "Gossip is young and has been tried by hand against one cluster so \
    far, so expect bugs. Please tell us what you see.";

pub const WARNING: &str = "**Use with caution.** Gossip is written largely by an AI coding \
    agent working from [SPEC.md](SPEC.md), with human review, a strict test gate and CI. That \
    makes it fast to build and easy to audit, but it is still young software. Gossip only \
    reads from your cluster and never creates, changes or deletes anything in it, but it runs \
    with the credentials of your kubeconfig. Until 1.0, treat every release as a preview. If \
    it does something surprising, that's a bug: please report it.";

/// The known limits, in their order. The second ends in the spec with "how
/// to raise it", which the README answers in its own words after these.
pub const KNOWN_LIMITS: [&str; 6] = [
    "One cluster at a time.",
    "One connection per followed container, so a few hundred at once can reach the open-file \
     limit (256 by default on macOS)",
    "The kubelet polls a container's log file, which adds a second or two that no client can \
     remove.",
    "History reaches as far back as the node keeps the log files, and events for one hour. \
     Record and replay are for anything longer.",
    "In a plain-text service, lines printed in the two seconds after a panic join the panic's \
     record.",
    "Tried on Linux only so far; the macOS, Windows and arm64 Linux builds pass their smoke \
     test and nothing more.",
];

/// The headings of the README, in their order.
pub const HEADINGS: [&str; 5] = [
    "# Gossip",
    "## Install",
    "## Using Gossip",
    "## Reference",
    "## Known limits",
];

/// What the closing lines link to: the three specs, and building and
/// development.
pub const CLOSING_LINKS: [&str; 5] = [
    "(SPEC.md)",
    "(SPEC-interactive.md)",
    "(SPEC-release.md)",
    "(docs/development.md)",
    "(LICENSE)",
];

/// The names of the real cluster Gossip was tried against. None of them
/// may be in the README: its examples are made up.
const REAL_NAMES: [&str; 6] = [
    "fc-stg",
    "fc-api",
    "fc-web-api",
    "flowcpt",
    "vulcan",
    "auth0",
];

/// The generated blocks, in the order they stand in the README: the name of
/// each and whether it is one of the walkthrough.
pub const BLOCKS: [&str; 13] = [
    "status",
    "sample",
    "install",
    "walk-find",
    "walk-follow",
    "walk-restart",
    "walk-narrow",
    "walk-pause",
    "walk-add",
    "walk-save",
    "walk-back",
    "walk-record",
    "reference",
];

/// A generated block: its name and what stands between its markers.
pub type Block = (String, String);

fn open(name: &str) -> String {
    format!("<!-- generated:{name} -->")
}

fn close(name: &str) -> String {
    format!("<!-- /generated:{name} -->")
}

/// The status line: the version and the status text.
pub fn status_line(version: &str) -> String {
    format!("v{version} · {STATUS}")
}

/// The Install section for a version.
pub fn install(version: &str) -> String {
    include_str!("install.md")
        .trim_end()
        .replace(
            "{{download}}",
            &format!("{REPOSITORY}/releases/download/v{version}"),
        )
        .replace("{{repository}}", REPOSITORY)
        .replace("{{version}}", version)
}

/// Lines of output as a block of the README.
#[cfg(unix)]
fn fenced(lines: &[String]) -> String {
    format!("```text\n{}\n```", lines.join("\n"))
}

/// Every generated block as the code and the built binary make it now. The
/// walkthrough is played by the binary at this path.
#[cfg(unix)]
pub async fn generate(binary: &Path) -> anyhow::Result<Vec<Block>> {
    let mut blocks = vec![
        ("status".to_string(), status_line(VERSION)),
        (
            "sample".to_string(),
            fenced(&walkthrough::sample(binary).await?),
        ),
        ("install".to_string(), install(VERSION)),
    ];
    for (name, lines) in walkthrough::steps(binary).await? {
        blocks.push((name.to_string(), fenced(&lines)));
    }
    blocks.push(("reference".to_string(), reference::reference()));
    let names: Vec<&str> = blocks.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, BLOCKS, "the blocks and their list");
    Ok(blocks)
}

/// The walkthrough needs a terminal for the binary, which this platform
/// does not give it.
#[cfg(not(unix))]
pub async fn generate(_binary: &Path) -> anyhow::Result<Vec<Block>> {
    bail!(
        "the walkthrough of the README runs the program on a pseudo-terminal, which this \
         generator opens on Linux and macOS only"
    )
}

/// The content of a generated block of a README. None when the README does
/// not have the block.
pub fn block<'a>(readme: &'a str, name: &str) -> Option<&'a str> {
    let (open, close) = (open(name), close(name));
    let start = readme.find(&open)? + open.len();
    let end = start + readme[start..].find(&close)?;
    Some(readme[start..end].trim_matches('\n'))
}

/// The README with these blocks written between their markers. A block the
/// README has no markers for is an error: the generator fills places, it
/// does not make them.
pub fn splice(readme: &str, blocks: &[Block]) -> anyhow::Result<String> {
    let mut text = readme.to_string();
    for (name, content) in blocks {
        let (open, close) = (open(name), close(name));
        let Some(start) = text.find(&open) else {
            bail!("the README has no `{open}` to write the block after");
        };
        let after = start + open.len();
        let Some(length) = text[after..].find(&close) else {
            bail!("the README has no `{close}` after `{open}`");
        };
        text.replace_range(after..after + length, &format!("\n{content}\n"));
    }
    Ok(text)
}

/// Everything in a README that is not what the code and the spec say: an
/// empty list for a README that is true.
pub fn check(readme: &str, generated: &[Block]) -> Vec<String> {
    let mut problems = Vec::new();
    check_fixed_texts(readme, &mut problems);
    check_order(readme, &mut problems);
    check_blocks(readme, generated, &mut problems);
    check_reference(readme, &mut problems);
    check_names(readme, &mut problems);
    problems
}

/// The fixed texts of the spec are in the README word for word.
fn check_fixed_texts(readme: &str, problems: &mut Vec<String>) {
    let mut fixed = vec![
        ("the tagline", format!("*{TAGLINE}*")),
        ("the paragraph of what Gossip is", WHAT_IT_IS.to_string()),
        ("the status text", STATUS.to_string()),
        ("the warning", WARNING.to_string()),
    ];
    for limit in KNOWN_LIMITS {
        fixed.push(("a known limit", format!("- {limit}")));
    }
    for (what, text) in fixed {
        if !readme.contains(&text) {
            problems.push(format!("{what} is not in the README word for word: {text}"));
        }
    }
}

/// The parts stand in the order of the spec.
fn check_order(readme: &str, problems: &mut Vec<String>) {
    let mut parts: Vec<(String, String)> = Vec::new();
    let mut part = |what: &str, text: String| parts.push((what.to_string(), text));
    part("the title", format!("{}\n", HEADINGS[0]));
    part("the tagline", format!("*{TAGLINE}*"));
    part("the paragraph of what Gossip is", WHAT_IT_IS.to_string());
    part("the status line", open("status"));
    part("the sample of the output", open("sample"));
    part("the warning", WARNING.to_string());
    part("the Install section", format!("\n{}\n", HEADINGS[1]));
    part("its generated block", open("install"));
    part("the walkthrough", format!("\n{}\n", HEADINGS[2]));
    for name in BLOCKS.iter().filter(|name| name.starts_with("walk-")) {
        part("a step of the walkthrough", open(name));
    }
    part("the Reference", format!("\n{}\n", HEADINGS[3]));
    part("its generated block", open("reference"));
    part("the known limits", format!("\n{}\n", HEADINGS[4]));
    for limit in KNOWN_LIMITS {
        part("a known limit", format!("- {limit}"));
    }
    for link in CLOSING_LINKS {
        part("a link of the closing lines", link.to_string());
    }

    let mut from = 0;
    for (what, text) in parts {
        match readme[from..].find(&text) {
            Some(at) => from += at + text.len(),
            None if readme.contains(&text) => {
                problems.push(format!("{what} is out of its place in the README: {text}"))
            }
            None => problems.push(format!("the README lacks {what}: {text}")),
        }
    }

    // And no other section has crept in between them.
    for line in readme.lines() {
        let top = line.starts_with("# ") || line.starts_with("## ");
        if top && !HEADINGS.contains(&line) && !inside_a_fence(readme, line) {
            problems.push(format!("`{line}` is a section the structure does not have"));
        }
    }
}

/// Whether a line of the README stands inside a fenced block.
fn inside_a_fence(readme: &str, wanted: &str) -> bool {
    let mut inside = false;
    for line in readme.lines() {
        if line.trim_start().starts_with("```") {
            inside = !inside;
        } else if line == wanted {
            return inside;
        }
    }
    false
}

/// Every generated block is in the README, and is what the generator would
/// write now.
fn check_blocks(readme: &str, generated: &[Block], problems: &mut Vec<String>) {
    for (name, content) in generated {
        match block(readme, name) {
            None => problems.push(format!(
                "the README has no generated block `{name}`; `cargo xtask readme` writes it \
                 between `{}` and `{}`",
                open(name),
                close(name)
            )),
            Some(found) if found != content => problems.push(format!(
                "the generated block `{name}` is not what the generator writes now; run \
                 `cargo xtask readme`.\n{}",
                first_difference(found, content)
            )),
            Some(_) => {}
        }
    }
    let known: BTreeSet<&str> = generated.iter().map(|(name, _)| name.as_str()).collect();
    for marker in readme.split("<!-- generated:").skip(1) {
        let name = marker.split(" -->").next().unwrap_or_default();
        if !known.contains(name) {
            problems.push(format!(
                "the README has a generated block `{name}` that the generator does not write"
            ));
        }
    }
}

/// The first line two texts differ in, as the README has it and as the
/// generator writes it.
fn first_difference(found: &str, wanted: &str) -> String {
    let mut found_lines = found.lines();
    let mut wanted_lines = wanted.lines();
    let mut number = 1;
    loop {
        match (found_lines.next(), wanted_lines.next()) {
            (Some(found), Some(wanted)) if found == wanted => number += 1,
            (found, wanted) => {
                return format!(
                    "line {number} of the block\n  in the README:  {}\n  generated now:  {}",
                    found.unwrap_or("(nothing)"),
                    wanted.unwrap_or("(nothing)")
                );
            }
        }
    }
}

/// The Reference of the README names what the code has, and nothing else:
/// every command, every flag of `follow`, `replay` and `list` with its
/// default, every key and every `:` command.
fn check_reference(readme: &str, problems: &mut Vec<String>) {
    let Some(start) = readme.find(&format!("\n{}\n", HEADINGS[3])) else {
        // Said already, with the order.
        return;
    };
    let end = readme[start + 1..]
        .find("\n## ")
        .map_or(readme.len(), |end| start + 1 + end);
    let reference = &readme[start..end];

    // Commands.
    let in_code: Vec<String> = reference::commands()
        .into_iter()
        .map(|command| command.usage)
        .collect();
    let in_readme = first_cells(&table_after(reference, "### Commands"));
    compare("command", "### Commands", &in_code, &in_readme, problems);

    // Flags, with their defaults.
    for command in FLAG_COMMANDS {
        let heading = format!("#### `gossip {command}`");
        let rows = table_after(reference, &heading);
        let flags = reference::flags(command);
        let in_code: Vec<String> = flags.iter().map(|flag| flag.name.clone()).collect();
        compare(
            &format!("flag of `gossip {command}`"),
            &heading,
            &in_code,
            &first_cells(&rows),
            problems,
        );
        for flag in flags {
            let row = rows.iter().find(|row| row[0] == flag.name);
            let default = row.and_then(|row| row.get(2));
            if default.is_some_and(|default| *default != flag.default) {
                problems.push(format!(
                    "the default of `{}` of `gossip {command}` is `{}` in the code and `{}` in \
                     the Reference",
                    flag.name,
                    flag.default,
                    default.cloned().unwrap_or_default()
                ));
            }
        }
    }

    // Keys, place by place.
    for (place, heading) in PLACES {
        let in_code: Vec<String> = KEYS
            .iter()
            .filter(|row| row.place == place)
            .flat_map(|row| row.keys.iter().map(|key| key.to_string()))
            .collect();
        let in_readme: Vec<String> = table_after(reference, heading)
            .iter()
            .flat_map(|row| {
                row[0]
                    .split(", ")
                    .map(str::to_string)
                    .collect::<Vec<String>>()
            })
            .collect();
        compare("key", heading, &in_code, &in_readme, problems);
    }

    // The `:` commands.
    let in_code: Vec<String> = COMMANDS.iter().map(|row| row.form.to_string()).collect();
    let in_readme = first_cells(&table_after(reference, "### `:` commands"));
    compare(
        "`:` command",
        "### `:` commands",
        &in_code,
        &in_readme,
        problems,
    );
}

/// Says what one list has and the other has not.
fn compare(
    what: &str,
    heading: &str,
    in_code: &[String],
    in_readme: &[String],
    problems: &mut Vec<String>,
) {
    for name in in_code {
        if !in_readme.contains(name) {
            problems.push(format!(
                "the {what} `{name}` is in the code and not in the Reference, under `{heading}`"
            ));
        }
    }
    for name in in_readme {
        if !in_code.contains(name) {
            problems.push(format!(
                "the {what} `{name}` is in the Reference, under `{heading}`, and not in the code"
            ));
        }
    }
}

/// The rows of the first table after a heading, or after a line of text:
/// the cells of each, without the code marks around them.
fn table_after(text: &str, heading: &str) -> Vec<Vec<String>> {
    let Some(start) = text.find(heading) else {
        return Vec::new();
    };
    text[start + heading.len()..]
        .lines()
        .skip_while(|line| !line.starts_with('|'))
        .take_while(|line| line.starts_with('|'))
        // The head of the table and the rule under it.
        .skip(2)
        .map(cells)
        .collect()
}

/// The cells of a row of a table. A bar inside a cell is written `\|`.
fn cells(row: &str) -> Vec<String> {
    const BAR: &str = "\u{0}";
    let row = row.replace("\\|", BAR);
    let row = row.trim().trim_start_matches('|').trim_end_matches('|');
    row.split('|')
        .map(|cell| {
            let cell = cell.trim().replace(BAR, "|");
            // A cell that is one piece of code is that code. The keys of a
            // row are several pieces.
            cell.split(", ")
                .map(|piece| {
                    piece
                        .strip_prefix('`')
                        .and_then(|piece| piece.strip_suffix('`'))
                        .filter(|piece| !piece.contains('`'))
                        .unwrap_or(piece)
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .collect()
}

fn first_cells(rows: &[Vec<String>]) -> Vec<String> {
    rows.iter().map(|row| row[0].clone()).collect()
}

/// No name of a real cluster, and no number of an issue or a pull request.
fn check_names(readme: &str, problems: &mut Vec<String>) {
    for name in REAL_NAMES {
        if readme.contains(name) {
            problems.push(format!(
                "`{name}` is a name of a real cluster; the examples of the README are made up: \
                 the namespace `shop` with the deployments `api` and `web`"
            ));
        }
    }
    let characters: Vec<char> = readme.chars().collect();
    for window in characters.windows(3) {
        let before = window[0];
        if window[1] == '#' && window[2].is_ascii_digit() && !before.is_alphanumeric() {
            problems.push(
                "the README names an issue or a pull request by its number, as in `#12`"
                    .to_string(),
            );
            break;
        }
    }
}

// ---- the binary ---------------------------------------------------------------

/// The root of the repository.
pub fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the crate lies two directories below the root of the repository")
}

/// Builds the `gossip` binary and gives its path. The walkthrough is played
/// by the binary, not by the code of this crate.
///
/// The whole workspace is named to cargo, so that the binary is built with
/// the features a build of the workspace gives it and is not built twice.
pub fn build_binary() -> anyhow::Result<PathBuf> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "build",
            "--workspace",
            "--bin",
            "gossip",
            "--message-format=json",
        ])
        .current_dir(repository())
        // A version handed to this build is not one for the README.
        .env_remove("GOSSIP_VERSION")
        .stderr(Stdio::piped())
        .output()
        .context("cargo could not be run to build gossip")?;
    if !output.status.success() {
        bail!(
            "gossip did not build:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let messages = String::from_utf8_lossy(&output.stdout);
    messages
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-artifact")
        .filter(|message| message["target"]["name"] == "gossip")
        .filter_map(|message| message["executable"].as_str().map(PathBuf::from))
        .next_back()
        .context("cargo built gossip and named no executable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_is_found_and_written_between_its_markers() {
        let readme = "a\n<!-- generated:status -->\nold\n<!-- /generated:status -->\nb\n";
        assert_eq!(block(readme, "status"), Some("old"));
        assert_eq!(block(readme, "sample"), None);
        let blocks = [("status".to_string(), "new\nlines".to_string())];
        let written = splice(readme, &blocks).unwrap();
        assert_eq!(
            written,
            "a\n<!-- generated:status -->\nnew\nlines\n<!-- /generated:status -->\nb\n"
        );
        // Writing it again changes nothing.
        assert_eq!(splice(&written, &blocks).unwrap(), written);
        // A block without its place is not written anywhere.
        let blocks = [("sample".to_string(), "x".to_string())];
        assert!(splice(readme, &blocks).is_err());
    }

    #[test]
    fn the_cells_of_a_row() {
        assert_eq!(
            cells("| `--output <text\\|json>` | The rendering. | text |"),
            ["--output <text|json>", "The rendering.", "text"]
        );
        assert_eq!(cells("| `q`, `Ctrl-C` | Quit. |"), ["q, Ctrl-C", "Quit."]);
        assert_eq!(
            cells("| any other key | No. |  |"),
            ["any other key", "No.", ""]
        );
        // Code inside a sentence stays as it is written.
        assert_eq!(
            cells("| `/` | Open it with `grep ` typed. |"),
            ["/", "Open it with `grep ` typed."]
        );
    }

    #[test]
    fn the_install_section_names_the_version_everywhere() {
        let install = install("1.2.3");
        assert!(!install.contains("{{"));
        assert_eq!(install.matches("releases/download/v1.2.3/").count(), 6);
        for archive in [
            "gossip-1.2.3-linux-x64.tar.gz",
            "gossip-1.2.3-linux-arm64.tar.gz",
            "gossip-1.2.3-macos-arm64.tar.gz",
            "gossip-1.2.3-windows-x64.zip",
        ] {
            let link = format!("[{archive}]({REPOSITORY}/releases/download/v1.2.3/{archive})");
            assert!(install.contains(&link), "{link}");
        }
        assert_eq!(status_line("1.2.3"), format!("v1.2.3 · {STATUS}"));
    }
}
