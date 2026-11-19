//! The Reference of the README: its tables come from the code, its prose
//! from `reference.md` beside this file.
//!
//! The commands and the flags are the ones `clap` reads, with the help and
//! the default each carries there. The keys, the `:` commands, the fields
//! and the emoji are the tables of the CLI crate; the columns, the history
//! and the pause buffer are the defaults of the core.

use gossip::reference::{
    ALL_FIELDS, ANY_CHARACTER, ANY_OTHER_KEY, COMMANDS, EVENT_EMOJI, EVENT_TAG, KEYS, KNOWN_FIELDS,
    OTHER_EMOJI, Place, WARNING_EMOJI, Where, command_line,
};
use gossip_core::config::{DEFAULT_PAUSE_BUFFER, DEFAULT_TAIL};
use gossip_core::{Columns, FormatChoice};

/// The prose of the Reference, with a `{{name}}` where a table goes.
const TEMPLATE: &str = include_str!("reference.md");

/// The name of the program, and of the command `clap` adds to it.
const PROGRAM: &str = "gossip";
const HELP: &str = "help";

/// What the help of a flag says for the reader of `--help`, and what the
/// Reference says in its place.
const IN_THE_README: [(&str, &str); 1] = [("see the forms below", "see [Targets](#targets)")];

/// The commands whose flags the Reference lists.
pub const FLAG_COMMANDS: [&str; 3] = ["follow", "replay", "list"];

/// The Reference, as it stands under its heading in the README.
pub fn reference() -> String {
    let mut text = TEMPLATE.trim_end().to_string();
    let mut fill = |name: &str, with: String| {
        let place = format!("{{{{{name}}}}}");
        assert!(
            text.contains(&place),
            "reference.md has no place for {place}"
        );
        text = text.replace(&place, with.trim_end());
    };
    fill("commands", commands_table());
    for command in FLAG_COMMANDS {
        fill(&format!("flags-{command}"), flags_table(command));
    }
    fill("keys", keys_tables());
    fill("colon-commands", colon_commands_table());
    fill("fields-first", code_list(&fields(Place::First)));
    fill("fields-hidden", code_list(&fields(Place::Hidden)));
    fill("fields-all", ALL_FIELDS.to_string());
    fill("columns", columns_table());
    fill("columns-default", columns_default());
    fill("pause-buffer", with_thousands(DEFAULT_PAUSE_BUFFER));
    fill("tail", DEFAULT_TAIL.to_string());
    fill("formats", formats());
    fill("events", events_table());
    fill("event-tag", EVENT_TAG.to_string());
    assert!(
        !text.contains("{{"),
        "reference.md has a place nothing fills: {}",
        text.split("{{").nth(1).unwrap_or_default()
    );
    text
}

/// A command of the program as the Reference lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLine {
    /// How it is typed: `gossip templates show <name>`.
    pub usage: String,
    pub does: String,
}

/// Every command and subcommand of the program.
pub fn commands() -> Vec<CommandLine> {
    let mut root = command_line();
    root.build();
    let mut lines = Vec::new();
    collect(&root, root.get_name(), &mut lines);
    lines
}

fn collect(command: &clap::Command, path: &str, lines: &mut Vec<CommandLine>) {
    for sub in command.get_subcommands() {
        let path = format!("{path} {}", sub.get_name());
        let does = sub
            .get_about()
            .map(|about| sentence(&about.to_string()))
            .unwrap_or_default();
        if sub.get_name() == HELP {
            // The `help` that clap adds: once for the program is enough,
            // and it takes the command to explain.
            if command.get_name() == PROGRAM {
                let usage = format!("{path} [command]");
                lines.push(CommandLine { usage, does });
            }
            continue;
        }
        let mut usage = path.clone();
        for argument in sub.get_positionals() {
            usage.push(' ');
            usage.push_str(&positional(argument));
        }
        lines.push(CommandLine { usage, does });
        collect(sub, &path, lines);
    }
}

fn commands_table() -> String {
    let rows: Vec<[String; 2]> = commands()
        .into_iter()
        .map(|command| [code(&command.usage), command.does])
        .collect();
    table(["Command", "Does"], &rows)
}

/// A positional argument as it is typed: `<target>...`.
fn positional(argument: &clap::Arg) -> String {
    let name = value_name(argument);
    let many = argument
        .get_num_args()
        .is_some_and(|range| range.max_values() > 1);
    let name = if many {
        format!("<{name}>...")
    } else {
        format!("<{name}>")
    };
    if argument.is_required_set() || many {
        name
    } else {
        format!("[{name}]")
    }
}

