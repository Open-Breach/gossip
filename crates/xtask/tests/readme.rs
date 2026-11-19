//! The README is checked, not trusted (SPEC-release.md 4.5).
//!
//! These tests fail when a `gossip` command, a flag of `follow`, `replay`
//! or `list`, a key or a `:` command is in the code and not in the
//! Reference of the README, or the other way round; when a default in the
//! Reference differs from the code's; when a fixed text of the spec is not
//! in the README word for word; and when a generated block differs from
//! what the generator would write now.
//!
//! The generated blocks are made once for all of them: the walkthrough is
//! played by the built binary against the stand-in API server, which takes
//! a quarter of a minute.

#![cfg(unix)]

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use gossip_core::config::{Config, SessionSpec};
use gossip_core::{Filter, Level, Target};
use xtask::readme::{
    self, BLOCKS, Block, CLOSING_LINKS, KNOWN_LIMITS, STATUS, TAGLINE, VERSION, WARNING,
    WHAT_IT_IS, block, check, splice,
};

fn generate() -> Vec<Block> {
    let binary = readme::build_binary().expect("gossip builds");
    tokio::runtime::Runtime::new()
        .expect("a runtime")
        .block_on(readme::generate(&binary))
        .expect("the generator runs")
}

/// The generated blocks, as the code and the binary make them now.
fn generated() -> &'static [Block] {
    static GENERATED: OnceLock<Vec<Block>> = OnceLock::new();
    GENERATED.get_or_init(generate)
}

fn generated_block(name: &str) -> &'static str {
    let (_, content) = generated()
        .iter()
        .find(|(known, _)| known == name)
        .unwrap_or_else(|| panic!("no generated block `{name}`"));
    content
}

/// The README of the repository.
fn repository_readme() -> String {
    std::fs::read_to_string(readme::repository().join("README.md")).expect("README.md is there")
}

/// A README that is true: the fixed texts of the spec in their order, and
/// every block as the generator writes it now.
fn a_true_readme() -> String {
    let mut text = format!("# Gossip\n\n*{TAGLINE}*\n\n{WHAT_IT_IS}\n\n");
    let place = |text: &mut String, name: &str| {
        text.push_str(&format!(
            "<!-- generated:{name} -->\n<!-- /generated:{name} -->\n\n"
        ));
    };
    place(&mut text, "status");
    place(&mut text, "sample");
    text.push_str(&format!("> {WARNING}\n\n## Install\n\n"));
    place(&mut text, "install");
    text.push_str("## Using Gossip\n\nOne session.\n\n");
    for name in BLOCKS.iter().filter(|name| name.starts_with("walk-")) {
        text.push_str("### A step\n\nA sentence.\n\n");
        place(&mut text, name);
    }
    text.push_str("## Reference\n\n");
    place(&mut text, "reference");
    text.push_str("## Known limits\n\n");
    for (index, limit) in KNOWN_LIMITS.iter().enumerate() {
        // The second says in its own words how the limit is raised.
        let end = if index == 1 { "; raise it." } else { "" };
        text.push_str(&format!("- {limit}{end}\n"));
    }
    let links: Vec<String> = CLOSING_LINKS[..4]
        .iter()
        .map(|link| format!("[a link]{link}"))
        .collect();
    text.push_str(&format!(
        "\n{}\n\nApache-2.0, see [LICENSE]{}.\n",
        links.join(" · "),
        CLOSING_LINKS[4]
    ));
    splice(&text, generated()).expect("every block has its place")
}

/// The problems the check finds in a README.
fn problems(readme: &str) -> Vec<String> {
    check(readme, generated())
}

/// Asserts that the check finds a problem with this text in it.
#[track_caller]
fn assert_problem(readme: &str, wanted: &str) {
    let problems = problems(readme);
    assert!(
        problems.iter().any(|problem| problem.contains(wanted)),
        "no problem with `{wanted}` among:\n{}",
        problems.join("\n")
    );
}

