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
//!
//! Press `F1` for contextual help on the current panel or dialog, including
//! column meanings, units, metric scope, and keyboard and mouse controls.
//! Help is scrollable and leaves the underlying selection and drafts unchanged.
//! Press `F1` or `Esc` to close it.
//!
//! Press uppercase `F` in either viewer to filter whole records by any complete
//! captured stack frame; lowercase `f` still only changes displayed stack frames.
//! Enter comma/whitespace-separated `crate:name`, `module:crate::module`, or
//! `function:crate::module::name` rules. Any include can match; exclusions win.
//! Matching uses symbol-path segments (not substring matches), ignores generic
//! type arguments and hashes, and attributes qualified impl methods to their
//! implementing type. Symbol ownership is not true execution lineage.
//! Missing or unresolved frames produce an Unknown decision when the available
//! frames cannot decide the rules; explicitly choose whether to show those records.
//! Enable backtrace recording before taking a new capture to select by code;
//! filtering cannot recover stacks omitted from an existing recording.
//! Runtime records can use their event stack or task spawn provenance.
//! Runtime task metadata uses the spawn stack; matching events in event mode can
//! also retain their associated task metadata. Spawn mode applies task spawn
//! provenance to associated events. I/O operations match the union of captured
//! frames from their start and completion: an include can match either side,
//! while a known exclusion on either side excludes the entire operation.
//! The whole pair is kept or removed, so filtering cannot invent unfinished I/O.
//! Retained allocations and hotspots use allocation-stack provenance, with the
//! complete pre-filter deallocation set preserving lifetimes rather than inventing leaks.
//! Use Tab/up/down to select fields, type at the end of rule fields, and use
//! Backspace/Delete to remove the last character. Left/right/space changes options.
//! Enter applies asynchronously; Esc cancels the draft. Empty rules show everything,
//! independently of the unknown-stack option. Applied rules persist across live
//! captures. Only one filter worker runs at a time; newer requests replace the
//! queued request while the current worker finishes, including after reconnect.
//! Filtering never rewrites the original file; source accepted/overwritten
//! counters, whole-process counters, and heap topology remain unfiltered, as
//! indicated in the filter banner.
//!
//! Drag a shared panel border with the left mouse button to resize the panes.
//! Click a tab header (Info, Heaps, and so on) to select that tab.
//! Click a list row to select and activate it, as with keyboard selection and Enter.
//! Sizes are retained per tab for the current monitor or viewer session.
//! The Threads tab includes same-thread object activity, marked `(self)`.
//! Channel receive waits indicate an empty channel, not lock contention; they
//! remain visible as events but are excluded from contention totals and highlighting.
//! In the recording configuration, `on` records every event with backtraces;
//! select `custom` to disable backtraces or change sampling.
//! Configuration changes are read back from the application, and the live state
//! is refreshed without replacing an open configuration draft.
//! Runtime tasks without a known worker appear in an `unassigned` group.
//! An empty Runtime tab distinguishes absent instrumentation from absent activity:
//! enabling recording does not install runtime instrumentation in the application.
//! With active filters, an empty Runtime tab instead reports no matching runtime
//! activity and points back to the filter controls.

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
    #[cfg_attr(coverage_nightly, coverage(off))] // Wrong-command panic callbacks are test-only failure paths.
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
