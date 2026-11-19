//! What the Reference of the README is written from: the commands and
//! flags as `clap` reads them, the keys and the `:` commands of the
//! interactive mode, the fields the output knows by name and the emoji of
//! the events.
//!
//! `cargo xtask readme` writes the tables of the README from here, and a
//! test compares the README with it, so a key or a command that is not in
//! these tables is not in the README either. The tests at the end of this
//! file hold the tables against the keyboard and the parser of the
//! commands.

use clap::CommandFactory;

pub use crate::fields::{ALL as ALL_FIELDS, KNOWN_FIELDS, Place};
pub use crate::interactive::command::HELP;
pub use crate::render::{EVENT_EMOJI, EVENT_TAG, OTHER_EMOJI, WARNING_EMOJI};

/// The command line as `clap` reads it: every command, argument and flag,
/// with its help and its default.
pub fn command_line() -> clap::Command {
    crate::cli::Cli::command()
}

/// Where a key means what a row says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Where {
    /// While the stream scrolls.
    Stream,
    /// While the command line is open.
    CommandLine,
    /// While a question waits on the status line.
    Question,
}

/// One row of the keys: the keys that do one thing, and what it is.
#[derive(Debug, Clone, Copy)]
pub struct KeyRow {
    pub place: Where,
    /// The keys, as a reader writes them: `Space`, `Ctrl-L`, `q`.
    pub keys: &'static [&'static str],
    pub does: &'static str,
}

/// What stands in a row for every key that has no row of its own.
pub const ANY_OTHER_KEY: &str = "any other key";

/// What stands in a row for the keys that are typed into the command line.
pub const ANY_CHARACTER: &str = "any character";

/// The keys of the interactive mode.
///
/// THIS TABLE IS WHAT THE README SAYS ABOUT THE KEYS. A key that is given a
/// meaning in `interactive/keys.rs` gets a row here; the tests below fail
/// until it has one.
pub const KEYS: &[KeyRow] = &[
    KeyRow {
        place: Where::Stream,
        keys: &["Space"],
        does: "Pause the output, and resume it. While it is paused the status line counts the \
               lines that wait; they are printed, in order, when it resumes.",
    },
    KeyRow {
        place: Where::Stream,
        keys: &["Ctrl-L"],
        does: "Clear the screen.",
    },
    KeyRow {
        place: Where::Stream,
        keys: &[":"],
        does: "Open the command line.",
    },
    KeyRow {
        place: Where::Stream,
        keys: &["/"],
        does: "Open the command line with `grep ` typed.",
    },
    KeyRow {
        place: Where::Stream,
        keys: &["?"],
        does: "List the keys and the commands.",
    },
    KeyRow {
        place: Where::Stream,
        keys: &["q", "Ctrl-C"],
        does: "Quit.",
    },
    KeyRow {
        place: Where::CommandLine,
        keys: &["Enter"],
        does: "Apply the line. An empty line closes the command line.",
    },
    KeyRow {
        place: Where::CommandLine,
        keys: &["Esc"],
        does: "Close the command line without applying.",
    },
    KeyRow {
        place: Where::CommandLine,
        keys: &[ANY_CHARACTER],
        does: "Typed into the line at the cursor.",
    },
    KeyRow {
        place: Where::CommandLine,
        keys: &["Backspace", "Ctrl-H"],
        does: "Erase the character before the cursor. On an empty line: close the command \
               line.",
    },
    KeyRow {
        place: Where::CommandLine,
        keys: &["Delete", "Ctrl-D"],
        does: "Erase the character at the cursor.",
    },
    KeyRow {
        place: Where::CommandLine,
        keys: &["Left", "Ctrl-B"],
        does: "Move the cursor one character to the left.",
    },
    KeyRow {
        place: Where::CommandLine,
        keys: &["Right", "Ctrl-F"],
        does: "Move the cursor one character to the right.",
    },
    KeyRow {
        place: Where::CommandLine,
        keys: &["Home", "Ctrl-A"],
        does: "Move the cursor to the start of the line.",
    },
    KeyRow {
        place: Where::CommandLine,
        keys: &["End", "Ctrl-E"],
        does: "Move the cursor to the end of the line.",
    },
    KeyRow {
        place: Where::CommandLine,
        keys: &["Ctrl-U"],
        does: "Erase from the start of the line to the cursor.",
    },
    KeyRow {
        place: Where::CommandLine,
        keys: &["Ctrl-W"],
        does: "Erase the word before the cursor.",
    },
    KeyRow {
        place: Where::CommandLine,
        keys: &["Up", "Ctrl-P"],
        does: "The line entered before this one.",
    },
    KeyRow {
        place: Where::CommandLine,
        keys: &["Down", "Ctrl-N"],
        does: "The line entered after it.",
    },
    KeyRow {
        place: Where::CommandLine,
        keys: &["Ctrl-L"],
        does: "Clear the screen. The line stays as it is.",
    },
    KeyRow {
        place: Where::CommandLine,
        keys: &["Ctrl-C"],
        does: "Quit.",
    },
    KeyRow {
        place: Where::Question,
        keys: &["y"],
        does: "Yes.",
    },
    KeyRow {
        place: Where::Question,
        keys: &["Ctrl-C"],
        does: "Quit.",
    },
    KeyRow {
        place: Where::Question,
        keys: &[ANY_OTHER_KEY],
        does: "No: nothing is done by accident.",
    },
];