/// A true README with one text replaced by another.
#[track_caller]
fn with(old: &str, new: &str) -> String {
    let readme = a_true_readme();
    assert!(readme.contains(old), "the README has no `{old}`");
    readme.replacen(old, new, 1)
}

/// A true README without the line that starts with this text.
#[track_caller]
fn without_line(start: &str) -> String {
    let readme = a_true_readme();
    let lines: Vec<&str> = readme.lines().collect();
    assert!(
        lines.iter().any(|line| line.starts_with(start)),
        "the README has no line `{start}`"
    );
    let kept: Vec<&str> = lines
        .into_iter()
        .filter(|line| !line.starts_with(start))
        .collect();
    kept.join("\n") + "\n"
}

// ---- the README of the repository --------------------------------------------

#[test]
fn the_readme_of_the_repository_is_what_the_code_and_the_spec_say() {
    let problems = problems(&repository_readme());
    assert!(problems.is_empty(), "README.md:\n{}", problems.join("\n\n"));
}

#[test]
fn the_generator_writes_the_same_twice() {
    // SPEC-release.md, acceptance 8: `cargo xtask readme` run twice on the
    // same commit changes nothing the second time.
    let again = generate();
    for ((name, first), (_, second)) in generated().iter().zip(&again) {
        assert_eq!(first, second, "the block `{name}`");
    }
    let readme = a_true_readme();
    assert_eq!(splice(&readme, &again).unwrap(), readme);
}

// ---- what the check catches ----------------------------------------------------

#[test]
fn a_true_readme_has_no_problem() {
    assert_eq!(problems(&a_true_readme()), Vec::<String>::new());
}

#[test]
fn a_flag_that_is_missing_from_the_reference_is_a_problem() {
    // SPEC-release.md, acceptance 7: removing a row from the Reference's
    // flag table fails `cargo test`. It is also what a flag added to the
    // code without regenerating looks like: in the code, not in the README.
    let readme = without_line("| `--since <duration>` |");
    assert_problem(
        &readme,
        "the flag of `gossip follow` `--since <duration>` is in the code and not in the Reference",
    );
    assert_problem(
        &readme,
        "the generated block `reference` is not what the generator writes now",
    );

    let readme = without_line("| `--speed <factor>` |");
    assert_problem(
        &readme,
        "the flag of `gossip replay` `--speed <factor>` is in the code",
    );
    let readme = without_line("| `--json` |");
    assert_problem(&readme, "the flag of `gossip list` `--json` is in the code");
}

#[test]
fn a_flag_the_code_does_not_have_is_a_problem() {
    let readme = with(
        "| `--json` |",
        "| `--yaml` | Print YAML instead of a table. | off |\n| `--json` |",
    );
    assert_problem(
        &readme,
        "the flag of `gossip list` `--yaml` is in the Reference, under `#### `gossip list``, and not in the code",
    );
}

#[test]
fn a_default_that_differs_from_the_codes_is_a_problem() {
    let readme = with("| 20, unless --since is given |", "| 50 |");
    assert_problem(
        &readme,
        "the default of `--tail <n>` of `gossip follow` is `20, unless --since is given` in the code and `50` in the Reference",
    );
    // A switch is off by default, and a flag the code gives a default has
    // that one.
    let readme = with(
        "| Leave the cluster's events out of the stream. | off |",
        "| Leave the cluster's events out of the stream. | on |",
    );
    assert_problem(
        &readme,
        "the default of `--no-events` of `gossip follow` is `off` in the code",
    );
    let readme = with(
        "and 0 is as fast as the terminal takes it. | 0 |",
        "and 0 is as fast as the terminal takes it. | 1 |",
    );
    assert_problem(
        &readme,
        "the default of `--speed <factor>` of `gossip replay` is `0` in the code and `1`",
    );
}

