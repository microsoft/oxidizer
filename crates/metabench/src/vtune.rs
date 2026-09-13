// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Control protocol for measuring exactly the annotated workload under a live
//! `vtune` collection, mirroring [`crate::perf`]'s Linux `perf` integration.
//!
//! Unlike `perf`, which is driven through a pair of FIFOs the worker polls
//! asynchronously, the `VTune` command-line interface exposes external
//! pause/resume control as its own synchronous subcommands run against a
//! result directory (`vtune -command pause|resume -r <result-dir>`; see
//! Intel's `VTune` Profiler CLI documentation and its collection-control
//! guidance at
//! <https://www.intel.com/content/www/us/en/docs/vtune-profiler/user-guide/current/command-line-interface.html>
//! and the ITT/JIT collection-control API docs at
//! <https://intel.github.io/ittapi/src/ittapi/collection-control-api.html>).
//! `runner::launch_vtune_worker` starts the collection with `--start-paused`
//! so nothing is measured until this module's [`begin`] resumes it, and this
//! module's [`Guard::drop`] pauses it again the instant the annotated
//! workload returns.

use std::env;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};

use crate::error::Error;

pub(crate) const RESULT_DIR_ENV: &str = "METABENCH_INTERNAL_VTUNE_RESULT_DIR";

static ACTIVE: LazyLock<bool> = LazyLock::new(|| env::var_os(RESULT_DIR_ENV).is_some());
static ERROR: Mutex<Option<Error>> = Mutex::new(None);
static MEASURED: AtomicBool = AtomicBool::new(false);

#[derive(Debug)]
pub struct Guard {
    result_dir: Option<std::path::PathBuf>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(result_dir) = self.result_dir.take()
            && let Err(error) = run_command(&result_dir, "pause")
        {
            record_error(error);
        }
    }
}

#[inline]
#[doc(hidden)]
pub fn begin() -> Option<Guard> {
    if !*ACTIVE || MEASURED.swap(true, Ordering::Relaxed) {
        return None;
    }
    let result_dir = env::var_os(RESULT_DIR_ENV)?;
    let result_dir = std::path::PathBuf::from(result_dir);
    match run_command(&result_dir, "resume") {
        Ok(()) => Some(Guard {
            result_dir: Some(result_dir),
        }),
        Err(error) => {
            record_error(error);
            None
        }
    }
}

/// Resolves the environment probe behind [`begin`] ahead of any measurement.
///
/// See [`crate::perf::prime`] for why this must run before the measured
/// workload: leaving the `LazyLock` cold would charge the first workload for
/// the `getenv` call and its surrounding one-time synchronization.
#[cfg_attr(coverage_nightly, coverage(off))] // not exercised by the instrumented test binary; see prime()'s doc comment
#[cfg_attr(test, mutants::skip)] // warm-up only affects timing, not any observable behavior
pub(crate) fn prime() {
    let _active = *ACTIVE;
}

pub(crate) fn finish_worker() -> Result<(), Error> {
    let error = ERROR.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    if let Some(error) = error {
        return Err(error);
    }
    if !MEASURED.load(Ordering::Relaxed) {
        return Err(Error::VtuneWorkloadNotMeasured);
    }
    Ok(())
}

fn record_error(error: Error) {
    let mut stored = ERROR.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if stored.is_none() {
        *stored = Some(error);
    }
}

fn run_command(result_dir: &std::path::Path, command: &str) -> Result<(), Error> {
    let status = Command::new("vtune")
        .args(["-command", command, "-r"])
        .arg(result_dir)
        .status()
        .map_err(Error::VtuneControl)?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::VtuneControl(std::io::Error::other(format!(
            "vtune -command {command} exited with status {status}"
        ))))
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    // `MEASURED` and `ERROR` start at their `false`/`None` defaults for the
    // whole test binary, and nothing else in this crate's unit tests calls
    // `begin`/`finish_worker`/`record_error`, so this is the only test that
    // observes those statics; it does not need to reset or serialize them
    // against any sibling test. The control-protocol failure path
    // (`Error::VtuneControl`) is covered by
    // `vtune_command_failure_surfaces_vtune_control_error` in
    // `tests/spawned_benchmark.rs`, which runs it as its own process so it can
    // freely drive a fake `vtune` binary without touching these process-wide
    // statics.
    #[test]
    fn finish_worker_reports_workload_not_measured_when_never_measured() {
        assert!(matches!(finish_worker(), Err(Error::VtuneWorkloadNotMeasured)));
    }
}
