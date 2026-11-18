//! `gossip follow`: follow the logs of a set of targets and print them as
//! they come.

use std::io::{self, IsTerminal, Write};

use anyhow::{Context, bail};
use gossip_core::{
    Cluster, Filter, Kind, Output, Record, Session, SessionOptions, Settings, Template,
};

use crate::cli::FollowArgs;
use crate::render::{TextOptions, TextRenderer, Zone, json_line};

pub async fn run(args: FollowArgs) -> anyhow::Result<()> {
    let settings = settings(&args)?;
    let cluster = Cluster::open(settings.context.as_deref()).await?;
    // A cluster that cannot be reached at start is an error. Once the
    // session runs, the core retries instead.
    cluster.check().await?;

    let mut printer = Printer::new(&args, &settings, io::stdout().is_terminal());
    let mut session = Session::start(
        &cluster,
        settings.targets.clone(),
        settings.filter.clone(),
        SessionOptions::from(&settings),
    );
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    loop {
        tokio::select! {
            // Ctrl-C stops the session, which closes every stream, and is
            // a normal way to end.
            _ = &mut interrupt => return Ok(()),
            record = session.next() => match record {
                Some(record) => printer.print(&record)?,
                None => return Ok(()),
            },
        }
    }
}

/// What the command line asks for, with the built-in defaults for whatever
/// it leaves out.
fn settings(args: &FollowArgs) -> anyhow::Result<Settings> {
    let asked = Template {
        context: args.context.clone(),
        targets: args.targets.clone(),
        container: args.container.clone(),
        since: args.since,
        tail: args.tail,
        events: None,
        format: None,
        filter: filter(args)?,
        fields: args.fields.clone(),
        output: args.output,
    };
    Ok(asked.resolve())
}

/// The filter the flags stand for: every one of them must hold.
fn filter(args: &FollowArgs) -> anyhow::Result<Option<Filter>> {
    let mut rules = Vec::new();
    if let Some(level) = args.level {
        rules.push(Filter::level_at_least(level));
    }
    if let Some(text) = &args.grep {
        rules.push(Filter::message_contains(text));
    }
    if let Some(regex) = &args.matches {
        rules.push(Filter::message_matches(regex).context("--match")?);
    }
    for pair in &args.fields_equal {
        let Some((key, value)) = pair.split_once('=') else {
            bail!("--field takes KEY=VALUE, and `{pair}` has no `=`");
        };
        rules.push(Filter::field_equals(key, value).context("--field")?);
    }
    Ok(match rules.len() {
        0 => None,
        1 => rules.pop(),
        _ => Some(Filter::All(rules)),
    })
}

/// Prints records to stdout in the rendering that was asked for.
struct Printer {
    mode: Mode,
}

enum Mode {
    Text(TextRenderer),
    Json,
    Raw,
}

impl Printer {
    fn new(args: &FollowArgs, settings: &Settings, stdout_is_terminal: bool) -> Self {
        let mode = if args.raw {
            Mode::Raw
        } else if settings.output == Output::Json {
            Mode::Json
        } else {
            Mode::Text(TextRenderer::new(TextOptions {
                colour: stdout_is_terminal && !args.no_color,
                fields: settings.fields.clone(),
                long_names: args.long_names,
                zone: Zone::Local,
            }))
        };
        Printer { mode }
    }