#[test]
fn a_command_a_key_or_a_colon_command_that_is_missing_or_made_up_is_a_problem() {
    let readme = without_line("| `gossip replay <file>` |");
    assert_problem(
        &readme,
        "the command `gossip replay <file>` is in the code and not in the Reference",
    );
    let readme = with(
        "| `gossip templates` |",
        "| `gossip doctor` | Check the cluster. |\n| `gossip templates` |",
    );
    assert_problem(&readme, "the command `gossip doctor` is in the Reference");

    let readme = without_line("| `Ctrl-W` |");
    assert_problem(
        &readme,
        "the key `Ctrl-W` is in the code and not in the Reference",
    );
    let readme = with(
        "| `q`, `Ctrl-C` | Quit. |",
        "| `q`, `Ctrl-C`, `x` | Quit. |",
    );
    assert_problem(&readme, "the key `x` is in the Reference");
    // A key that has its row in one place is still missed in another.
    let readme = with("| `?` | List the keys and the commands. |\n", "");
    assert_problem(
        &readme,
        "the key `?` is in the code and not in the Reference, under `While the stream scrolls:`",
    );

    let readme = without_line("| `:targets` |");
    assert_problem(
        &readme,
        "the `:` command `:targets` is in the code and not in the Reference",
    );
    let readme = with("| `:targets` |", "| `:undo` | Undo. |\n| `:targets` |");
    assert_problem(&readme, "the `:` command `:undo` is in the Reference");
}

#[test]
fn a_fixed_text_that_is_reworded_is_a_problem() {
    let readme = with("says behind your back.", "says about you.");
    assert_problem(&readme, "the tagline is not in the README word for word");
    let readme = with(
        "add a service, save the session.",
        "add a service and save it.",
    );
    assert_problem(
        &readme,
        "the paragraph of what Gossip is is not in the README word for word",
    );
    let readme = with(
        "Until 1.0, treat every release as a preview.",
        "It is a preview.",
    );
    assert_problem(&readme, "the warning is not in the README word for word");
    let readme = with(
        "- One cluster at a time.",
        "- One cluster at a time, for now.",
    );
    assert_problem(&readme, "a known limit is not in the README word for word");
    let readme = without_line("- The kubelet polls");
    assert_problem(&readme, "a known limit is not in the README word for word");

    // The status text is in a generated block, where the generator would
    // put it back; a README that carries other words there is caught both
    // ways.
    let readme = with("so expect bugs. Please", "and has bugs. Please");
    assert_problem(
        &readme,
        "the status text is not in the README word for word",
    );
    assert_problem(
        &readme,
        "the generated block `status` is not what the generator writes now",
    );
}

#[test]
fn a_part_that_is_out_of_its_place_or_one_too_many_is_a_problem() {
    // The Reference before the walkthrough.
    let readme = a_true_readme();
    let walk = readme.find("## Using Gossip").unwrap();
    let reference = readme.find("## Reference").unwrap();
    let limits = readme.find("## Known limits").unwrap();
    let swapped = format!(
        "{}{}{}{}",
        &readme[..walk],
        &readme[reference..limits],
        &readme[walk..reference],
        &readme[limits..]
    );
    assert_problem(&swapped, "is out of its place in the README");

    // The warning after the Install section.
    let warning = format!("> {WARNING}\n\n");
    let moved = readme.replacen(&warning, "", 1).replacen(
        "## Using Gossip",
        &format!("{warning}## Using Gossip"),
        1,
    );
    // What comes after it is then what stands before it: the order is
    // read from the top.
    assert_problem(&moved, "the Install section is out of its place");

    let readme = with(
        "## Known limits",
        "## Status\n\nA prototype.\n\n## Known limits",
    );
    assert_problem(
        &readme,
        "`## Status` is a section the structure does not have",
    );
    let readme = with("## Known limits", "## Limits");
    assert_problem(&readme, "the README lacks the known limits");
    let readme = with("(docs/development.md)", "(docs/building.md)");
    assert_problem(
        &readme,
        "the README lacks a link of the closing lines: (docs/development.md)",
    );
}

