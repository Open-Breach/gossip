//! `gossip follow`: follow the logs of a set of targets and print them as
//! they come.

use std::io::{self, IsTerminal, Write};

use anyhow::{Context, bail};
use gossip_core::save::LAST;
use gossip_core::{
    Cluster, Config, Kind, Output, Record, Session, SessionOptions, SessionSpec, Settings,
};

use crate::cli::{FollowArgs, OutputFlags};
use crate::fields::FieldChoice;
use crate::filters::{FilterState, LineRules};
use crate::interactive;
use crate::record::{Failures, Recorder};
use crate::render::{TextOptions, TextRenderer, Zone, json_line};

pub async fn run(args: FollowArgs) -> anyhow::Result<()> {
    let config = Config::load()?;
    let (settings, filter) = settings_and_filter(&args, &config)?;
    let template = template_name(&args);
    let cluster = Cluster::open(settings.context.as_deref()).await?;
    // A cluster that cannot be reached at start is an error. Once the
    // session runs, the core retries instead.
    cluster.check().await?;

    let stdout_is_terminal = io::stdout().is_terminal();
    let printer = Printer::new(&args.output, &settings, stdout_is_terminal);
    let session = Session::start(
        &cluster,
        settings.targets.clone(),
        settings.filter.clone(),
        SessionOptions::from(&settings),
    );
    // A recording gets every record from the first one on, before the
    // filter. It is beside the output and changes nothing of it.
    let failures = Failures::new();
    let recorder = match &args.record {
        Some(file) => {
            let (recorder, tap) = Recorder::start(file, failures.tx.clone())
                .with_context(|| format!("cannot record to {}", file.display()))?;
            session.handle().set_tap(Some(tap));
            Some(recorder)
        }
        None => None,
    };
    // The interactive mode is for a terminal on both sides. Without one,
    // nothing of it shows: the lines are printed as they always were.
    if interactive::wanted(&args.output, io::stdin().is_terminal(), stdout_is_terminal) {
        let setup = interactive::Setup {
            printer,
            filter,
            pause_buffer: config.pause_buffer,
            // `last` is no template to save back into by its name.
            template: template.filter(|name| name != LAST),
            config_path: Config::path()?,
            recorder,
            failures,
        };
        return interactive::run(session, setup).await;
    }
    follow_plainly(session, printer, recorder, failures).await
}

/// Prints the records of a session as they come, until Ctrl-C.
async fn follow_plainly(
    mut session: Session,
    mut printer: Printer,
    recorder: Option<Recorder>,
    mut failures: Failures,
) -> anyhow::Result<()> {
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    loop {
        tokio::select! {
            // Ctrl-C stops the session, which closes every stream, and is
            // a normal way to end.
            _ = &mut interrupt => break,
            record = session.next() => match record {
                Some(record) => printer.print(&record)?,
                None => break,
            },
            // A recording that stopped by itself; the stream goes on.
            Some(failure) = failures.rx.recv() => eprintln!("gossip: {failure}"),
        }
    }
    // The session goes, and its tap with it; then the last records of a
    // recording reach the file.
    drop(session);
    if let Some(recorder) = recorder {
        recorder.finish();
    }
    Ok(())
}

/// What to follow and how: the flags, over the template when one is named,
/// over the `[defaults]` and the `[columns]` of the config file, over the
/// built-in defaults.
///
/// The filter is the one thing a flag does not override. The filter flags
/// narrow the filter of the config file: a record must pass both. With
/// `--no-filter` the filter of the config file is dropped first.
#[cfg(test)]
fn settings(args: &FollowArgs, config: &Config) -> anyhow::Result<Settings> {
    settings_and_filter(args, config).map(|(settings, _)| settings)
}

