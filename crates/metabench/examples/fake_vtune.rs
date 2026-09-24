// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A minimal stand-in for the real `vtune` CLI, used only by
//! `tests/spawned_benchmark.rs` to exercise `runner::launch_vtune_worker` and
//! `vtune`'s control protocol on every platform (a real `VTune` install is
//! Linux/Windows-only and not available in this repository's test
//! environment). Copied to `vtune`/`vtune.exe` on a temporary `PATH` entry so
//! it stands in for the real binary.
//!
//! Understood invocations, mirroring exactly what `runner::launch_vtune_worker`
//! and `vtune::run_command` issue:
//! - `vtune -command <resume|pause> -r <result-dir>`: control-protocol calls.
//!   Any other verb fails with a distinct exit code so tests can tell a
//!   misdirected control call apart from a genuine failure. Set
//!   `FAKE_VTUNE_FAIL_COMMAND=<verb>` to make that verb fail instead, and set
//!   `VTUNE_TEST_COMMAND_LOG=<path>` to append every accepted `<verb>
//!   <result-dir>` line for tests to assert against.
//! - `vtune -report hw-events -r <dir> -format csv -report-output <path>`:
//!   writes a small, fixed hardware-event CSV report to `<path>` and prints a
//!   marker line to stderr so tests can check whether the worker suppressed
//!   this command's own output.
//! - anything else: the initial `--start-paused` collection launch. Validates
//!   the `-collect-with runsa [<forwarded --vtune-arg values>] --start-paused
//!   -result-dir <dir>` shape before `--`, then execs everything after the
//!   first `--` (the wrapped benchmark executable and its arguments) and
//!   forwards its exit code, standing in for `vtune` actually running the
//!   workload.

use std::env;
use std::io::Write as _;
use std::process::{Command, ExitCode};

const REJECTED_COMMAND_EXIT_CODE: u8 = 97;

fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("-command") => run_control_command(&arguments),
        Some("-report") => run_report(&arguments),
        _ => run_wrapped_workload(&arguments),
    }
}

fn run_control_command(arguments: &[String]) -> ExitCode {
    let Some(verb) = arguments.get(1) else {
        return ExitCode::FAILURE;
    };
    let result_dir = arguments.get(3).map_or("", String::as_str);
    if verb != "resume" && verb != "pause" {
        return ExitCode::from(REJECTED_COMMAND_EXIT_CODE);
    }
    if env::var("FAKE_VTUNE_FAIL_COMMAND").ok().as_deref() == Some(verb.as_str()) {
        return ExitCode::FAILURE;
    }
    if let Ok(log_path) = env::var("VTUNE_TEST_COMMAND_LOG")
        && let Ok(mut log) = std::fs::OpenOptions::new().create(true).append(true).open(log_path)
    {
        let _ = writeln!(log, "{verb} {result_dir}");
    }
    ExitCode::SUCCESS
}

fn run_report(arguments: &[String]) -> ExitCode {
    // Validate the full invocation shape (`-report hw-events -r <dir> -format
    // csv -report-output <path>`), not just `-report-output`, so a
    // regression in how `launch_vtune_worker` builds this command is caught
    // by the tests that exercise this fixture rather than silently ignored.
    if arguments.first().map(String::as_str) != Some("-report") || arguments.get(1).map(String::as_str) != Some("hw-events") {
        return ExitCode::FAILURE;
    }
    let mut values = arguments.iter();
    let Some(_result_dir) = values.find(|value| *value == "-r").and_then(|_| values.next()) else {
        return ExitCode::FAILURE;
    };
    if values
        .find(|value| *value == "-format")
        .and_then(|_| values.next())
        .map(String::as_str)
        != Some("csv")
    {
        return ExitCode::FAILURE;
    }
    let Some(output) = values.find(|value| *value == "-report-output").and_then(|_| values.next()) else {
        return ExitCode::FAILURE;
    };
    eprintln!("vtune-report-invoked");
    // Mirrors the full column set a real `vtune -report hw-events -format
    // csv` report includes, not just the two columns this crate reads, so
    // tests exercise the header-driven column lookup in `artifact::parse_vtune`.
    let report = concat!(
        "Hardware Event Sample Count:Self,Hardware Event Type,Events Per Sample,Hardware Event Count:Self,Precise:Self\n",
        "1,INST_RETIRED.ANY,1234.0,1234,Yes\n",
        "1,CPU_CLK_UNHALTED.THREAD,567.0,567,Yes\n",
    );
    if std::fs::write(output, report).is_err() {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run_wrapped_workload(arguments: &[String]) -> ExitCode {
    let Some(separator) = arguments.iter().position(|argument| argument == "--") else {
        return ExitCode::FAILURE;
    };
    // Validate the collection-launch shape `launch_vtune_worker` builds
    // (`-collect-with runsa [<forwarded --vtune-arg values>] --start-paused
    // -result-dir <dir>`), not just that a `--` separator exists, so a
    // regression in how that command is assembled is caught by the tests
    // that exercise this fixture rather than silently ignored.
    let launch_arguments = &arguments[..separator];
    if launch_arguments.first().map(String::as_str) != Some("-collect-with") || launch_arguments.get(1).map(String::as_str) != Some("runsa")
    {
        return ExitCode::FAILURE;
    }
    let Some(start_paused_index) = launch_arguments.iter().position(|argument| argument == "--start-paused") else {
        return ExitCode::FAILURE;
    };
    if launch_arguments.get(start_paused_index + 1).map(String::as_str) != Some("-result-dir")
        || launch_arguments.get(start_paused_index + 2).is_none()
        || start_paused_index + 2 != launch_arguments.len() - 1
    {
        // `-result-dir <dir>` must be the last two arguments before `--`: a
        // trailing argument after the result directory would mean
        // `launch_vtune_worker` assembled something other than the expected
        // shape, and this fixture is meant to catch that regression.
        return ExitCode::FAILURE;
    }
    let Some((program, workload_arguments)) = arguments[separator + 1..].split_first() else {
        return ExitCode::FAILURE;
    };
    eprintln!("vtune-worker-invoked");
    match Command::new(program).args(workload_arguments).status() {
        Ok(status) => u8::try_from(status.code().unwrap_or(1)).map_or(ExitCode::FAILURE, ExitCode::from),
        Err(_error) => ExitCode::FAILURE,
    }
}
