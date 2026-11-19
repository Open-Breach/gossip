//! The commands of the command line: what a line typed after `:` asks for.
//!
//! One command per line. The arguments use the syntax of the flags of
//! `gossip follow`; a line that cannot be read is an error that says why,
//! and changes nothing.

use gossip_core::{ColumnsSetting, Level, Pattern, Target};

/// What a line of the command line asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// `:level <lvl>`, or `:level off` for none.
    Level(Option<Level>),
    /// `:grep <text>`, or `:grep` alone to take the rule away.
    Grep(Option<String>),
    /// `:match <regex>`, or `:match` alone to take the rule away.
    Match(Option<String>),
    /// `:field <k=v>`, or `:field <k>` to take the rules on that key away.
    Field { key: String, value: Option<String> },
    /// `:filter`: print the current filter.
    ShowFilter,
    /// `:filter off`: remove every rule.
    FilterOff,
    /// `:add <target>...`
    Add(Vec<Target>),
    /// `:drop <target>...`
    Drop(Vec<Target>),
    /// `:targets`
    Targets,
    /// `:events on` and `:events off`
    Events(bool),
    /// `:container <glob>`, or `:container off` for every container.
    Container(Option<Pattern>),
    /// `:fields <a,b,c>`, `:fields all`, and `:fields default` for none.
    Fields(Option<Vec<String>>),
    /// `:columns`: print the columns.
    ShowColumns,
    /// `:columns default`: the columns the run started with.
    ColumnsDefault,
    /// `:columns pod=24:middle,container=8`
    Columns(ColumnsSetting),
    /// `:save <name>`, or `:save` alone for the template the session was
    /// started from.
    Save(Option<String>),
    /// `:load <name>`
    Load(String),
    /// `:templates`
    Templates,
    /// `:templates show <name>`
    ShowTemplate(String),
    /// `:clear`
    Clear,
    /// `:q`
    Quit,
    /// `:help`
    Help,
    /// `:panic`, in debug builds only: panics, to see by hand that a panic
    /// leaves the terminal usable. It is not in the list of commands.
    #[cfg(debug_assertions)]
    Panic,
}

/// The keys and the commands, one line each, as `?` prints them.
pub const HELP: &[&str] = &[
    "keys:",
    "  Space             pause and resume the output",
    "  Ctrl-L            clear the screen",
    "  :                 open the command line",
    "  /                 open the command line with `grep ` typed",
    "  Esc               close the command line without applying",
    "  q, Ctrl-C         quit",
    "  ?                 this list",
    "commands:",
    "  :level <lvl>      only lines at this level or above; :level off",
    "  :grep <text>      only lines whose message contains the text; :grep alone takes it away",
    "  :match <regex>    only lines whose message matches; :match alone takes it away",
    "  :field <k=v>      only lines whose field has the value; :field <k> takes it away",
    "  :filter           print the filter; :filter off removes every rule",
    "  :add <target>...  follow more targets",
    "  :drop <target>... stop following them",
    "  :targets          what is followed, and how many pods each target has",
    "  :events on|off    the cluster's events",
    "  :container <glob> only containers whose name matches; :container off",
    "  :fields <a,b,c>   the fields shown; :fields all, :fields default",
    "  :columns <spec>   the column widths, as pod=24:middle,container=8; :columns default",
    "  :save [name]      save the session as a template; alone, the one it was started from",
    "  :load <name>      replace the session with a template",
    "  :templates        list the templates; :templates show <name> prints one",
    "  :clear            clear the screen",
    "  :q                quit",
];

