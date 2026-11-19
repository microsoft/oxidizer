// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// CLI behavior remains tested, but interactive presentation is not a required coverage metric.
#![cfg_attr(coverage_nightly, coverage(off))]
#![deny(unsafe_code)]
#![expect(
    clippy::renamed_function_params,
    reason = "Display implementations use descriptive formatter names"
)]

//! The Seismograph CLI shows thread activity, task execution, and memory allocations in your terminal.
//! Run `seismograph monitor` to inspect a running application.
//! To open a saved snapshot, run `seismograph view capture.seismograph`.

mod allocation_identities;
mod allocator_sources;
mod allocator_topology;
mod allocator_view;
mod commands;
mod mimalloc_view;
mod snapshot_error;
mod snapshot_symbols;

#[cfg(test)]
mod native_view {
    pub(crate) mod fixture;
}

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
}

/// Runs the command-line interface with explicit process arguments.
///
/// The first argument names the executable. Commands use the host terminal
/// and filesystem. Errors are printed with the same format as the executable.
#[must_use]
pub fn execute(args: Vec<OsString>) -> ExitCode {
    let program = program_name(&args).to_owned();
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            // clap returns 0 or 2, so the conversion fallback cannot be selected.
            // #[gamma::skip(fallback.unwrap_or_to_default, literal.int_decrement, literal.int_increment)]
            return ExitCode::from(u8::try_from(code).unwrap_or(1));
        }
    };
    ExitCode::from(exit_code(run(cli, commands::monitor::verb, commands::monitor::view), &program))
}

fn program_name(args: &[OsString]) -> &str {
    args.first()
        .and_then(|value| std::path::Path::new(value).file_stem())
        .and_then(|value| value.to_str())
        .unwrap_or("seismograph")
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
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command, run};

    #[test]
    fn diagnostics_use_the_executable_stem_not_a_command_argument() {
        let executable = std::path::Path::new("tools").join("renamed.exe").into_os_string();
        let args = [executable, "view".into(), "capture.seismograph".into()];

        assert_eq!(super::program_name(&args), "renamed");
        assert_eq!(super::program_name(&[]), "seismograph");
    }

    #[test]
    fn removed_html_export_is_not_a_command() {
        use clap::Parser;

        let error = Cli::try_parse_from(["seismograph", "snapshot", "html", "capture.seismograph"])
            .err()
            .unwrap();

        assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn help_version_and_unknown_commands_use_standard_parser_results() {
        use clap::Parser;

        for (argument, expected) in [
            ("--help", clap::error::ErrorKind::DisplayHelp),
            ("--version", clap::error::ErrorKind::DisplayVersion),
            ("unknown", clap::error::ErrorKind::InvalidSubcommand),
        ] {
            let error = Cli::try_parse_from(["seismograph", argument]).err().unwrap();
            assert_eq!(error.kind(), expected);
        }
    }

    #[test]
    #[cfg_attr(coverage_nightly, coverage(off))] // Wrong-command panic callbacks are test-only failure paths.
    fn monitor_command_dispatches_and_propagates_failures() {
        let result = run(
            Cli {
                command: Command::Monitor(crate::commands::monitor::VerbArgs { terminal_error: None }),
            },
            |_| Ok(()),
            |_| panic!("monitor must not dispatch view"),
        );
        assert_eq!(super::exit_code(result, "seismograph"), 0);

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
    #[cfg_attr(coverage_nightly, coverage(off))] // Wrong-command panic callbacks are test-only failure paths.
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
