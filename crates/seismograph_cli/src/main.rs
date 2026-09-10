// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![expect(
    clippy::renamed_function_params,
    reason = "Display implementations use descriptive formatter names"
)]

//! Live monitoring and snapshot-to-HTML reporting for the `seismograph` command.
//!
//! The CLI renders common thread, stack, and runtime-event data directly.
//! Rallocator payloads use the built-in schema-specific renderer; unknown
//! sources remain visible in the source inventory.

mod commands;
mod report;

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "seismograph", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Opens the live application monitor.
    Monitor(commands::monitor::VerbArgs),
    Snapshot {
        #[command(subcommand)]
        command: SnapshotCommand,
    },
}

#[derive(Subcommand)]
enum SnapshotCommand {
    Html(commands::snapshot::html::VerbArgs),
}

fn main() -> ExitCode {
    execute(std::env::args_os().collect())
}

fn execute(args: Vec<OsString>) -> ExitCode {
    let program = args
        .first()
        .and_then(|value| std::path::Path::new(value).file_stem())
        .and_then(|value| value.to_str())
        .unwrap_or("seismograph")
        .to_owned();
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return ExitCode::from(u8::try_from(code).unwrap_or(1));
        }
    };
    ExitCode::from(exit_code(run(cli, commands::monitor::verb), &program))
}

fn exit_code(result: Result<(), Box<dyn std::error::Error>>, program: &str) -> u8 {
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{program}: {error}");
            2
        }
    }
}

fn run(
    cli: Cli,
    run_monitor: impl FnOnce(commands::monitor::VerbArgs) -> Result<(), commands::monitor::Error>,
) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Command::Monitor(args) => run_monitor(args).map_err(Into::into),
        Command::Snapshot { command } => match command {
            SnapshotCommand::Html(args) => commands::snapshot::html::verb(args).map_err(Into::into),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command, run};

    #[test]
    fn monitor_command_dispatches_and_propagates_failures() {
        run(
            Cli {
                command: Command::Monitor(crate::commands::monitor::VerbArgs { terminal_error: None }),
            },
            |_| Ok(()),
        )
        .unwrap();

        let error = run(
            Cli {
                command: Command::Monitor(crate::commands::monitor::VerbArgs { terminal_error: None }),
            },
            |_| Err(crate::commands::monitor::Error::UnexpectedResponse),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "monitor returned an unexpected response");
    }
}