/// One form of a `:` command, and what it does.
#[derive(Debug, Clone, Copy)]
pub struct CommandRow {
    /// The form, as a reader writes it: `:level <level>`.
    pub form: &'static str,
    /// A line of that form, as it would be typed after the colon.
    pub example: &'static str,
    pub does: &'static str,
}

/// The `:` commands of the interactive mode, every form of every one.
///
/// THIS TABLE IS WHAT THE README SAYS ABOUT THE COMMANDS. A command that
/// `interactive/command.rs` reads gets a row here for each of its forms;
/// the tests below fail until it has them.
pub const COMMANDS: &[CommandRow] = &[
    CommandRow {
        form: ":level <level>",
        example: "level warn",
        does: "Only lines at this level or above: `trace`, `debug`, `info`, `warn`, `error`.",
    },
    CommandRow {
        form: ":level off",
        example: "level off",
        does: "Take the level rule away.",
    },
    CommandRow {
        form: ":grep <text>",
        example: "grep payment intent",
        does: "Only lines whose message contains the text. The text is the rest of the line, \
               spaces included.",
    },
    CommandRow {
        form: ":grep",
        example: "grep",
        does: "Take that rule away.",
    },
    CommandRow {
        form: ":match <regex>",
        example: "match ^pay",
        does: "Only lines whose message matches the regular expression.",
    },
    CommandRow {
        form: ":match",
        example: "match",
        does: "Take that rule away.",
    },
    CommandRow {
        form: ":field <key>=<value>",
        example: "field tenant_id=t_123",
        does: "Only lines whose field has the value. It is the one rule on that key: a second \
               `:field` for the key replaces the first.",
    },
    CommandRow {
        form: ":field <key>",
        example: "field tenant_id",
        does: "Take the rules on that key away.",
    },
    CommandRow {
        form: ":filter",
        example: "filter",
        does: "Print the filter.",
    },
    CommandRow {
        form: ":filter off",
        example: "filter off",
        does: "Remove every rule, the filter of the config file included.",
    },
    CommandRow {
        form: ":add <target>...",
        example: "add shop/web",
        does: "Follow more targets. Each starts with the history a target has at start.",
    },
    CommandRow {
        form: ":drop <target>...",
        example: "drop shop/web",
        does: "Stop following them. A target is named as it was given.",
    },
    CommandRow {
        form: ":targets",
        example: "targets",
        does: "Print what is followed, and how many pods each target has.",
    },
    CommandRow {
        form: ":events on",
        example: "events on",
        does: "Show the cluster's events.",
    },
    CommandRow {
        form: ":events off",
        example: "events off",
        does: "Leave them out.",
    },
    CommandRow {
        form: ":container <glob>",
        example: "container api*",
        does: "Only containers whose name matches the glob.",
    },
    CommandRow {
        form: ":container off",
        example: "container off",
        does: "Every container.",
    },
    CommandRow {
        form: ":fields <a,b,c>",
        example: "fields tenant_id,request_id",
        does: "Show only these fields of a line, in this order.",
    },
    CommandRow {
        form: ":fields all",
        example: "fields all",
        does: "Show every field, the hidden ones too.",
    },
    CommandRow {
        form: ":fields default",
        example: "fields default",
        does: "The default choice of fields.",
    },
    CommandRow {
        form: ":columns <columns>",
        example: "columns pod=24:middle,container=8",
        does: "Set widths and cut sides, written as for `--columns`: \
               `pod=24:middle,container=8`.",
    },
    CommandRow {
        form: ":columns",
        example: "columns",
        does: "Print the columns.",
    },
    CommandRow {
        form: ":columns default",
        example: "columns default",
        does: "The columns the run started with.",
    },
    CommandRow {
        form: ":save <name>",
        example: "save shop",
        does: "Save the session as the template of that name in the config file: what it \
               follows, with its filter, fields and columns. Asks `y/n` before it replaces a \
               template that is there. The name `last` is refused: it is Gossip's own.",
    },
    CommandRow {
        form: ":save",
        example: "save",
        does: "Save to the template the session is: the one it was started from, loaded, or \
               last saved as. Without one, the command line opens again with `save ` typed, to \
               take a name.",
    },
    CommandRow {
        form: ":load <name>",
        example: "load shop",
        does: "Replace the session with a template.",
    },
    CommandRow {
        form: ":templates",
        example: "templates",
        does: "List the templates.",
    },
    CommandRow {
        form: ":templates show <name>",
        example: "templates show shop",
        does: "Print one template as it stands in the config file.",
    },
    CommandRow {
        form: ":record <file>",
        example: "record shop.jsonl",
        does: "Append every record to the file from now on, one JSON object per line, also \
               what the filter hides.",
    },
    CommandRow {
        form: ":record",
        example: "record",
        does: "Say whether a recording runs, and how much it holds.",
    },
    CommandRow {
        form: ":record off",
        example: "record off",
        does: "Stop recording.",
    },
    CommandRow {
        form: ":clear",
        example: "clear",
        does: "Clear the screen.",
    },
    CommandRow {
        form: ":q",
        example: "q",
        does: "Quit. `:quit` does the same.",
    },
    CommandRow {
        form: ":help",
        example: "help",
        does: "List the keys and the commands, as `?` does.",
    },
];