    fn print(&mut self, record: &Record) -> anyhow::Result<()> {
        let mut out = io::stdout().lock();
        match &mut self.mode {
            Mode::Text(renderer) => writeln!(out, "{}", renderer.render(record))?,
            Mode::Json => {
                let line = json_line(record).context("cannot write a record as JSON")?;
                writeln!(out, "{line}")?;
            }
            // The raw output is the lines and nothing else, so what the
            // core has to say goes to stderr.
            Mode::Raw if record.kind == Kind::Notice => eprintln!("-- {}", record.message),
            Mode::Raw => writeln!(out, "{}", record.raw)?,
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use clap::Parser;
    use gossip_core::{FormatChoice, FormatSetting, Level};

    use crate::cli::{Cli, Command};

    fn args(extra: &[&str]) -> FollowArgs {
        let cli = Cli::try_parse_from(
            ["gossip", "follow", "flowcpt/flowcpt-api"]
                .iter()
                .chain(extra),
        )
        .unwrap();
        match cli.command {
            Command::Follow(args) => *args,
            other => panic!("expected the follow command, got {other:?}"),
        }
    }

    #[test]
    fn the_defaults_of_follow() {
        let settings = settings(&args(&[])).unwrap();
        assert_eq!(settings.context, None);
        assert_eq!(settings.targets.len(), 1);
        assert_eq!(settings.container, None);
        // The last 20 lines per container.
        assert_eq!(settings.since, None);
        assert_eq!(settings.tail, Some(20));
        assert_eq!(settings.format, FormatSetting::All(FormatChoice::Auto));
        assert_eq!(settings.filter, None);
        assert_eq!(settings.fields, None);
        assert_eq!(settings.output, Output::Text);
    }

    #[test]
    fn since_alone_is_not_cut_to_twenty_lines() {
        let settings = settings(&args(&["--since", "5m"])).unwrap();
        assert_eq!(settings.since, Some(Duration::from_secs(300)));
        assert_eq!(settings.tail, None);

        let both = super::settings(&args(&["--since", "5m", "--tail", "50"])).unwrap();
        assert_eq!(both.since, Some(Duration::from_secs(300)));
        assert_eq!(both.tail, Some(50));
    }

    #[test]
    fn the_flags_build_the_rules_they_stand_for() {
        assert_eq!(
            filter(&args(&["--level", "warn"])).unwrap(),
            Some(Filter::level_at_least(Level::Warn))
        );
        assert_eq!(
            filter(&args(&["--grep", "expired"])).unwrap(),
            Some(Filter::message_contains("expired"))
        );
        assert_eq!(
            filter(&args(&["--match", "^pay"])).unwrap(),
            Some(Filter::message_matches("^pay").unwrap())
        );
        assert_eq!(
            filter(&args(&["--field", "tenant_id=t_123"])).unwrap(),
            Some(Filter::field_equals("tenant_id", "t_123").unwrap())
        );
        // A value may hold an equals sign of its own.
        assert_eq!(
            filter(&args(&["--field", "query=a=b"])).unwrap(),
            Some(Filter::field_equals("query", "a=b").unwrap())
        );
    }

    #[test]
    fn several_flags_combine_with_and() {
        let combined = filter(&args(&[
            "--level",
            "warn",
            "--grep",
            "expired",
            "--field",
            "tenant_id=t_123",
            "--field",
            "status=402",
        ]))
        .unwrap();
        assert_eq!(
            combined,
            Some(Filter::All(vec![
                Filter::level_at_least(Level::Warn),
                Filter::message_contains("expired"),
                Filter::field_equals("tenant_id", "t_123").unwrap(),
                Filter::field_equals("status", "402").unwrap(),
            ]))
        );
    }

    #[test]
    fn a_flag_that_is_not_a_rule_is_an_error() {
        let error = filter(&args(&["--match", "(unclosed"])).unwrap_err();
        assert!(format!("{error:#}").contains("--match"), "{error:#}");
        let error = filter(&args(&["--field", "tenant_id"])).unwrap_err();
        assert!(error.to_string().contains("KEY=VALUE"), "{error}");
        assert!(filter(&args(&["--field", "=t_123"])).is_err());
    }

    #[test]
    fn which_rendering_the_flags_pick() {
        let text = |extra: &[&str], terminal: bool| {
            let args = args(extra);
            let settings = settings(&args).unwrap();
            match Printer::new(&args, &settings, terminal).mode {
                Mode::Text(_) => "text",
                Mode::Json => "json",
                Mode::Raw => "raw",
            }
        };
        assert_eq!(text(&[], true), "text");
        assert_eq!(text(&["--output", "text"], false), "text");
        assert_eq!(text(&["--output", "json"], true), "json");
        assert_eq!(text(&["--raw"], true), "raw");
        // Raw is raw, whatever the output says.
        assert_eq!(text(&["--raw", "--output", "json"], true), "raw");
    }
}