/// The same, and the filter in its two parts: the filter of the config file
/// and the rules of the flags, which the commands typed later change.
fn settings_and_filter(
    args: &FollowArgs,
    config: &Config,
) -> anyhow::Result<(Settings, FilterState)> {
    let asked = SessionSpec {
        context: args.context.clone(),
        targets: args.targets.clone(),
        container: args.container.clone(),
        since: args.since,
        tail: args.tail,
        events: args.no_events.then_some(false),
        format: None,
        filter: None,
        fields: args.output.fields.clone(),
        output: args.output.output,
        columns: args.output.columns.unwrap_or_default(),
    };
    let template = match template_name(args) {
        Some(name) if args.last && !config.templates.contains_key(LAST) => bail!(
            "there is no session to resume yet: `--last` follows what an interactive \
             `gossip follow` was following when it quit, kept as the template `{name}`"
        ),
        Some(name) => crate::templates::find(config, &name)?.clone(),
        None => SessionSpec::default(),
    };
    let mut settings = asked.or(template).or(config.base()).resolve();
    // What is left in `filter` is the filter of the config file: the
    // template's, or the one of the defaults.
    let filter = FilterState {
        base: settings.filter.take().filter(|_| !args.filter.no_filter),
        rules: LineRules::from_args(&args.filter)?,
    };
    settings.filter = filter.filter();
    if settings.targets.is_empty()
        && let Some(name) = template_name(args)
    {
        bail!("the template `{name}` has no targets");
    }
    Ok((settings, filter))
}

/// The template a run starts from: the one `--template` names, or with
/// `--last` the one that holds the session as it was when Gossip last quit.
fn template_name(args: &FollowArgs) -> Option<String> {
    args.template
        .clone()
        .or_else(|| args.last.then(|| LAST.to_string()))
}