#[test]
fn a_generated_block_that_is_stale_missing_or_unknown_is_a_problem() {
    // A line of the walkthrough that the binary does not print.
    let readme = with(
        "-- pod api-6c9f7b8d54-x7k2p went away",
        "-- pod api-6c9f7b8d54-x7k2p is gone",
    );
    assert_problem(
        &readme,
        "the generated block `walk-restart` is not what the generator writes now; run `cargo xtask readme`",
    );
    assert_problem(
        &readme,
        "generated now:  14:02:08.000 -- pod api-6c9f7b8d54-x7k2p went away",
    );

    // The version of another release.
    let readme = with(&format!("v{VERSION} · "), "v9.9.9 · ");
    assert_problem(
        &readme,
        "the generated block `status` is not what the generator writes now",
    );
    let readme = with(
        &format!("releases/download/v{VERSION}/gossip-{VERSION}-linux-x64.tar.gz)"),
        "releases/download/v9.9.9/gossip-9.9.9-linux-x64.tar.gz)",
    );
    assert_problem(
        &readme,
        "the generated block `install` is not what the generator writes now",
    );

    let readme = a_true_readme()
        .replace("<!-- generated:sample -->", "")
        .replace("<!-- /generated:sample -->", "");
    assert_problem(&readme, "the README has no generated block `sample`");
    let readme = with(
        "## Known limits",
        "<!-- generated:roadmap -->\n<!-- /generated:roadmap -->\n\n## Known limits",
    );
    assert_problem(
        &readme,
        "a generated block `roadmap` that the generator does not write",
    );
}

#[test]
fn a_name_of_a_real_cluster_or_a_number_of_a_pull_request_is_a_problem() {
    let readme = with("One session.", "One session, against fc-stg.");
    assert_problem(&readme, "`fc-stg` is a name of a real cluster");
    let readme = with("One session.", "One session (see #12).");
    assert_problem(&readme, "names an issue or a pull request by its number");
    // An anchor and a status code are neither.
    assert!(a_true_readme().contains("(#targets)"));
}

// ---- what the generated parts say ------------------------------------------------

#[test]
fn the_status_line_and_the_install_section_name_the_version_of_cargo_toml() {
    assert_eq!(generated_block("status"), format!("v{VERSION} · {STATUS}"));
    let install = generated_block("install");
    for platform in [
        "linux-x64.tar.gz",
        "linux-arm64.tar.gz",
        "macos-arm64.tar.gz",
        "windows-x64.zip",
    ] {
        let link = format!(
            "https://github.com/Open-Breach/gossip/releases/download/v{VERSION}/gossip-{VERSION}-{platform}"
        );
        assert!(install.contains(&link), "{link}");
    }
}

#[test]
fn the_install_steps_of_the_readme_are_the_ones_the_smoke_test_tries() {
    // Every line of a code block of the Install section, from the archive
    // on, is a line of the smoke test of the release workflow, with the
    // version and the platform as the workflow writes them. The download is
    // not: before the release there is nothing to download.
    let workflow =
        std::fs::read_to_string(readme::repository().join(".github/workflows/release.yml"))
            .unwrap();
    let install = generated_block("install");
    let source = install.find("**From source**").unwrap();
    let mut tried = 0;
    let mut inside = false;
    for line in install[..source].lines() {
        if line.starts_with("```") {
            inside = !inside;
            continue;
        }
        if !inside || line.starts_with("curl ") {
            continue;
        }
        let in_the_workflow = line
            .replace(
                &format!("gossip-{VERSION}-linux-x64"),
                "gossip-$version-${{ matrix.platform }}",
            )
            .replace(
                &format!("gossip-{VERSION}-macos-arm64"),
                "gossip-$version-${{ matrix.platform }}",
            )
            .replace(
                &format!("gossip-{VERSION}-windows-x64"),
                "gossip-$version-windows-x64",
            );
        assert!(
            workflow.contains(&format!("          {in_the_workflow}\n")),
            "the smoke test does not try `{line}`: no line `{in_the_workflow}` in release.yml"
        );
        tried += 1;
    }
    // Two lines for Linux, two for macOS, two for Windows.
    assert_eq!(tried, 6);
}

