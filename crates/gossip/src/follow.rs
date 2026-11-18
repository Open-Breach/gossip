//! `gossip follow`: follow the logs of a set of targets and print them as
//! they come.

use std::io::{self, IsTerminal, Write};

use anyhow::{Context, bail};
use gossip_core::{
    Cluster, Config, Filter, Kind, Output, Record, Session, SessionOptions, Settings, Template,
};

use crate::cli::FollowArgs;
use crate::render::{TextOptions, TextRenderer, Zone, json_line};

pub async fn run(args: FollowArgs) -> anyhow::Result<()> {
    let config = Config::load()?;
    let settings = settings(&args, &config)?;
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

/// What to follow and how: the flags, over the template when one is named,
/// over the `[defaults]` of the config file, over the built-in defaults.
fn settings(args: &FollowArgs, config: &Config) -> anyhow::Result<Settings> {
    let asked = Template {
        context: args.context.clone(),
        targets: args.targets.clone(),
        container: args.container.clone(),
        since: args.since,
        tail: args.tail,
        events: args.no_events.then_some(false),
        format: None,
        filter: filter(args)?,
        fields: args.fields.clone(),
        output: args.output,
    };
    let template = match &args.template {
        Some(name) => crate::templates::find(config, name)?.clone(),
        None => Template::default(),
    };
    let settings = asked
        .or(template)
        .or(config.defaults.clone().into())
        .resolve();
    if settings.targets.is_empty()
        && let Some(name) = &args.template
    {
        bail!("the template `{name}` has no targets");
    }
    Ok(settings)
}

/// The filter the flags stand for: every one of them must hold for a line.
/// The flags are about lines; events pass them, and `--no-events` is what
/// leaves events out.
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
    let lines = match rules.len() {
        0 => return Ok(None),
        1 => rules.remove(0),
        _ => Filter::All(rules),
    };
    Ok(Some(Filter::Any(vec![Filter::kind_is(Kind::Event), lines])))
}

/// Whether the terminal can be trusted with emoji: the locale says UTF-8.
fn locale_is_utf8() -> bool {
    if cfg!(windows) {
        return true;
    }
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .filter_map(|variable| std::env::var(variable).ok())
        .find(|value| !value.is_empty());
    locale.is_some_and(|locale| is_utf8_locale(&locale))
}