/// The filter the flags stand for: every one of them must hold for a line.
/// The flags are about lines; events pass them, and `--no-events` is what
/// leaves events out.
#[cfg(test)]
fn filter(args: &FollowArgs) -> anyhow::Result<Option<gossip_core::Filter>> {
    Ok(LineRules::from_args(&args.filter)?.filter())
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
pub struct Printer {
    mode: Mode,
}

enum Mode {
    Text(TextRenderer),
    Json,
    Raw,
}

/// A record as it is printed, without its line ending.
#[derive(Debug, PartialEq, Eq)]
pub enum Rendered {
    /// A line of the output.
    Out(String),
    /// Something beside the output: a notice of the raw output, which is
    /// the lines and nothing else, so what the core has to say goes to
    /// stderr.
    Aside(String),
}

impl Printer {
    /// A printer of text lines.
    pub fn text_with(options: TextOptions) -> Printer {
        Printer {
            mode: Mode::Text(TextRenderer::new(options)),
        }
    }

    /// A printer of one JSON object per record.
    #[cfg(test)]
    pub fn json() -> Printer {
        Printer { mode: Mode::Json }
    }

    /// A printer of the raw lines.
    #[cfg(test)]
    pub fn raw() -> Printer {
        Printer { mode: Mode::Raw }
    }

    /// The printer the output flags ask for, with the fields, the output
    /// and the columns as the config file and the flags settle them.
    pub fn new(args: &OutputFlags, settings: &Settings, stdout_is_terminal: bool) -> Self {
        if args.raw {
            return Printer { mode: Mode::Raw };
        }
        if settings.output == Output::Json {
            return Printer { mode: Mode::Json };
        }
        Printer::text_with(TextOptions {
            colour: stdout_is_terminal && !args.no_color,
            fields: FieldChoice::from_setting(settings.fields.as_deref()),
            // Emoji only where they will show: a terminal that says
            // it speaks UTF-8.
            emoji: !args.no_emoji && stdout_is_terminal && locale_is_utf8(),
            zone: Zone::Local,
            columns: settings.columns,
        })
    }

    /// A record in the rendering that was asked for.
    pub fn render(&mut self, record: &Record) -> anyhow::Result<Rendered> {
        Ok(match &mut self.mode {
            Mode::Text(renderer) => Rendered::Out(renderer.render(record)),
            Mode::Json => {
                Rendered::Out(json_line(record).context("cannot write a record as JSON")?)
            }
            Mode::Raw if record.kind == Kind::Notice => {
                Rendered::Aside(format!("-- {}", record.message))
            }
            Mode::Raw => Rendered::Out(record.raw.clone()),
        })
    }

    /// Prints a record to stdout, or what goes beside the output to stderr.
    pub fn print(&mut self, record: &Record) -> anyhow::Result<()> {
        match self.render(record)? {
            Rendered::Out(line) => writeln!(io::stdout().lock(), "{line}")?,
            Rendered::Aside(line) => eprintln!("{line}"),
        }
        Ok(())
    }

    /// The output as a template names it: JSON, or none for the text
    /// output, which is the default, and for the raw output, which only a
    /// flag asks for.
    pub fn output(&self) -> Option<Output> {
        matches!(self.mode, Mode::Json).then_some(Output::Json)
    }

    /// The renderer of the text output; none for JSON and raw output,
    /// which have no fields and no columns to choose.
    pub fn text(&mut self) -> Option<&mut TextRenderer> {
        match &mut self.mode {
            Mode::Text(renderer) => Some(renderer),
            Mode::Json | Mode::Raw => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use clap::Parser;
    use gossip_core::{Filter, FormatChoice, FormatSetting, Level};

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
        assert_eq!(settings.columns, gossip_core::Columns::default());
    }

    #[test]
    fn the_columns_flag_goes_over_the_template_and_the_config_file() {
        use gossip_core::Cut;

        let config = Config::from_toml(
            r#"
            [columns]
            pod = { width = 30, cut = "start" }
            container = { width = 8 }

            [templates.t]
            targets = ["flowcpt"]
            columns = { pod = { cut = "end" }, level = { width = 0 } }
            "#,
        )
        .unwrap();
        let columns =
            |arguments: &[&str]| settings(&follow_args(arguments), &config).unwrap().columns;

        // Without a template: the table of the config file over the built-in
        // columns.
        let plain = columns(&["flowcpt"]);
        assert_eq!((plain.pod.width, plain.pod.cut), (30, Cut::Start));
        assert_eq!((plain.container.width, plain.container.cut), (8, Cut::End));
        assert_eq!(plain.level.width, 5);

        // The template over that, and the flag over both, part by part.
        let template = columns(&["--template", "t"]);
        assert_eq!((template.pod.width, template.pod.cut), (30, Cut::End));
        assert_eq!(template.level.width, 0);
        let flag = columns(&["--template", "t", "--columns", "pod=12,level=5:none"]);
        assert_eq!((flag.pod.width, flag.pod.cut), (12, Cut::End));
        assert_eq!((flag.level.width, flag.level.cut), (5, Cut::None));
        assert_eq!(flag.container.width, 8);
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
        // A filter flag does not override: it narrows. A record must pass
        // the filter of the template and the filter of the flags.
        assert_eq!(
            settings.filter,
            Some(Filter::All(vec![
                Filter::level_at_least(Level::Info),
                Filter::Any(vec![
                    Filter::kind_is(Kind::Event),
                    Filter::level_at_least(Level::Error),
                ]),
            ]))
        );
        // What no flag names stays as the template has it.
        assert_eq!(settings.targets.len(), 2);
        assert_eq!(settings.format.for_container("api"), FormatChoice::Json);
    }

    #[test]
    fn no_filter_drops_the_filter_of_the_template_before_the_flags_apply() {
        // Alone, it leaves no filter at all.
        let settings = settings(
            &follow_args(&["--template", "dev-api", "--no-filter"]),
            &config(),
        )
        .unwrap();
        assert_eq!(settings.filter, None);
        // The rest of the template stays.
        assert_eq!(settings.targets.len(), 2);
        assert!(settings.container.unwrap().matches("api"));

        // With a filter flag, that flag is the whole filter: it replaces.
        let settings = super::settings(
            &follow_args(&["--template", "dev-api", "--no-filter", "--grep", "expired"]),
            &config(),
        )
        .unwrap();
        assert_eq!(
            settings.filter,
            Some(Filter::Any(vec![
                Filter::kind_is(Kind::Event),
                Filter::message_contains("expired"),
            ]))
        );
    }

    #[test]
    fn the_filter_of_the_defaults_is_narrowed_and_dropped_in_the_same_way() {
        let config = Config::from_toml(
            r#"
            [defaults]
            filter = { field = "f.tenant_id", op = "==", value = "t_123" }

            [templates.plain]
            targets = ["flowcpt"]
            "#,
        )
        .unwrap();
        let tenant = Filter::field_equals("tenant_id", "t_123").unwrap();
        let warn = Filter::Any(vec![
            Filter::kind_is(Kind::Event),
            Filter::level_at_least(Level::Warn),
        ]);
        let filter_of =
            |arguments: &[&str]| settings(&follow_args(arguments), &config).unwrap().filter;

        // With a template that has no filter of its own, and with none.
        for start in [&["--template", "plain"][..], &["flowcpt"]] {
            let with = |more: &[&str]| filter_of(&[start, more].concat());
            assert_eq!(with(&[]), Some(tenant.clone()));
            assert_eq!(
                with(&["--level", "warn"]),
                Some(Filter::All(vec![tenant.clone(), warn.clone()]))
            );
            assert_eq!(with(&["--no-filter"]), None);
            assert_eq!(
                with(&["--no-filter", "--level", "warn"]),
                Some(warn.clone())
            );
        }
    }

    #[test]
    fn a_line_with_no_level_passes_the_level_flag() {
        use chrono::Utc;
        use gossip_core::{Format, Record};
        use std::collections::BTreeMap;

        let filter = filter(&args(&["--level", "error"])).unwrap().unwrap();
        let line = |level: Option<Level>| Record {
            cluster: "c".into(),
            namespace: "flowcpt".into(),
            pod: "p".into(),
            container: Some("api".into()),
            time: Utc::now(),
            kind: Kind::Line,
            format: Format::Text,
            level,
            message: "thread 'main' panicked at src/main.rs:5:5:".into(),
            fields: BTreeMap::new(),
            raw: String::new(),
            previous: false,
            truncated: false,
        };
        assert!(filter.accepts(&line(None)));
        assert!(filter.accepts(&line(Some(Level::Error))));
        assert!(!filter.accepts(&line(Some(Level::Warn))));
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
    fn last_is_the_template_of_that_name() {
        let config = Config::from_toml(
            r#"
            [templates.last]
            targets = ["fc-stg/fc-api"]
            filter = { field = "level", op = ">=", value = "warn" }
            fields = ["tenant_id"]
            columns = { pod = { width = 40 } }
            "#,
        )
        .unwrap();
        let from_last = settings(&follow_args(&["--last"]), &config).unwrap();
        let from_template = settings(&follow_args(&["--template", "last"]), &config).unwrap();
        assert_eq!(from_last, from_template);
        assert_eq!(from_last.targets, ["fc-stg/fc-api".parse().unwrap()]);
        assert_eq!(from_last.filter, Some(Filter::level_at_least(Level::Warn)));
        assert_eq!(from_last.columns.pod.width, 40);
        // A flag beside it overrides, and a filter flag narrows, as beside
        // any template.
        let narrowed = settings(&follow_args(&["--last", "--grep", "x"]), &config).unwrap();
        assert!(matches!(narrowed.filter, Some(Filter::All(parts)) if parts.len() == 2));

        // Before any session was kept there is nothing to resume.
        let error = settings(&follow_args(&["--last"]), &Config::default()).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("there is no session to resume yet"),
            "{error}"
        );
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
            match Printer::new(&args.output, &settings, terminal).mode {
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