#[test]
fn the_sample_shows_a_line_a_pod_being_killed_and_the_new_pod_starting() {
    let sample = generated_block("sample");
    let lines: Vec<&str> = sample.lines().collect();
    assert_eq!(lines.first(), Some(&"```text"));
    assert_eq!(lines.last(), Some(&"```"));
    let lines = &lines[1..lines.len() - 1];
    assert!((4..=5).contains(&lines.len()), "{sample}");
    assert!(lines[0].contains("request finished"));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("💀") && line.contains("Killing"))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("🚀") && line.contains("Started"))
    );
    assert!(lines[lines.len() - 1].contains("api-6c9f7b8d54-r9t2v api"));
}

#[test]
fn the_walkthrough_runs_the_commands_of_the_spec_in_their_order() {
    let step = |name: &str| generated_block(name);
    let has = |name: &str, text: &str| {
        assert!(
            step(name).contains(text),
            "`{text}` in {name}:\n{}",
            step(name)
        );
    };
    // 1. Find your way.
    has("walk-find", "$ gossip list contexts\n");
    has("walk-find", "$ gossip list namespaces\n");
    has("walk-find", "$ gossip list shop\n");
    // 2. Follow one deployment.
    has("walk-follow", "$ gossip follow shop/api\n");
    // 3. A restart: the events with their emoji, the notices, the first
    // line of the new pod, and no key pressed.
    for text in [
        "💀",
        "📍",
        "📦",
        "🚀",
        "-- pod api-6c9f7b8d54-r9t2v appeared",
        "went away",
        "listening",
    ] {
        has("walk-restart", text);
    }
    assert!(!step("walk-restart").contains("\n:"));
    // 4. Narrow it.
    has("walk-narrow", "\n:level info\n");
    has("walk-narrow", "\n:filter\n");
    assert!(!step("walk-narrow").contains("DEBUG"));
    // 5. Stop to read.
    has(
        "walk-pause",
        "paused · 0 lines waiting\npaused · 3 lines waiting\n",
    );
    // 6. Add a second service.
    has("walk-add", "\n:add shop/web\n");
    has("walk-add", "\n:targets\n");
    has("walk-add", "shop/web: 1 pod");
    // 7. Keep it.
    has("walk-save", "\n:save shop\n");
    has("walk-save", "saved as the template shop in ");
    // 8. Come back.
    has("walk-back", "$ gossip follow --last\n");
    has("walk-back", "$ gossip follow --template shop\n");
    // 9. Keep a record.
    has("walk-record", "--record api.jsonl\n");
    has("walk-record", "$ gossip replay api.jsonl\n");
    // The recording holds what the filter hid from the screen.
    has("walk-record", "DEBUG");

    // Nothing of the machine it ran on, and no colour.
    for (name, content) in generated() {
        assert!(!content.contains('\u{1b}'), "an escape code in {name}");
        assert!(!content.contains("127.0.0.1"), "an address in {name}");
        assert!(
            !content.lines().any(|line| line.ends_with(' ')),
            "a space at the end of a line in {name}"
        );
    }
}

// ---- the prose of the Reference is true of the code ------------------------------

/// The cells of the first column of the table after a line of the
/// Reference, without their code marks.
fn first_column(after: &str) -> Vec<String> {
    let reference = generated_block("reference");
    let start = reference
        .find(after)
        .unwrap_or_else(|| panic!("no `{after}`"));
    reference[start + after.len()..]
        .lines()
        .skip_while(|line| !line.starts_with('|'))
        .take_while(|line| line.starts_with('|'))
        .skip(2)
        .map(|row| row.split('|').nth(1).unwrap().trim().to_string())
        .collect()
}