fn is_utf8_locale(locale: &str) -> bool {
    let locale = locale.to_lowercase();
    locale.contains("utf-8") || locale.contains("utf8")
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
                // Emoji only where they will show: a terminal that says
                // it speaks UTF-8.
                emoji: !args.no_emoji && stdout_is_terminal && locale_is_utf8(),
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
        let settings = settings(&args(&[]), &Config::default()).unwrap();
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
        let settings = settings(&args(&["--since", "5m"]), &Config::default()).unwrap();
        assert_eq!(settings.since, Some(Duration::from_secs(300)));
        assert_eq!(settings.tail, None);

        let both = super::settings(
            &args(&["--since", "5m", "--tail", "50"]),
            &Config::default(),
        )
        .unwrap();
        assert_eq!(both.since, Some(Duration::from_secs(300)));
        assert_eq!(both.tail, Some(50));
    }

    /// The filter of some flags, without the part that lets events pass.
    fn line_filter(extra: &[&str]) -> Filter {
        match filter(&args(extra)).unwrap() {
            Some(Filter::Any(mut parts)) => {
                assert_eq!(parts.len(), 2);
                assert_eq!(parts[0], Filter::kind_is(Kind::Event));
                parts.remove(1)
            }
            other => panic!("expected events or lines, got {other:?}"),
        }
    }

    #[test]
    fn the_flags_build_the_rules_they_stand_for() {
        assert_eq!(filter(&args(&[])).unwrap(), None);
        assert_eq!(
            line_filter(&["--level", "warn"]),
            Filter::level_at_least(Level::Warn)
        );
        assert_eq!(
            line_filter(&["--grep", "expired"]),
            Filter::message_contains("expired")
        );
        assert_eq!(
            line_filter(&["--match", "^pay"]),
            Filter::message_matches("^pay").unwrap()
        );
        assert_eq!(
            line_filter(&["--field", "tenant_id=t_123"]),
            Filter::field_equals("tenant_id", "t_123").unwrap()
        );
        // A value may hold an equals sign of its own.
        assert_eq!(
            line_filter(&["--field", "query=a=b"]),
            Filter::field_equals("query", "a=b").unwrap()
        );
    }

    #[test]
    fn the_filter_flags_are_about_lines_and_events_still_show() {
        use chrono::Utc;
        use gossip_core::{Format, Record};
        use std::collections::BTreeMap;

        let filter = filter(&args(&["--level", "warn", "--field", "tenant_id=t_123"]))
            .unwrap()
            .unwrap();
        let record = |kind: Kind, level: Option<Level>, tenant: Option<&str>| Record {
            cluster: "c".into(),
            namespace: "flowcpt".into(),
            pod: "p".into(),
            container: None,
            time: Utc::now(),
            kind,
            format: Format::Text,
            level,
            message: String::new(),
            fields: tenant
                .map(|tenant| BTreeMap::from([("tenant_id".to_string(), tenant.into())]))
                .unwrap_or_default(),
            raw: String::new(),
            previous: false,
            truncated: false,
        };
        // A line must pass every flag.
        assert!(filter.accepts(&record(Kind::Line, Some(Level::Error), Some("t_123"))));
        assert!(!filter.accepts(&record(Kind::Line, Some(Level::Info), Some("t_123"))));
        assert!(!filter.accepts(&record(Kind::Line, Some(Level::Error), Some("t_999"))));
        assert!(!filter.accepts(&record(Kind::Line, None, None)));
        // An event passes, normal or warning, with no tenant at all.
        assert!(filter.accepts(&record(Kind::Event, Some(Level::Info), None)));
        assert!(filter.accepts(&record(Kind::Event, Some(Level::Warn), None)));
    }

    #[test]
    fn events_are_on_unless_no_events_is_given() {
        assert!(settings(&args(&[]), &Config::default()).unwrap().events);
        assert!(
            !settings(&args(&["--no-events"]), &Config::default())
                .unwrap()
                .events
        );
    }

    // ---- templates and defaults ---------------------------------------

    const CONFIG: &str = r#"
[defaults]
context = "aks-vulcan-01"
tail = 50
events = false
output = "json"

[templates.dev-api]
targets = ["flowcpt/flowcpt-api", "flowcpt/flowcpt-web-api"]
container = "api"
filter = { field = "level", op = ">=", value = "info" }
fields = ["tenant_id", "request_id"]
format = { api = "json" }

[templates.staging-all]
context = "aks-staging"
targets = ["flowcpt-stg/*"]
since = "5m"
output = "text"