/// The name of an argument's value, in lower case: `duration`.
fn value_name(argument: &clap::Arg) -> String {
    argument
        .get_value_names()
        .and_then(|names| names.first())
        .map(|name| name.to_string().to_lowercase())
        .unwrap_or_else(|| argument.get_id().to_string())
}

/// A flag, or a positional argument, as the Reference lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flag {
    /// As it is typed: `--since <duration>`, `<target>...`.
    pub name: String,
    pub meaning: String,
    /// The default the code states; empty when it states none.
    pub default: String,
}

/// The arguments and flags of a command of the program, in the order of
/// its help.
pub fn flags(command: &str) -> Vec<Flag> {
    let mut root = command_line();
    root.build();
    let command = root
        .find_subcommand(command)
        .unwrap_or_else(|| panic!("gossip has no command `{command}`"));
    command
        .get_arguments()
        // What clap adds itself is said once, under Commands.
        .filter(|argument| !["help", "version"].contains(&argument.get_id().as_str()))
        .filter(|argument| !argument.is_hide_set())
        .map(flag)
        .collect()
}

fn flag(argument: &clap::Arg) -> Flag {
    let switch = !argument.get_action().takes_values();
    let name = match argument.get_long() {
        None => positional(argument),
        Some(long) if switch => format!("--{long}"),
        Some(long) => format!("--{long} <{}>", value_name(argument)),
    };
    let help = argument
        .get_help()
        .map(|help| help.to_string())
        .unwrap_or_default();
    let (meaning, stated) = split_default(&help);
    let given: Vec<String> = argument
        .get_default_values()
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    let default = match stated {
        Some(stated) => stated,
        None if switch => "off".to_string(),
        None => given.join(","),
    };
    let mut meaning = sentence(&meaning);
    for (in_the_help, in_the_readme) in IN_THE_README {
        meaning = meaning.replace(in_the_help, in_the_readme);
    }
    Flag {
        name,
        meaning,
        default,
    }
}

/// The help of a flag without the default it states at its end, and that
/// default: `... [default: the current context].`
fn split_default(help: &str) -> (String, Option<String>) {
    const OPEN: &str = "[default: ";
    let Some(start) = help.rfind(OPEN) else {
        return (help.to_string(), None);
    };
    let rest = &help[start + OPEN.len()..];
    let Some(end) = rest.rfind(']') else {
        return (help.to_string(), None);
    };
    let meaning = help[..start].trim_end().to_string();
    (meaning, Some(rest[..end].to_string()))
}

/// A text as one sentence: on one line, with a full stop.
fn sentence(text: &str) -> String {
    let mut text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if !text.ends_with(['.', ':', '?', '!']) {
        text.push('.');
    }
    text
}

fn flags_table(command: &str) -> String {
    let rows: Vec<[String; 3]> = flags(command)
        .into_iter()
        .map(|flag| [code(&flag.name), flag.meaning, flag.default])
        .collect();
    table(["Flag", "Meaning", "Default"], &rows)
}

/// The places a key has a meaning in, with the heading of each.
pub const PLACES: [(Where, &str); 3] = [
    (Where::Stream, "While the stream scrolls:"),
    (Where::CommandLine, "While the command line is open:"),
    (
        Where::Question,
        "While a question waits on the status line, as `:save` asks one:",
    ),
];

fn keys_tables() -> String {
    let mut text = String::new();
    for (place, heading) in PLACES {
        let rows: Vec<[String; 2]> = KEYS
            .iter()
            .filter(|row| row.place == place)
            .map(|row| [key_names(row.keys), row.does.to_string()])
            .collect();
        text.push_str(heading);
        text.push_str("\n\n");
        text.push_str(&table(["Key", "Does"], &rows));
        text.push('\n');
    }
    text
}