fn code_pieces(cell: &str) -> Vec<String> {
    cell.split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

#[test]
fn every_target_of_the_reference_is_one_the_program_reads() {
    let targets: Vec<String> = first_column("### Targets")
        .iter()
        .flat_map(|cell| code_pieces(cell))
        .collect();
    assert_eq!(targets.len(), 8);
    for target in targets {
        target
            .parse::<Target>()
            .unwrap_or_else(|error| panic!("`{target}`: {error}"));
    }
}

/// The TOML blocks of the Reference.
fn toml_blocks() -> Vec<String> {
    generated_block("reference")
        .split("```toml\n")
        .skip(1)
        .map(|rest| rest.split("```").next().unwrap().to_string())
        .collect()
}

#[test]
fn the_config_example_of_the_reference_is_a_config_that_uses_every_key() {
    let example = toml_blocks()
        .into_iter()
        .find(|block| block.starts_with("pause_buffer"))
        .expect("the example starts with pause_buffer");
    let config: Config = toml::from_str(&example).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(config.pause_buffer, Some(10_000));
    assert_eq!(config.templates.len(), 2);

    // Every key a template has is in the table of keys, and the other way
    // round. A new key of `SessionSpec` does not compile here until it is
    // named, and then wants its row.
    let every_key = SessionSpec {
        context: Some("demo".to_string()),
        targets: vec!["shop/api".parse().unwrap()],
        container: Some("api".parse().unwrap()),
        since: Some(std::time::Duration::from_secs(300)),
        tail: Some(20),
        events: Some(true),
        format: config.templates["api-json"].format.clone(),
        filter: Some(Filter::level_at_least(Level::Info)),
        fields: Some(vec!["tenant_id".to_string()]),
        output: config.templates["api-json"].output,
        columns: config.templates["api-json"].columns,
    };
    let serialized = serde_json::to_value(&every_key).unwrap();
    let mut in_code: Vec<&str> = serialized
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(in_code.len(), 11, "a key was left out when it was written");
    let mut in_reference: Vec<String> = first_column("The keys of a template")
        .iter()
        .flat_map(|cell| code_pieces(cell))
        .collect();
    in_code.sort_unstable();
    in_reference.sort_unstable();
    assert_eq!(in_reference, in_code);

    // And the example uses each of them somewhere.
    for key in in_code {
        assert!(
            example
                .lines()
                .any(|line| line.starts_with(&format!("{key} = "))),
            "the example does not use `{key}`"
        );
    }
}

#[test]
fn the_filters_and_the_columns_of_the_reference_are_read_by_the_program() {
    let blocks = toml_blocks();
    // The filters of the section Filters, one per `filter = `.
    let filters = blocks
        .iter()
        .find(|block| block.starts_with("filter = "))
        .expect("the filters");
    let filters: Vec<String> = filters
        .split("filter = ")
        .skip(1)
        .map(|filter| format!("filter = {filter}"))
        .collect();
    assert_eq!(filters.len(), 3);
    for filter in filters {
        toml::from_str::<SessionSpec>(&filter).unwrap_or_else(|error| panic!("{filter}: {error}"));
    }
    // The `[columns]` of the section Columns.
    let columns = blocks
        .iter()
        .find(|block| block.starts_with("[columns]"))
        .expect("the columns");
    toml::from_str::<Config>(columns).unwrap_or_else(|error| panic!("{error}"));

    // The fields and the operators of a rule.
    for field in first_column("A rule compares a field of a record with a value.")
        .iter()
        .flat_map(|cell| code_pieces(cell))
    {
        let field = field.replace("<key>", "tenant_id");
        field
            .parse::<gossip_core::Field>()
            .unwrap_or_else(|error| panic!("`{field}`: {error}"));
    }
    let operators: Vec<String> = first_column("| `f.<key>` |")
        .iter()
        .flat_map(|cell| code_pieces(cell))
        .collect();
    assert_eq!(
        operators,
        ["==", "!=", ">=", "contains", "matches", "exists"]
    );
    for operator in operators {
        operator
            .parse::<gossip_core::Op>()
            .unwrap_or_else(|error| panic!("`{operator}`: {error}"));
    }
}

#[test]
fn the_defaults_the_help_states_are_the_defaults_of_the_code() {
    // The Reference takes the default of a flag from its help. For the ones
    // that are a number or a list in the code, the help is held against it.
    let flags = xtask::readme::reference::flags("follow");
    let default = |name: &str| {
        flags
            .iter()
            .find(|flag| flag.name == name)
            .unwrap_or_else(|| panic!("no flag `{name}`"))
            .default
            .clone()
    };
    assert_eq!(
        default("--tail <n>"),
        format!(
            "{}, unless --since is given",
            gossip_core::config::DEFAULT_TAIL
        )
    );
    assert_eq!(
        default("--columns <columns>"),
        xtask::readme::reference::columns_default()
    );
    assert_eq!(default("--output <text|json>"), "text");
    assert_eq!(
        Config::default().base().resolve().output,
        gossip_core::Output::Text
    );
}

// ---- the check the release workflow runs first -----------------------------------

/// Runs `.github/scripts/check-readme.sh`; its exit code and what it said.
fn check_readme(tag: &str, readme: &Path) -> (i32, String) {
    let output = Command::new("bash")
        .arg(readme::repository().join(".github/scripts/check-readme.sh"))
        .arg(tag)
        .arg(readme)
        .output()
        .expect("bash runs");
    let mut text = String::from_utf8(output.stdout).unwrap();
    text.push_str(&String::from_utf8(output.stderr).unwrap());
    (output.status.code().expect("an exit code"), text)
}

fn readme_file(test: &str, text: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "gossip-check-readme-{}-{test}.md",
        std::process::id()
    ));
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn the_release_check_passes_a_readme_of_the_tags_version_and_no_other() {
    // SPEC-release.md, acceptance 5: a tag whose version differs from the
    // README's status line fails in the first job, before any build.
    let path = readme_file("true", &a_true_readme());
    let (code, text) = check_readme(&format!("v{VERSION}"), &path);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains(&format!("describes v{VERSION}")), "{text}");
    // The dry run of the workflow is a pre-release of the same version.
    assert_eq!(check_readme(&format!("v{VERSION}-rc1"), &path).0, 0);

    let (code, text) = check_readme("v9.9.9", &path);
    assert_eq!(code, 1, "{text}");
    assert!(
        text.contains("describes another release than v9.9.9"),
        "{text}"
    );
    assert!(text.contains("cargo xtask readme"), "{text}");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn the_release_check_fails_on_a_link_to_an_archive_of_another_release() {
    let stale = a_true_readme().replacen(
        &format!("releases/download/v{VERSION}/gossip-{VERSION}-macos-arm64"),
        "releases/download/v0.0.9/gossip-0.0.9-macos-arm64",
        1,
    );
    let path = readme_file("link", &stale);
    let (code, text) = check_readme(&format!("v{VERSION}"), &path);
    assert_eq!(code, 1, "{text}");
    assert!(
        text.contains("links to an archive of another release")
            && text.contains("v0.0.9/gossip-0.0.9-macos-arm64"),
        "{text}"
    );
    std::fs::remove_file(path).unwrap();

    // A README without a status line, or without the Install links, is not
    // the README of any release.
    let path = readme_file("none", "# Gossip\n\nA prototype.\n");
    let (code, text) = check_readme(&format!("v{VERSION}"), &path);
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("(missing)"), "{text}");
    let without_links = a_true_readme().replace("releases/download/", "releases/");
    std::fs::write(&path, without_links).unwrap();
    let (code, text) = check_readme(&format!("v{VERSION}"), &path);
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("has no link to an archive"), "{text}");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn the_readme_of_the_repository_passes_the_release_check_for_its_version() {
    let path = readme::repository().join("README.md");
    let (code, text) = check_readme(&format!("v{VERSION}"), &path);
    assert_eq!(code, 0, "{text}");
    assert_eq!(
        block(&repository_readme(), "status"),
        Some(generated_block("status"))
    );
}
