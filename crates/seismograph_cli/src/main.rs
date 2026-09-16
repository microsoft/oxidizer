// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![expect(
    clippy::renamed_function_params,
    reason = "Display implementations use descriptive formatter names"
)]

//! Live monitoring, interactive snapshot viewing, and HTML reporting for `seismograph`.
//!
//! The CLI renders common thread, stack, and runtime-event data directly.
//! Rallocator payloads use the built-in schema-specific renderer; unknown
//! sources remain visible in the source inventory.
//!
//! Run `seismograph monitor` to capture a running application, or
//! `seismograph view "C:\captures\capture.seismograph"` to inspect a native
//! snapshot in the same interactive tabs without connecting to a process.
//! The offline viewer is read-only: use `Tab` or `1` through `8` to select tabs,
//! the usual arrow/Enter/Backspace navigation within tabs, and `q` or `Esc` to quit.
//! Large files load on a worker thread while the terminal remains responsive.
//! Loading still requires memory for the decoded events and their summaries.
//! Snapshot files do not record a wall-clock capture time.

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
    /// Opens a saved native snapshot in the read-only interactive viewer.
    View(commands::monitor::ViewArgs),
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
    ExitCode::from(exit_code(run(cli, commands::monitor::verb, commands::monitor::view), &program))
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
    run_view: impl FnOnce(commands::monitor::ViewArgs) -> Result<(), commands::monitor::Error>,
) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Command::Monitor(args) => run_monitor(args).map_err(Into::into),
        Command::View(args) => run_view(args).map_err(Into::into),
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
            |_| panic!("monitor must not dispatch view"),
        )
        .unwrap();

        let error = run(
            Cli {
                command: Command::Monitor(crate::commands::monitor::VerbArgs { terminal_error: None }),
            },
            |_| Err(crate::commands::monitor::Error::UnexpectedResponse),
            |_| panic!("monitor must not dispatch view"),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "monitor returned an unexpected response");
    }

    #[test]
    fn view_parses_and_dispatches_a_required_path_with_spaces() {
        use clap::Parser;
        let path = r"C:\capture files\blob.seismograph";
        let cli = Cli::try_parse_from(["seismograph", "view", path]).unwrap();
        let error = run(
            cli,
            |_| panic!("view must not dispatch monitor"),
            |args| {
                assert_eq!(args.snapshot_file, std::path::PathBuf::from(path));
                Err(crate::commands::monitor::Error::UnexpectedResponse)
            },
        )
        .unwrap_err();
        assert_eq!(super::exit_code(Err(error), "seismograph"), 2);
        assert_eq!(
            Cli::try_parse_from(["seismograph", "view"]).err().unwrap().kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }
}
