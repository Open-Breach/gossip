//! The command line as `clap` reads it.

use clap::{Args, Parser, Subcommand};

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
}

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
        let Command::List(args) = cli.command;
        assert_eq!(args.what, "flowcpt/flowcpt-api");
        assert_eq!(args.context.as_deref(), Some("aks-vulcan-01"));
        assert!(args.json);

        let cli = Cli::try_parse_from(["gossip", "list", "contexts"]).unwrap();
        let Command::List(args) = cli.command;
        assert_eq!(args.context, None);
        assert!(!args.json);
    }

    #[test]
    fn list_needs_to_know_what() {
        assert!(Cli::try_parse_from(["gossip", "list"]).is_err());
    }
}
