//! The `gossip` command line.

use clap::{CommandFactory, Parser};

/// Follow Kubernetes pod logs and cluster events across restarts and rollouts.
#[derive(Debug, Parser)]
#[command(name = "gossip", version, about)]
struct Cli {}

fn main() {
    Cli::parse();
    // No commands exist yet, so a bare `gossip` shows the help.
    Cli::command()
        .print_help()
        .expect("the help text can be written to stdout");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }
}