[templates.empty]
container = "api"
"#;

    /// `gossip follow` with these arguments and nothing else.
    fn follow_args(arguments: &[&str]) -> FollowArgs {
        let cli = Cli::try_parse_from(["gossip", "follow"].iter().chain(arguments)).unwrap();
        match cli.command {
            Command::Follow(args) => *args,
            other => panic!("expected the follow command, got {other:?}"),
        }
    }

    fn config() -> Config {
        Config::from_toml(CONFIG).unwrap()
    }

    #[test]
    fn a_template_behaves_as_the_flags_it_spells_out() {
        let settings = settings(&follow_args(&["--template", "dev-api"]), &config()).unwrap();
        assert_eq!(
            settings.targets,
            [
                "flowcpt/flowcpt-api".parse().unwrap(),
                "flowcpt/flowcpt-web-api".parse().unwrap()
            ]
        );
        assert!(settings.container.unwrap().matches("api"));
        assert_eq!(settings.filter, Some(Filter::level_at_least(Level::Info)));
        assert_eq!(
            settings.fields,
            Some(vec!["tenant_id".to_string(), "request_id".to_string()])
        );
        assert_eq!(settings.format.for_container("api"), FormatChoice::Json);
        assert_eq!(settings.format.for_container("envoy"), FormatChoice::Auto);
        // What the template leaves out comes from the defaults.
        assert_eq!(settings.context.as_deref(), Some("aks-vulcan-01"));
        assert_eq!(settings.tail, Some(50));
        assert_eq!(settings.since, None);
        assert!(!settings.events);
        assert_eq!(settings.output, Output::Json);
    }

    #[test]
    fn a_template_wins_over_the_defaults() {
        let settings = settings(&follow_args(&["--template", "staging-all"]), &config()).unwrap();
        assert_eq!(settings.context.as_deref(), Some("aks-staging"));
        assert_eq!(settings.output, Output::Text);
        // Its `since` decides the history; the `tail` of the defaults does
        // not cut it to 50 lines.
        assert_eq!(settings.since, Some(Duration::from_secs(300)));
        assert_eq!(settings.tail, None);
    }

    #[test]
    fn a_flag_beside_a_template_overrides_it() {
        let settings = settings(
            &follow_args(&[
                "--template",
                "dev-api",
                "--context",
                "kind-dev",
                "--container",
                "envoy",
                "--tail",
                "5",
                "--fields",
                "status",
                "--output",
                "text",
                "--level",
                "error",
            ]),
            &config(),
        )
        .unwrap();
        assert_eq!(settings.context.as_deref(), Some("kind-dev"));
        assert!(settings.container.unwrap().matches("envoy"));
        assert_eq!(settings.tail, Some(5));
        assert_eq!(settings.fields, Some(vec!["status".to_string()]));
        assert_eq!(settings.output, Output::Text);
        // The filter of the flags takes the place of the template's.
        assert_eq!(
            settings.filter,
            Some(Filter::Any(vec![
                Filter::kind_is(Kind::Event),
                Filter::level_at_least(Level::Error),
            ]))
        );
        // What no flag names stays as the template has it.
        assert_eq!(settings.targets.len(), 2);
        assert_eq!(settings.format.for_container("api"), FormatChoice::Json);
    }

    #[test]
    fn the_defaults_apply_without_a_template_too() {
        let settings = settings(&follow_args(&["flowcpt"]), &config()).unwrap();
        assert_eq!(settings.context.as_deref(), Some("aks-vulcan-01"));
        assert_eq!(settings.tail, Some(50));
        assert!(!settings.events);
        assert_eq!(settings.output, Output::Json);
        assert_eq!(settings.filter, None);

        let overridden =
            super::settings(&follow_args(&["flowcpt", "--since", "1m"]), &config()).unwrap();
        assert_eq!(overridden.since, Some(Duration::from_secs(60)));
        assert_eq!(overridden.tail, None);
    }

    #[test]
    fn a_template_that_is_not_there_or_has_no_targets_is_an_error() {
        let error = settings(&follow_args(&["--template", "nope"]), &config()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "there is no template `nope`; the templates are: dev-api, empty, staging-all"
        );
        let error = settings(&follow_args(&["--template", "empty"]), &config()).unwrap_err();
        assert_eq!(error.to_string(), "the template `empty` has no targets");
        let error = settings(&follow_args(&["--template", "x"]), &Config::default()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "there is no template `x`; the config file holds no templates"
        );
    }

    #[test]
    fn a_locale_that_says_utf8() {
        for locale in ["en_US.UTF-8", "de_CH.utf8", "C.UTF-8", "en_GB.utf-8"] {
            assert!(is_utf8_locale(locale), "{locale}");
        }
        for locale in ["C", "POSIX", "en_US", "en_US.ISO-8859-1"] {
            assert!(!is_utf8_locale(locale), "{locale}");
        }
    }

    #[test]
    fn several_flags_combine_with_and() {
        let combined = line_filter(&[
            "--level",
            "warn",
            "--grep",
            "expired",
            "--field",
            "tenant_id=t_123",
            "--field",
            "status=402",
        ]);
        assert_eq!(
            combined,
            Filter::All(vec![
                Filter::level_at_least(Level::Warn),
                Filter::message_contains("expired"),
                Filter::field_equals("tenant_id", "t_123").unwrap(),
                Filter::field_equals("status", "402").unwrap(),
            ])
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
            let settings = settings(&args, &Config::default()).unwrap();
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