/// Reads one line of the command line. The colon that opened it is not
/// part of the line, but one that was typed is forgiven.
pub fn parse(line: &str) -> Result<Command, String> {
    let line = line.trim();
    let line = line.strip_prefix(':').unwrap_or(line).trim_start();
    let (name, rest) = match line.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim()),
        None => (line, ""),
    };
    let nothing_more = |command: Command| {
        if rest.is_empty() {
            Ok(command)
        } else {
            Err(format!("`:{name}` takes no argument, and got `{rest}`"))
        }
    };
    match name {
        "" => Err("an empty command; `?` lists the commands".to_string()),
        "level" => match rest {
            "" => Err("`:level` takes a level, or `off`: trace, debug, info, warn, error".into()),
            "off" => Ok(Command::Level(None)),
            level => level
                .parse()
                .map(|level| Command::Level(Some(level)))
                .map_err(|error: gossip_core::record::UnknownLevel| error.to_string()),
        },
        "grep" => Ok(Command::Grep(text(rest))),
        "match" => Ok(Command::Match(text(rest))),
        "field" => {
            let pair = unquoted(rest);
            match pair.split_once('=') {
                _ if pair.is_empty() => {
                    Err("`:field` takes KEY=VALUE, or a key to take its rules away".to_string())
                }
                Some(("", _)) => Err(format!("`{pair}` names no field before the `=`")),
                Some((key, value)) => Ok(Command::Field {
                    key: key.trim().to_string(),
                    value: Some(unquoted(value.trim_start()).to_string()),
                }),
                None => Ok(Command::Field {
                    key: pair.to_string(),
                    value: None,
                }),
            }
        }
        "filter" => match rest {
            "" => Ok(Command::ShowFilter),
            "off" => Ok(Command::FilterOff),
            other => Err(format!(
                "`:filter` prints the filter and `:filter off` removes it; `{other}` is neither"
            )),
        },
        "add" => targets(name, rest).map(Command::Add),
        "drop" => targets(name, rest).map(Command::Drop),
        "targets" => nothing_more(Command::Targets),
        "events" => match rest {
            "on" => Ok(Command::Events(true)),
            "off" => Ok(Command::Events(false)),
            _ => Err("`:events` takes `on` or `off`".to_string()),
        },
        "container" => match rest {
            "" => Err("`:container` takes a glob, or `off` for every container".to_string()),
            "off" => Ok(Command::Container(None)),
            glob => glob
                .parse()
                .map(|pattern| Command::Container(Some(pattern)))
                .map_err(|error: gossip_core::TargetError| error.to_string()),
        },
        "fields" => match rest {
            "" => Err(
                "`:fields` takes the fields to show, as a,b,c; or `all`, or `default`".to_string(),
            ),
            "default" => Ok(Command::Fields(None)),
            list => Ok(Command::Fields(Some(
                list.split(',')
                    .map(|field| field.trim().to_string())
                    .filter(|field| !field.is_empty())
                    .collect(),
            ))),
        },
        "columns" => match rest {
            "" => Ok(Command::ShowColumns),
            "default" => Ok(Command::ColumnsDefault),
            columns => columns
                .parse()
                .map(Command::Columns)
                .map_err(|error: gossip_core::ColumnsError| error.to_string()),
        },
        "save" => match rest.split_whitespace().collect::<Vec<_>>()[..] {
            [] => Ok(Command::Save(None)),
            [name] => Ok(Command::Save(Some(name.to_string()))),
            _ => Err(format!(
                "`:save` takes one name, of letters, digits, `-` and `_`; `{rest}` is more"
            )),
        },
        "load" => match rest.split_whitespace().collect::<Vec<_>>()[..] {
            [name] => Ok(Command::Load(name.to_string())),
            _ => Err("`:load` takes the name of one template; `:templates` lists them".to_string()),
        },
        "templates" => match rest.split_whitespace().collect::<Vec<_>>()[..] {
            [] => Ok(Command::Templates),
            ["show", name] => Ok(Command::ShowTemplate(name.to_string())),
            _ => Err(
                "`:templates` lists the templates and `:templates show <name>` prints one"
                    .to_string(),
            ),
        },
        "clear" => nothing_more(Command::Clear),
        "q" | "quit" => nothing_more(Command::Quit),
        "help" => nothing_more(Command::Help),
        #[cfg(debug_assertions)]
        "panic" => nothing_more(Command::Panic),
        other => Err(format!(
            "`:{other}` is not a command; `?` lists the commands"
        )),
    }
}

/// The text a command was given: all of the rest of the line, spaces
/// included. None when there is none.
fn text(rest: &str) -> Option<String> {
    let text = unquoted(rest);
    (!text.is_empty()).then(|| text.to_string())
}

/// A text without the quotes around it, for whoever types them out of the
/// habit of a shell. There is no shell here: a text needs none.
fn unquoted(text: &str) -> &str {
    for quote in ['"', '\''] {
        if text.len() >= 2 && text.starts_with(quote) && text.ends_with(quote) {
            return &text[1..text.len() - 1];
        }
    }
    text
}

