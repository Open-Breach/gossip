//! The `gossip` command line.
//!
//! Nothing in here talks to the cluster: every command asks the core
//! library and presents what it answers.

mod age;
mod cli;
mod list;
mod table;

use std::io;
use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        // The reader went away, as in `gossip ... | head`: not an error.
        Err(error) if is_broken_pipe(&error) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("gossip: {}", error_text(&error));
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::List(args) => list::run(&args, &mut io::stdout().lock()).await,
    }
}

/// An error with its causes in one line. A cause that an outer error has
/// already spelt out is not repeated.
fn error_text(error: &anyhow::Error) -> String {
    let mut text = String::new();
    for cause in error.chain() {
        let cause = cause.to_string();
        if text.is_empty() {
            text = cause;
        } else if !text.contains(&cause) {
            text.push_str(": ");
            text.push_str(&cause);
        }
    }
    text
}

fn is_broken_pipe(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::BrokenPipe)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn an_error_is_printed_with_its_causes_once() {
        let inner = io::Error::new(io::ErrorKind::NotFound, "no such file");
        let error = anyhow::Error::new(inner)
            .context("cannot read the file: no such file")
            .context("cannot start");
        assert_eq!(
            error_text(&error),
            "cannot start: cannot read the file: no such file"
        );
    }

    #[test]
    fn a_broken_pipe_is_found_among_the_causes() {
        let pipe = io::Error::new(io::ErrorKind::BrokenPipe, "pipe closed");
        let error = Err::<(), _>(pipe).context("cannot print").unwrap_err();
        assert!(is_broken_pipe(&error));
        let other = io::Error::new(io::ErrorKind::NotFound, "no such file");
        assert!(!is_broken_pipe(&anyhow::Error::new(other)));
    }
}
