//! `cargo xtask`: the tasks of the repository that are not the program.

use std::process::ExitCode;

use anyhow::Context;
use clap::{Parser, Subcommand};
use xtask::readme;

/// The tasks of the repository that are not the program itself.
#[derive(Debug, Parser)]
#[command(name = "cargo xtask", arg_required_else_help = true)]
struct Tasks {
    #[command(subcommand)]
    task: Task,
}

#[derive(Debug, Subcommand)]
enum Task {
    /// Write the generated parts of README.md: the version in the status
    /// line, the sample, the Install section, what the walkthrough prints
    /// and the Reference. The walkthrough is played by the built binary
    /// against the stand-in API server.
    Readme {
        /// Write nothing: say what in README.md is not what the code says,
        /// and fail if anything is.
        #[arg(long)]
        check: bool,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let tasks = Tasks::parse();
    let result = match tasks.task {
        Task::Readme { check } => write_readme(check).await,
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("cargo xtask: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn write_readme(check: bool) -> anyhow::Result<ExitCode> {
    let path = readme::repository().join("README.md");
    let before = std::fs::read_to_string(&path)
        .with_context(|| format!("{} could not be read", path.display()))?;
    let binary = readme::build_binary()?;
    let blocks = readme::generate(&binary).await?;

    let text = if check {
        before.clone()
    } else {
        let text = readme::splice(&before, &blocks)?;
        if text == before {
            println!("README.md is up to date: nothing was written");
        } else {
            std::fs::write(&path, &text)
                .with_context(|| format!("{} could not be written", path.display()))?;
            let changed: Vec<&str> = blocks
                .iter()
                .filter(|(name, content)| readme::block(&before, name) != Some(content.as_str()))
                .map(|(name, _)| name.as_str())
                .collect();
            println!("README.md: wrote {}", changed.join(", "));
        }
        text
    };

    // What the generator does not write is still held against the spec.
    let problems = readme::check(&text, &blocks);
    for problem in &problems {
        eprintln!("README.md: {problem}");
    }
    Ok(if problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