fn targets(name: &str, rest: &str) -> Result<Vec<Target>, String> {
    if rest.is_empty() {
        return Err(format!(
            "`:{name}` takes one or more targets, as namespace[/workload[/pod[/container]]]"
        ));
    }
    rest.split_whitespace()
        .map(|target| {
            target
                .parse()
                .map_err(|error: gossip_core::TargetError| error.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(text: &str) -> Target {
        text.parse().unwrap()
    }

    #[test]
    fn the_level_command() {
        assert_eq!(parse("level warn"), Ok(Command::Level(Some(Level::Warn))));
        assert_eq!(parse("level ERROR"), Ok(Command::Level(Some(Level::Error))));
        assert_eq!(parse("level off"), Ok(Command::Level(None)));
        assert!(parse("level").unwrap_err().contains("takes a level"));
        let error = parse("level loud").unwrap_err();
        assert!(error.contains("`loud` is not a level"), "{error}");
    }

    #[test]
    fn grep_and_match_take_the_rest_of_the_line() {
        assert_eq!(
            parse("grep payment"),
            Ok(Command::Grep(Some("payment".into())))
        );
        // Spaces and all, as far as the line goes.
        assert_eq!(
            parse("grep payment intent  expired"),
            Ok(Command::Grep(Some("payment intent  expired".into())))
        );
        assert_eq!(
            parse("match ^(card|a plain) line$"),
            Ok(Command::Match(Some("^(card|a plain) line$".into())))
        );
        // Alone, they take their rule away.
        assert_eq!(parse("grep"), Ok(Command::Grep(None)));
        assert_eq!(parse("match  "), Ok(Command::Match(None)));
        // Quotes around the text are a habit of the shell, and dropped.
        assert_eq!(
            parse("grep \"payment intent\""),
            Ok(Command::Grep(Some("payment intent".into())))
        );
        assert_eq!(parse("grep 'off'"), Ok(Command::Grep(Some("off".into()))));
        // `off` is a text like any other here.
        assert_eq!(parse("grep off"), Ok(Command::Grep(Some("off".into()))));
        // A quote inside the text stays.
        assert_eq!(
            parse("grep say \"hi\""),
            Ok(Command::Grep(Some("say \"hi\"".into())))
        );
    }

    #[test]
    fn the_field_command() {
        assert_eq!(
            parse("field tenant_id=t_123"),
            Ok(Command::Field {
                key: "tenant_id".into(),
                value: Some("t_123".into())
            })
        );
        // A value may hold spaces and an equals sign of its own.
        assert_eq!(
            parse("field otel.name=POST /api/v1/flows"),
            Ok(Command::Field {
                key: "otel.name".into(),
                value: Some("POST /api/v1/flows".into())
            })
        );
        assert_eq!(
            parse("field query=a=b"),
            Ok(Command::Field {
                key: "query".into(),
                value: Some("a=b".into())
            })
        );
        assert_eq!(
            parse("field note=\"two words\""),
            Ok(Command::Field {
                key: "note".into(),
                value: Some("two words".into())
            })
        );
        // An empty value is a value.
        assert_eq!(
            parse("field note="),
            Ok(Command::Field {
                key: "note".into(),
                value: Some(String::new())
            })
        );
        // A key alone takes the rules on it away.
        assert_eq!(
            parse("field tenant_id"),
            Ok(Command::Field {
                key: "tenant_id".into(),
                value: None
            })
        );
        assert!(parse("field").unwrap_err().contains("KEY=VALUE"));
        assert!(
            parse("field =t_123")
                .unwrap_err()
                .contains("names no field")
        );
    }

    #[test]
    fn the_filter_command() {
        assert_eq!(parse("filter"), Ok(Command::ShowFilter));
        assert_eq!(parse("filter off"), Ok(Command::FilterOff));
        assert!(parse("filter on").unwrap_err().contains("`on` is neither"));
    }

    #[test]
    fn add_and_drop_take_targets() {
        assert_eq!(
            parse("add fc-stg/fc-web-api"),
            Ok(Command::Add(vec![target("fc-stg/fc-web-api")]))
        );
        assert_eq!(
            parse("add fc-stg/fc-web-api   fc-stg/fc-app monitoring/prometheus/*/exporter"),
            Ok(Command::Add(vec![
                target("fc-stg/fc-web-api"),
                target("fc-stg/fc-app"),
                target("monitoring/prometheus/*/exporter"),
            ]))
        );
        assert_eq!(
            parse("drop fc-stg/fc-api"),
            Ok(Command::Drop(vec![target("fc-stg/fc-api")]))
        );
        assert!(parse("add").unwrap_err().contains("one or more targets"));
        assert!(parse("drop").unwrap_err().contains("one or more targets"));
        // One target that cannot be read refuses the whole line.
        let error = parse("add fc-stg/fc-api a/b/c/d/e").unwrap_err();
        assert!(error.contains("levels"), "{error}");
        assert_eq!(parse("targets"), Ok(Command::Targets));
        assert!(parse("targets now").unwrap_err().contains("no argument"));
    }

    #[test]
    fn the_event_switch() {
        assert_eq!(parse("events on"), Ok(Command::Events(true)));
        assert_eq!(parse("events off"), Ok(Command::Events(false)));
        for bad in ["events", "events yes", "events on off"] {
            assert!(parse(bad).unwrap_err().contains("`on` or `off`"), "{bad}");
        }
    }

    #[test]
    fn the_container_command() {
        let Ok(Command::Container(Some(pattern))) = parse("container api*") else {
            panic!("expected a glob");
        };
        assert!(pattern.matches("api-2") && !pattern.matches("envoy"));
        assert_eq!(parse("container off"), Ok(Command::Container(None)));
        assert!(parse("container").unwrap_err().contains("takes a glob"));
        assert!(parse("container a b").is_err());
    }

    #[test]
    fn the_fields_command() {
        let list = |names: &[&str]| Some(names.iter().map(|name| name.to_string()).collect());
        assert_eq!(
            parse("fields tenant_id,request_id"),
            Ok(Command::Fields(list(&["tenant_id", "request_id"])))
        );
        assert_eq!(
            parse("fields tenant_id, request_id ,"),
            Ok(Command::Fields(list(&["tenant_id", "request_id"])))
        );
        // `all` is the one word the flag knows too; `default` is none.
        assert_eq!(parse("fields all"), Ok(Command::Fields(list(&["all"]))));
        assert_eq!(parse("fields default"), Ok(Command::Fields(None)));
        assert!(parse("fields").unwrap_err().contains("takes the fields"));
    }

    #[test]
    fn the_columns_command() {
        assert_eq!(parse("columns"), Ok(Command::ShowColumns));
        assert_eq!(parse("columns default"), Ok(Command::ColumnsDefault));
        assert_eq!(
            parse("columns pod=24:middle,container=8"),
            Ok(Command::Columns(
                "pod=24:middle,container=8".parse().unwrap()
            ))
        );
        assert!(
            parse("columns node=24")
                .unwrap_err()
                .contains("not a column")
        );
        assert!(
            parse("columns pod=wide")
                .unwrap_err()
                .contains("not a width")
        );
        assert!(
            parse("columns pod=24:left")
                .unwrap_err()
                .contains("not a side")
        );
    }

    #[test]
    fn save_load_and_templates() {
        assert_eq!(parse("save"), Ok(Command::Save(None)));
        assert_eq!(
            parse("save dev-api"),
            Ok(Command::Save(Some("dev-api".into())))
        );
        assert!(parse("save dev api").unwrap_err().contains("one name"));
        assert_eq!(
            parse("load staging-all"),
            Ok(Command::Load("staging-all".into()))
        );
        for bad in ["load", "load a b"] {
            assert!(parse(bad).unwrap_err().contains("one template"), "{bad}");
        }
        assert_eq!(parse("templates"), Ok(Command::Templates));
        assert_eq!(
            parse("templates show dev-api"),
            Ok(Command::ShowTemplate("dev-api".into()))
        );
        for bad in ["templates show", "templates dev-api", "templates show a b"] {
            assert!(parse(bad).unwrap_err().contains("prints one"), "{bad}");
        }
    }

    #[test]
    fn clear_quit_and_help() {
        assert_eq!(parse("clear"), Ok(Command::Clear));
        assert_eq!(parse("q"), Ok(Command::Quit));
        assert_eq!(parse("quit"), Ok(Command::Quit));
        assert_eq!(parse("help"), Ok(Command::Help));
        assert!(parse("q now").unwrap_err().contains("no argument"));
        assert!(parse("clear all").unwrap_err().contains("no argument"));
    }

    #[test]
    fn what_is_not_a_command_says_so() {
        assert_eq!(
            parse("lvl warn"),
            Err("`:lvl` is not a command; `?` lists the commands".to_string())
        );
        assert!(parse("").unwrap_err().contains("empty command"));
        assert!(parse("   ").unwrap_err().contains("empty command"));
        // Commands are lower case, like the flags.
        assert!(parse("Level warn").is_err());
    }

    #[test]
    fn spaces_around_and_a_colon_in_front_are_forgiven() {
        assert_eq!(
            parse("  level   warn  "),
            Ok(Command::Level(Some(Level::Warn)))
        );
        assert_eq!(parse(":level warn"), Ok(Command::Level(Some(Level::Warn))));
        assert_eq!(parse(": q"), Ok(Command::Quit));
    }

    #[test]
    fn the_help_names_every_command() {
        let help = HELP.join("\n");
        for command in [
            ":level",
            ":grep",
            ":match",
            ":field",
            ":filter",
            ":add",
            ":drop",
            ":targets",
            ":events",
            ":container",
            ":fields",
            ":columns",
            ":save",
            ":load",
            ":templates",
            ":clear",
            ":q",
        ] {
            assert!(help.contains(command), "{command}");
        }
        for key in ["Space", "Ctrl-L", "Esc", "Ctrl-C", "?"] {
            assert!(help.contains(key), "{key}");
        }
    }
}
