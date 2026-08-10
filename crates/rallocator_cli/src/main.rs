// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![expect(
    clippy::renamed_function_params,
    reason = "Display implementations use descriptive formatter names"
)]

//! Snapshot-to-HTML reporting for the `rallocator_cli` command.

mod commands;
mod report;

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
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
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return ExitCode::from(u8::try_from(code).unwrap_or(1));
        }
    };
    ExitCode::from(exit_code(run(cli)))
}

fn exit_code(result: Result<(), commands::snapshot::html::Error>) -> u8 {
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("rallocator_cli: {error}");
            2
        }
    }
}

fn run(cli: Cli) -> Result<(), commands::snapshot::html::Error> {
    match cli.command {
        Command::Snapshot { command } => match command {
            SnapshotCommand::Html(args) => commands::snapshot::html::verb(args),
        },
    }
}