/// The keys of a row: each as code, the rows that stand for the rest as
/// words.
fn key_names(keys: &[&str]) -> String {
    keys.iter()
        .map(|key| {
            if [ANY_OTHER_KEY, ANY_CHARACTER].contains(key) {
                key.to_string()
            } else {
                code(key)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn colon_commands_table() -> String {
    let rows: Vec<[String; 2]> = COMMANDS
        .iter()
        .map(|row| [code(row.form), row.does.to_string()])
        .collect();
    table(["Command", "Does"], &rows)
}

/// The fields the output knows by name that have this place.
pub fn fields(place: Place) -> Vec<&'static str> {
    KNOWN_FIELDS
        .iter()
        .filter(|(_, known)| *known == place)
        .map(|(name, _)| *name)
        .collect()
}

/// What each column holds, in the order of the line.
const COLUMNS: [(&str, &str); 4] = [
    ("time", "`HH:MM:SS.mmm`, local zone"),
    (
        "pod",
        "the full pod name, or what an event is about, as in `deployment/api`",
    ),
    (
        "container",
        "the container name; the emoji or `EVNT` for an event",
    ),
    ("level", "`TRACE` to `ERROR`; empty for events"),
];

fn columns_table() -> String {
    let defaults = Columns::default();
    let rows: Vec<[String; 4]> = COLUMNS
        .iter()
        .map(|(name, holds)| {
            let column = match *name {
                "time" => defaults.time,
                "pod" => defaults.pod,
                "container" => defaults.container,
                _ => defaults.level,
            };
            [
                code(name),
                column.width.to_string(),
                code(column.cut.as_str()),
                holds.to_string(),
            ]
        })
        .collect();
    table(["Column", "Width", "Cut", "Holds"], &rows)
}

/// The default columns, written as `--columns` takes them.
pub fn columns_default() -> String {
    let defaults = Columns::default();
    [
        ("time", defaults.time),
        ("pod", defaults.pod),
        ("container", defaults.container),
        ("level", defaults.level),
    ]
    .iter()
    .map(|(name, column)| format!("{name}={}:{}", column.width, column.cut))
    .collect::<Vec<_>>()
    .join(",")
}

/// The values of the `format` key, as TOML writes them.
fn formats() -> String {
    let choices = [
        FormatChoice::Auto,
        FormatChoice::Json,
        FormatChoice::Logfmt,
        FormatChoice::Text,
    ];
    let names: Vec<String> = choices
        .iter()
        .map(|choice| {
            let name = serde_json::to_string(choice).expect("a format has a name");
            code(&name)
        })
        .collect();
    match names.split_last() {
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

fn events_table() -> String {
    let mut rows: Vec<[String; 2]> = EVENT_EMOJI
        .iter()
        .map(|(reasons, emoji)| [emoji.to_string(), code_list(reasons)])
        .collect();
    rows.push([
        WARNING_EMOJI.to_string(),
        "`Unhealthy`, and every other warning".to_string(),
    ]);
    rows.push([OTHER_EMOJI.to_string(), "any other reason".to_string()]);
    table(["Emoji", "Reasons"], &rows)
}

/// A Markdown table.
fn table<const N: usize>(head: [&str; N], rows: &[[String; N]]) -> String {
    let mut text = format!("| {} |\n", head.join(" | "));
    text.push_str(&format!("|{}\n", " --- |".repeat(N)));
    for row in rows {
        let cells: Vec<String> = row.iter().map(|cell| cell.replace('|', "\\|")).collect();
        text.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
    text
}

fn code(text: &str) -> String {
    format!("`{text}`")
}

fn code_list(names: &[&str]) -> String {
    names
        .iter()
        .map(|name| code(name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A number with its thousands apart: `10,000`.
fn with_thousands(number: usize) -> String {
    let digits = number.to_string();
    let mut text = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stated_default_is_taken_out_of_the_help() {
        assert_eq!(
            split_default("The kubeconfig context of the cluster [default: the current context]."),
            (
                "The kubeconfig context of the cluster".to_string(),
                Some("the current context".to_string())
            )
        );
        assert_eq!(
            split_default("Print JSON instead of a table."),
            ("Print JSON instead of a table.".to_string(), None)
        );
        // The default may hold brackets of its own kind.
        assert_eq!(
            split_default("Cut [default: a [b] c]"),
            ("Cut".to_string(), Some("a [b] c".to_string()))
        );
    }

    #[test]
    fn a_table_escapes_the_bar() {
        let rows = [["`--output <text|json>`".to_string(), "x".to_string()]];
        assert_eq!(
            table(["Flag", "Meaning"], &rows),
            "| Flag | Meaning |\n| --- | --- |\n| `--output <text\\|json>` | x |\n"
        );
    }

    #[test]
    fn numbers_and_sentences() {
        assert_eq!(with_thousands(10_000), "10,000");
        assert_eq!(with_thousands(999), "999");
        assert_eq!(with_thousands(1_234_567), "1,234,567");
        assert_eq!(sentence("two\n    lines"), "two lines.");
        assert_eq!(sentence("done."), "done.");
    }

    #[test]
    fn the_reference_fills_every_place() {
        let reference = reference();
        assert!(reference.starts_with("### Commands\n"));
        assert!(!reference.contains("{{"));
        // And it is the same every time.
        assert_eq!(reference, super::reference());
    }
}
