//! The command line as `clap` reads it.

use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use gossip_core::{ColumnsSetting, Level, Output, Pattern, Target, parse_duration};

/// Follow Kubernetes pod logs and cluster events across restarts and rollouts.
#[derive(Debug, Parser)]
#[command(name = "gossip", version, about, arg_required_else_help = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List the contexts of the kubeconfig, or the namespaces, deployments
    /// and pods of a cluster.
    #[command(after_help = LIST_FORMS)]
    List(ListArgs),

    /// Follow the logs of a set of targets, across restarts and rollouts.
    #[command(after_help = TARGET_FORMS)]
    Follow(Box<FollowArgs>),

    /// List the templates of the config file, or show one.
    Templates(TemplatesArgs),
}

const TARGET_FORMS: &str = "\
Targets:
  A target is a path namespace[/workload[/pod[/container]]]; a name may be a glob.
  flowcpt                           every pod of the namespace
  flowcpt/flowcpt-api               the pods of a Deployment, StatefulSet, DaemonSet, Job or CronJob
  flowcpt/cronjob:nightly-sync      the same, of one kind only
  flowcpt/-/flowcpt-api-7d9f-xk2p1  one pod, given directly
  monitoring/prometheus/*/exporter  one container of every pod of a workload";

const LIST_FORMS: &str = "\
Forms:
  gossip list contexts                 the contexts of the kubeconfig, the current one marked
  gossip list namespaces               the namespaces of the cluster
  gossip list <namespace>              the deployments of the namespace
  gossip list <namespace>/<deployment> the pods of the deployment";

#[derive(Debug, Args)]
pub struct ListArgs {
    /// `contexts`, `namespaces`, a namespace, or `namespace/deployment`.
    #[arg(value_name = "WHAT")]
    pub what: String,

    /// The kubeconfig context of the cluster [default: the current context].
    #[arg(long, value_name = "NAME")]
    pub context: Option<String>,

    /// Print JSON instead of a table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct FollowArgs {
    /// What to follow; see the forms below.
    #[arg(
        value_name = "TARGET",
        required_unless_present = "template",
        conflicts_with = "template"
    )]
    pub targets: Vec<Target>,

    /// Follow what a template of the config file says. Flags given beside
    /// it override its values; the filter flags narrow its filter.
    #[arg(long, value_name = "NAME")]
    pub template: Option<String>,

    /// Drop the filter of the template, or of the defaults, so that only
    /// the filter flags apply.
    #[arg(long)]
    pub no_filter: bool,

    /// The kubeconfig context of the cluster [default: the current context].
    #[arg(long, value_name = "NAME")]
    pub context: Option<String>,

    /// History per container at start, as a duration: 30s, 5m, 1h.
    #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
    pub since: Option<Duration>,

    /// History per container at start, as a number of lines [default: 20,
    /// unless --since is given].
    #[arg(long, value_name = "N")]
    pub tail: Option<u64>,

    /// Leave the cluster's events out of the stream.
    #[arg(long)]
    pub no_events: bool,

    /// Only containers whose name matches the glob.
    #[arg(long, value_name = "GLOB")]
    pub container: Option<Pattern>,

    /// Only lines at this level or above: trace, debug, info, warn, error.
    #[arg(long, value_name = "LEVEL")]
    pub level: Option<Level>,

    /// Only lines whose message contains the text.
    #[arg(long, value_name = "TEXT")]
    pub grep: Option<String>,

    /// Only lines whose message matches the regular expression.
    #[arg(long = "match", value_name = "REGEX")]
    pub matches: Option<String>,

    /// Only lines whose field has the value; can be given several times.
    #[arg(long = "field", value_name = "KEY=VALUE")]
    pub fields_equal: Vec<String>,

    /// Print the raw line, with no prefix, parsing or colour.
    #[arg(long)]
    pub raw: bool,

    /// The rendering: text, or json with one record per line.
    #[arg(long, value_name = "text|json")]
    pub output: Option<Output>,

    /// No colours [default: colours when stdout is a terminal].
    #[arg(long)]
    pub no_color: bool,

    /// Show only these fields of a line, in this order: a,b,c. `all` shows
    /// every field. Without it: the fields a request is read by first, the
    /// rest in alphabetical order, and the noisy ones hidden.
    #[arg(long, value_name = "KEYS", value_delimiter = ',')]
    pub fields: Option<Vec<String>>,

    /// The widths of the columns time, pod, container and level, in terminal
    /// cells, and the side a longer value is cut on (start, end, middle or
    /// none): pod=24:middle,container=8. A width of 0 hides the column
    /// [default: time=12:none,pod=20:middle,container=10:end,level=5:end].
    #[arg(long, value_name = "COLUMNS")]
    pub columns: Option<ColumnsSetting>,

    /// Mark events with `EVNT` instead of an emoji [default: emoji when
    /// stdout is a terminal and the locale is UTF-8].
    #[arg(long)]
    pub no_emoji: bool,

    /// Read keys while following, also when stdin is not a terminal: Space
    /// pauses, `:` opens a command line, `?` lists the keys and commands.
    /// Stdout must be a terminal [default: on when stdin and stdout are
    /// both a terminal].
    #[arg(long, conflicts_with = "no_interactive")]
    pub interactive: bool,

    /// Read no keys: follow and print, and nothing else.
    #[arg(long)]
    pub no_interactive: bool,
}

#[derive(Debug, Args)]
pub struct TemplatesArgs {
    #[command(subcommand)]
    pub command: Option<TemplatesCommand>,
}

#[derive(Debug, Subcommand)]
pub enum TemplatesCommand {
    /// Print one template as it stands in the config file.
    Show {
        /// The name of the template.
        #[arg(value_name = "NAME")]
        name: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn list_takes_what_a_context_and_json() {
        let cli = Cli::try_parse_from([
            "gossip",
            "list",
            "flowcpt/flowcpt-api",
            "--context",
            "aks-vulcan-01",
            "--json",
        ])
        .unwrap();
        let Command::List(args) = cli.command else {
            panic!("expected the list command");
        };
        assert_eq!(args.what, "flowcpt/flowcpt-api");
        assert_eq!(args.context.as_deref(), Some("aks-vulcan-01"));
        assert!(args.json);

        let cli = Cli::try_parse_from(["gossip", "list", "contexts"]).unwrap();
        let Command::List(args) = cli.command else {
            panic!("expected the list command");
        };
        assert_eq!(args.context, None);
        assert!(!args.json);
    }

    #[test]
    fn list_needs_to_know_what() {
        assert!(Cli::try_parse_from(["gossip", "list"]).is_err());
    }

    fn follow(args: &[&str]) -> Result<FollowArgs, clap::Error> {
        let cli = Cli::try_parse_from(["gossip", "follow"].iter().chain(args))?;
        match cli.command {
            Command::Follow(args) => Ok(*args),
            other => panic!("expected the follow command, got {other:?}"),
        }
    }

    #[test]
    fn follow_takes_targets_and_every_flag_of_the_spec() {
        let args = follow(&[
            "flowcpt/flowcpt-api",
            "monitoring/prometheus/*/exporter",
            "--context",
            "aks-vulcan-01",
            "--since",
            "5m",
            "--tail",
            "100",
            "--container",
            "api*",
            "--level",
            "warn",
            "--grep",
            "expired",
            "--match",
            "^payment",
            "--field",
            "tenant_id=t_123",
            "--field",
            "status=200",
            "--raw",
            "--output",
            "json",
            "--no-color",
            "--fields",
            "tenant_id,request_id",
            "--no-events",
            "--no-emoji",
            "--no-filter",
            "--columns",
            "pod=24:middle,container=8",
            "--no-interactive",
        ])
        .unwrap();
        assert_eq!(args.targets.len(), 2);
        assert_eq!(args.targets[0].to_string(), "flowcpt/flowcpt-api");
        assert_eq!(args.context.as_deref(), Some("aks-vulcan-01"));
        assert_eq!(args.since, Some(Duration::from_secs(300)));
        assert_eq!(args.tail, Some(100));
        assert!(args.container.unwrap().matches("api-2"));
        assert_eq!(args.level, Some(Level::Warn));
        assert_eq!(args.grep.as_deref(), Some("expired"));
        assert_eq!(args.matches.as_deref(), Some("^payment"));
        assert_eq!(args.fields_equal, ["tenant_id=t_123", "status=200"]);
        assert!(args.raw);
        assert_eq!(args.output, Some(Output::Json));
        assert!(args.no_color);
        assert_eq!(
            args.fields,
            Some(vec!["tenant_id".to_string(), "request_id".to_string()])
        );
        assert!(args.no_events);
        assert!(args.no_emoji);
        assert!(args.no_filter);
        assert_eq!(
            args.columns,
            Some("pod=24:middle,container=8".parse().unwrap())
        );
        assert!(args.no_interactive && !args.interactive);
    }

    #[test]
    fn interactive_can_be_forced_or_turned_off_but_not_both() {
        let args = follow(&["flowcpt"]).unwrap();
        assert!(!args.interactive && !args.no_interactive);
        assert!(follow(&["flowcpt", "--interactive"]).unwrap().interactive);
        assert!(follow(&["flowcpt", "--interactive", "--no-interactive"]).is_err());
    }

    #[test]
    fn follow_with_nothing_but_a_target() {
        let args = follow(&["flowcpt"]).unwrap();
        assert_eq!(args.context, None);
        assert_eq!(args.since, None);
        assert_eq!(args.tail, None);
        assert_eq!(args.output, None);
        assert_eq!(args.fields, None);
        assert_eq!(args.columns, None);
        assert!(!args.raw && !args.no_color);
        assert!(!args.no_events && !args.no_emoji && !args.no_filter);
    }

    #[test]
    fn follow_takes_a_template_in_place_of_targets() {
        let args = follow(&["--template", "dev-api", "--tail", "50"]).unwrap();
        assert_eq!(args.template.as_deref(), Some("dev-api"));
        assert!(args.targets.is_empty());
        assert_eq!(args.tail, Some(50));
        // One or the other, not both and not neither.
        assert!(follow(&["flowcpt", "--template", "dev-api"]).is_err());
        assert!(follow(&["--tail", "50"]).is_err());
    }

    #[test]
    fn templates_lists_or_shows_one() {
        let parse = |args: &[&str]| {
            let cli = Cli::try_parse_from(["gossip", "templates"].iter().chain(args)).unwrap();
            match cli.command {
                Command::Templates(args) => args.command,
                other => panic!("expected the templates command, got {other:?}"),
            }
        };
        assert!(parse(&[]).is_none());
        let Some(TemplatesCommand::Show { name }) = parse(&["show", "dev-api"]) else {
            panic!("expected show");
        };
        assert_eq!(name, "dev-api");
        assert!(Cli::try_parse_from(["gossip", "templates", "show"]).is_err());
    }

    #[test]
    fn follow_refuses_what_it_cannot_read() {
        assert!(follow(&[]).is_err());
        assert!(follow(&["a/b/c/d/e"]).is_err());
        assert!(follow(&["flowcpt", "--since", "soon"]).is_err());
        assert!(follow(&["flowcpt", "--tail", "many"]).is_err());
        assert!(follow(&["flowcpt", "--level", "loud"]).is_err());
        assert!(follow(&["flowcpt", "--output", "xml"]).is_err());
        assert!(follow(&["flowcpt", "--container", "a b"]).is_err());
        assert!(follow(&["flowcpt", "--columns", "node=24"]).is_err());
        assert!(follow(&["flowcpt", "--columns", "pod=wide"]).is_err());
        assert!(follow(&["flowcpt", "--columns", "pod=24:left"]).is_err());
        assert!(follow(&["flowcpt", "--columns", "pod"]).is_err());
        // Pod names are never shortened, so the flag that kept them whole is
        // gone.
        assert!(follow(&["flowcpt", "--long-names"]).is_err());
    }
}