/// The name of the command a form is a form of: `level` for `:level off`.
pub fn command_name(form: &str) -> &str {
    let form = form.strip_prefix(':').unwrap_or(form);
    form.split_whitespace().next().unwrap_or(form)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::interactive::command::{Command, parse};
    use crate::interactive::keys::{Action, Key, Keyboard};

    /// The key a row names.
    fn key(name: &str) -> Key {
        match name {
            "Space" => Key::Char(' '),
            "Enter" => Key::Enter,
            "Esc" => Key::Esc,
            "Backspace" => Key::Backspace,
            "Delete" => Key::Delete,
            "Left" => Key::Left,
            "Right" => Key::Right,
            "Up" => Key::Up,
            "Down" => Key::Down,
            "Home" => Key::Home,
            "End" => Key::End,
            name => match (name.strip_prefix("Ctrl-"), name.chars().count()) {
                (Some(letter), _) if letter.chars().count() == 1 => {
                    Key::Ctrl(letter.chars().next().unwrap().to_ascii_lowercase())
                }
                (None, 1) => Key::Char(name.chars().next().unwrap()),
                _ => panic!("`{name}` is not the name of a key"),
            },
        }
    }

    /// The keys a place has rows for, without the rows that stand for the
    /// rest.
    fn named(place: Where) -> BTreeSet<String> {
        KEYS.iter()
            .filter(|row| row.place == place)
            .flat_map(|row| row.keys.iter())
            .filter(|name| ![ANY_OTHER_KEY, ANY_CHARACTER].contains(name))
            .map(|name| format!("{:?}", key(name)))
            .collect()
    }

    /// Every key that is not a character to type: the named keys and every
    /// letter with `Ctrl`.
    fn special_keys() -> Vec<Key> {
        let mut keys = vec![
            Key::Enter,
            Key::Esc,
            Key::Backspace,
            Key::Delete,
            Key::Left,
            Key::Right,
            Key::Up,
            Key::Down,
            Key::Home,
            Key::End,
        ];
        keys.extend(('a'..='z').map(Key::Ctrl));
        keys
    }

    /// Every character a keyboard types.
    fn characters() -> impl Iterator<Item = Key> {
        (' '..='~').map(Key::Char)
    }

    #[test]
    fn every_key_that_does_something_in_the_stream_has_a_row() {
        let acting: BTreeSet<String> = special_keys()
            .into_iter()
            .chain(characters())
            .filter(|key| Keyboard::default().press(*key) != Action::Nothing)
            .map(|key| format!("{key:?}"))
            .collect();
        assert_eq!(acting, named(Where::Stream));
    }

    /// A keyboard whose command line is open on `ab` with the cursor
    /// between the two, after one line was entered before.
    fn on_the_command_line() -> Keyboard {
        let mut keyboard = Keyboard::default();
        keyboard.press(Key::Char(':'));
        keyboard.press(Key::Char('x'));
        keyboard.press(Key::Enter);
        keyboard.press(Key::Char(':'));
        keyboard.press(Key::Char('a'));
        keyboard.press(Key::Char('b'));
        keyboard.press(Key::Left);
        keyboard
    }

    #[test]
    fn every_key_that_does_something_on_the_command_line_has_a_row() {
        let acting: BTreeSet<String> = special_keys()
            .into_iter()
            .filter(|key| {
                let mut keyboard = on_the_command_line();
                if matches!(key, Key::Down | Key::Ctrl('n')) {
                    // There is a later line only after going back to an
                    // earlier one.
                    keyboard.press(Key::Up);
                }
                keyboard.press(*key) != Action::Nothing
            })
            .map(|key| format!("{key:?}"))
            .collect();
        assert_eq!(acting, named(Where::CommandLine));

        // Every character is typed into the line, the ones that are keys of
        // the stream too.
        for character in characters() {
            let mut keyboard = on_the_command_line();
            assert_eq!(keyboard.press(character), Action::Prompt, "{character:?}");
            let (text, _) = keyboard.prompt().unwrap();
            assert_eq!(text.chars().count(), 3, "{character:?}");
        }
        assert!(
            KEYS.iter()
                .any(|row| { row.place == Where::CommandLine && row.keys == [ANY_CHARACTER] })
        );
    }

    #[test]
    fn a_question_is_answered_as_the_rows_say() {
        let answer = |key: Key| {
            let mut keyboard = Keyboard::default();
            keyboard.ask("replace it?");
            keyboard.press(key)
        };
        let mut yes = BTreeSet::new();
        let mut quit = BTreeSet::new();
        for key in special_keys().into_iter().chain(characters()) {
            match answer(key) {
                // The capital letter is the same answer; the row names the
                // key once.
                Action::Answer(true) if key == Key::Char('Y') => {}
                Action::Answer(true) => {
                    yes.insert(format!("{key:?}"));
                }
                Action::Quit => {
                    quit.insert(format!("{key:?}"));
                }
                other => assert_eq!(other, Action::Answer(false), "{key:?}"),
            }
        }
        assert_eq!(answer(Key::Char('Y')), Action::Answer(true));
        let rows: Vec<&KeyRow> = KEYS
            .iter()
            .filter(|row| row.place == Where::Question)
            .collect();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].keys, ["y"]);
        assert_eq!(yes, BTreeSet::from([format!("{:?}", key("y"))]));
        assert_eq!(rows[1].keys, ["Ctrl-C"]);
        assert_eq!(quit, BTreeSet::from([format!("{:?}", key("Ctrl-C"))]));
        assert_eq!(rows[2].keys, [ANY_OTHER_KEY]);
    }

    /// The form of a command, as the table writes it.
    ///
    /// A NEW COMMAND DOES NOT COMPILE HERE until it has a form. Give it a
    /// row in `COMMANDS` as well, with that form.
    fn form(command: &Command) -> &'static str {
        match command {
            Command::Level(Some(_)) => ":level <level>",
            Command::Level(None) => ":level off",
            Command::Grep(Some(_)) => ":grep <text>",
            Command::Grep(None) => ":grep",
            Command::Match(Some(_)) => ":match <regex>",
            Command::Match(None) => ":match",
            Command::Field { value: Some(_), .. } => ":field <key>=<value>",
            Command::Field { value: None, .. } => ":field <key>",
            Command::ShowFilter => ":filter",
            Command::FilterOff => ":filter off",
            Command::Add(_) => ":add <target>...",
            Command::Drop(_) => ":drop <target>...",
            Command::Targets => ":targets",
            Command::Events(true) => ":events on",
            Command::Events(false) => ":events off",
            Command::Container(Some(_)) => ":container <glob>",
            Command::Container(None) => ":container off",
            Command::Fields(Some(fields)) if fields == &[ALL_FIELDS] => ":fields all",
            Command::Fields(Some(_)) => ":fields <a,b,c>",
            Command::Fields(None) => ":fields default",
            Command::Columns(_) => ":columns <columns>",
            Command::ShowColumns => ":columns",
            Command::ColumnsDefault => ":columns default",
            Command::Save(Some(_)) => ":save <name>",
            Command::Save(None) => ":save",
            Command::Load(_) => ":load <name>",
            Command::Templates => ":templates",
            Command::ShowTemplate(_) => ":templates show <name>",
            Command::Record(_) => ":record <file>",
            Command::RecordStatus => ":record",
            Command::RecordOff => ":record off",
            Command::Clear => ":clear",
            Command::Quit => ":q",
            Command::Help => ":help",
            // Not a command of the program: debug builds only.
            #[cfg(debug_assertions)]
            Command::Panic => ":panic",
        }
    }

    /// How many forms the match above gives, `:panic` aside. It goes up with
    /// every arm that is added there, and the table then needs the row.
    const FORMS: usize = 34;

    #[test]
    fn every_form_of_every_command_has_a_row_and_its_example_is_that_form() {
        let mut forms = BTreeSet::new();
        for row in COMMANDS {
            let command = parse(row.example)
                .unwrap_or_else(|error| panic!("`{}` is not read: {error}", row.example));
            assert_eq!(form(&command), row.form, "the example `{}`", row.example);
            assert!(forms.insert(row.form), "{} has two rows", row.form);
            assert!(row.form.starts_with(':'), "{}", row.form);
            assert!(row.does.ends_with('.'), "{}", row.form);
        }
        assert_eq!(forms.len(), FORMS);
    }

    #[test]
    fn the_rows_and_the_list_that_the_question_mark_prints_name_the_same_commands() {
        let rows: BTreeSet<&str> = COMMANDS.iter().map(|row| command_name(row.form)).collect();
        let mut listed: BTreeSet<&str> = HELP
            .iter()
            .skip_while(|line| **line != "commands:")
            .skip(1)
            .map(|line| command_name(line.trim_start()))
            .collect();
        // `:help` is not in its own list: `?` is.
        listed.insert("help");
        assert_eq!(rows, listed);

        // And the keys of that list have their rows.
        let keys: BTreeSet<&str> = KEYS
            .iter()
            .flat_map(|row| row.keys.iter().copied())
            .collect();
        for line in HELP.iter().skip(1).take_while(|line| **line != "commands:") {
            let names = line.trim_start().split("  ").next().unwrap();
            for name in names.split(", ") {
                assert!(keys.contains(name), "`{name}` of `{line}`");
            }
        }
    }

    #[test]
    fn the_command_line_is_the_one_clap_reads() {
        let command = command_line();
        let names: Vec<&str> = command
            .get_subcommands()
            .map(|sub| sub.get_name())
            .collect();
        assert_eq!(names, ["list", "follow", "replay", "templates"]);
    }

    #[test]
    fn a_row_says_one_thing_in_whole_sentences() {
        for row in KEYS {
            assert!(!row.keys.is_empty());
            assert!(row.does.ends_with('.'), "{:?}", row.keys);
            assert!(!row.does.contains("  "), "{:?}", row.keys);
        }
        for row in COMMANDS {
            assert!(!row.does.contains("  "), "{}", row.form);
        }
    }
}
