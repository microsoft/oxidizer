// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Control protocol for measuring an explicit workload boundary under a live
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
//! module's [`Guard::drop`] pauses it again the instant the selected
//! workload returns.
//!
//! # Sampling limitations
//!
//! `VTune`'s hardware-event collection is a statistical sampler, not an exact
//! event counter: only the single native probe invocation that first calls
//! [`begin`] is measured (the `MEASURED` flag admits exactly one resume per
//! process), and unlike the exact per-invocation counts `perf` produces,
//! `VTune`'s reported counts are sampling estimates whose accuracy improves
//! with a longer-running workload. A workload short enough to complete
//! between sampler ticks can therefore report zero events for a metric it
//! genuinely exercises. Additionally, [`Guard::drop`]'s synchronous
//! `vtune -command pause` call is issued after the workload returns, so the
//! measured window extends slightly beyond the workload itself to include
//! that pause round-trip. Callers who need exact, per-invocation counts
//! should prefer the Linux `perf` engine (see [`crate::perf`]).

use std::env;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::Error;

pub(crate) const RESULT_DIR_ENV: &str = "METABENCH_INTERNAL_VTUNE_RESULT_DIR";

static ACTIVE: AtomicBool = AtomicBool::new(false);
static ERROR: Mutex<Option<Error>> = Mutex::new(None);
static MEASURED: AtomicBool = AtomicBool::new(false);

pub struct Guard {
    result_dir: Option<std::path::PathBuf>,
    pause: PauseCommand,
}

#[expect(
    clippy::missing_fields_in_debug,
    reason = "the private command adapter must not change the existing public Guard Debug output"
)]
impl std::fmt::Debug for Guard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Guard").field("result_dir", &self.result_dir).finish()
    }
}

#[derive(Default)]
enum PauseCommand {
    #[default]
    Native,
    #[cfg(test)]
    Injected {
        calls: std::sync::Arc<Mutex<Vec<(std::path::PathBuf, String)>>>,
        error: Option<Error>,
    },
}

impl PauseCommand {
    fn run(self, path: &std::path::Path, command: &str) -> Result<(), Error> {
        match self {
            Self::Native => run_command(path, command),
            #[cfg(test)]
            Self::Injected { calls, error } => {
                calls.lock().unwrap().push((path.to_owned(), command.to_owned()));
                error.map_or(Ok(()), Err)
            }
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let pause = std::mem::take(&mut self.pause);
        self.pause_with(|path, command| pause.run(path, command));
    }
}

impl Guard {
    fn pause_with(&mut self, command: impl FnOnce(&std::path::Path, &str) -> Result<(), Error>) {
        if let Some(result_dir) = self.result_dir.take()
            && let Err(error) = command(&result_dir, "pause")
        {
            // #[gamma::skip(call_result.default, reason = "record_error returns unit and default_after preserves the stored error")]
            record_error(error);
        }
    }
}

#[inline]
#[doc(hidden)]
pub fn begin() -> Option<Guard> {
    begin_with(vtune_environment, run_command)
}

fn begin_with(
    environment: impl FnOnce(&str) -> Option<std::ffi::OsString>,
    command: impl FnOnce(&std::path::Path, &str) -> Result<(), Error>,
) -> Option<Guard> {
    if !ACTIVE.load(Ordering::Relaxed) {
        return None;
    }
    // Validate before claiming the one measurement slot.
    let Some(result_dir) = environment(RESULT_DIR_ENV) else {
        // #[gamma::skip(call_result.default, reason = "record_error returns unit and default_after preserves the stored error")]
        record_error(Error::VtuneControl(std::io::Error::other(
            "METABENCH_INTERNAL_VTUNE_RESULT_DIR was unset after vtune::begin's environment was primed",
        )));
        return None;
    };
    if MEASURED.swap(true, Ordering::Relaxed) {
        return None;
    }
    let result_dir = std::path::PathBuf::from(result_dir);
    match command(&result_dir, "resume") {
        Ok(()) => Some(Guard {
            result_dir: Some(result_dir),
            pause: PauseCommand::Native,
        }),
        Err(error) => {
            // #[gamma::skip(call_result.default, reason = "record_error returns unit and default_after preserves the stored error")]
            record_error(error);
            None
        }
    }
}

/// Enables collection after validating a `VTune` worker's token and mode.
///
/// Initialize the collector before a private probe invokes its workload,
/// as in [`crate::perf::prime`].
#[cfg_attr(coverage_nightly, coverage(off))] // not exercised by the instrumented test binary; see prime()'s doc comment
pub(crate) fn prime() {
    // #[gamma::skip(call_result.default, reason = "prime_with returns unit and default_after preserves the activation store")]
    prime_with(vtune_environment);
}

fn prime_with(environment: impl FnOnce(&str) -> Option<std::ffi::OsString>) {
    ACTIVE.store(environment(RESULT_DIR_ENV).is_some(), Ordering::Relaxed);
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
    run_command_with(result_dir, command, vtune_status)
}

fn vtune_environment(name: &str) -> Option<std::ffi::OsString> {
    #[cfg(test)]
    if let Some(sources) = TEST_VTUNE_SOURCES.get() {
        return (sources.environment)(name);
    }
    env::var_os(name)
}

fn vtune_status(command: &mut Command) -> std::io::Result<std::process::ExitStatus> {
    #[cfg(test)]
    if let Some(sources) = TEST_VTUNE_SOURCES.get() {
        return (sources.status)(command);
    }
    command.status()
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct TestVtuneSources {
    environment: fn(&str) -> Option<std::ffi::OsString>,
    status: fn(&mut Command) -> std::io::Result<std::process::ExitStatus>,
}

#[cfg(test)]
thread_local! {
    static TEST_VTUNE_SOURCES: std::cell::Cell<Option<TestVtuneSources>> = const { std::cell::Cell::new(None) };
}

fn run_command_with(
    result_dir: &std::path::Path,
    command: &str,
    status: impl FnOnce(&mut Command) -> std::io::Result<std::process::ExitStatus>,
) -> Result<(), Error> {
    let mut process = Command::new("vtune");
    process.args(["-command", command, "-r"]).arg(result_dir);
    let status = status(&mut process).map_err(Error::VtuneControl)?;
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
    #[test]
    fn november_actual_vtune_entrypoints_preserve_activation_resume_pause_and_native_command_errors_without_processes() {
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt as _;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt as _;

        thread_local! {
            static COMMANDS: std::cell::RefCell<Vec<Vec<std::ffi::OsString>>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        struct Restore(Option<TestVtuneSources>, Vec<Vec<std::ffi::OsString>>);
        impl Drop for Restore {
            fn drop(&mut self) {
                TEST_VTUNE_SOURCES.set(self.0);
                COMMANDS.set(std::mem::take(&mut self.1));
            }
        }
        let _lock = STATE_LOCK.lock().unwrap();
        let _reset = ResetVtuneState;
        ACTIVE.store(false, Ordering::Relaxed);
        MEASURED.store(false, Ordering::Relaxed);
        let mut sources = TestVtuneSources {
            environment: |name| {
                assert_eq!(name, RESULT_DIR_ENV);
                Some("declared result directory".into())
            },
            status: |command| {
                assert_eq!(command.get_program(), "vtune");
                COMMANDS.with_borrow_mut(|commands| commands.push(command.get_args().map(std::ffi::OsStr::to_owned).collect()));
                Ok(std::process::ExitStatus::from_raw(0))
            },
        };
        let _restore = Restore(TEST_VTUNE_SOURCES.replace(Some(sources)), COMMANDS.take());
        prime();
        assert!(ACTIVE.load(Ordering::Relaxed));
        let guard = begin().expect("authorized first workload must retain its live guard");
        assert_eq!(guard.result_dir.as_deref(), Some(std::path::Path::new("declared result directory")));
        drop(guard);
        COMMANDS.with_borrow(|commands| {
            let expected: Vec<Vec<std::ffi::OsString>> = ["resume", "pause"]
                .into_iter()
                .map(|command| {
                    ["-command", command, "-r", "declared result directory"]
                        .map(std::ffi::OsString::from)
                        .to_vec()
                })
                .collect();
            assert_eq!(*commands, expected);
        });
        finish_worker().unwrap();
        MEASURED.store(false, Ordering::Relaxed);
        sources.status = |_| Err(std::io::Error::from_raw_os_error(5));
        TEST_VTUNE_SOURCES.set(Some(sources));
        assert!(begin().is_none());
        assert!(matches!(finish_worker(), Err(Error::VtuneControl(error)) if error.raw_os_error() == Some(5)));
        sources.environment = |_| None;
        TEST_VTUNE_SOURCES.set(Some(sources));
        prime();
        assert!(!ACTIVE.load(Ordering::Relaxed));
    }

    use super::*;

    #[test]
    fn november_owned_vtune_guard_drop_pauses_once_and_records_the_exact_error() {
        let _lock = STATE_LOCK.lock().unwrap();
        let _reset = ResetVtuneState;
        for failed in [false, true] {
            let calls = std::sync::Arc::new(Mutex::new(Vec::new()));
            let guard = Guard {
                result_dir: Some("owned result directory".into()),
                pause: PauseCommand::Injected {
                    calls: std::sync::Arc::clone(&calls),
                    error: failed.then(|| Error::Metadata("owned pause refused".into())),
                },
            };
            assert!(calls.lock().unwrap().is_empty());
            assert_eq!(format!("{guard:?}"), "Guard { result_dir: Some(\"owned result directory\") }");
            drop(guard);
            assert_eq!(
                *calls.lock().unwrap(),
                [(std::path::PathBuf::from("owned result directory"), "pause".into())]
            );
            let error = ERROR.lock().unwrap().take();
            match error {
                None => assert!(!failed),
                Some(Error::Metadata(message)) => {
                    assert!(failed);
                    assert_eq!(message, "owned pause refused");
                }
                Some(error) => panic!("unexpected pause error: {error}"),
            }
        }
    }

    #[test]
    fn gamma_round2_vtune_activation_admits_one_resume_and_one_pause() {
        let _lock = STATE_LOCK.lock().unwrap();
        let _reset = ResetVtuneState;
        MEASURED.store(false, Ordering::Relaxed);
        for present in [false, true] {
            prime_with(|name| {
                assert_eq!(name, RESULT_DIR_ENV);
                present.then(|| "result directory".into())
            });
            assert_eq!(ACTIVE.load(Ordering::Relaxed), present);
        }
        let mut guard = begin_with(
            |name| {
                assert_eq!(name, RESULT_DIR_ENV);
                Some("result directory".into())
            },
            |path, command| {
                assert_eq!(path, std::path::Path::new("result directory"));
                assert_eq!(command, "resume");
                Ok(())
            },
        )
        .unwrap();
        let result_dir = guard.result_dir.clone();
        let mut pauses = 0;
        guard.pause_with(|path, command| {
            pauses += 1;
            assert_eq!(path, std::path::Path::new("result directory"));
            assert_eq!(command, "pause");
            Ok(())
        });
        assert_eq!(result_dir.as_deref(), Some(std::path::Path::new("result directory")));
        assert_eq!(pauses, 1);
        assert!(guard.result_dir.is_none());
        assert!(MEASURED.load(Ordering::Relaxed));
        assert!(begin_with(|_| Some("result directory".into()), |_, _| panic!("measurement is latched")).is_none());
        guard.pause_with(|_, _| panic!("pause is latched"));
        finish_worker().unwrap();
    }

    #[test]
    fn gamma_round2_vtune_missing_activation_and_control_failures_preserve_error_state() {
        let _lock = STATE_LOCK.lock().unwrap();
        let _reset = ResetVtuneState;
        ACTIVE.store(false, Ordering::Relaxed);
        MEASURED.store(false, Ordering::Relaxed);
        assert!(begin_with(|_| panic!("inactive collector"), |_, _| panic!("inactive collector")).is_none());
        assert!(!MEASURED.load(Ordering::Relaxed));
        ACTIVE.store(true, Ordering::Relaxed);
        assert!(begin_with(|_| None, |_, _| panic!("missing result directory")).is_none());
        assert!(!MEASURED.load(Ordering::Relaxed));
        assert!(matches!(finish_worker(), Err(Error::VtuneControl(error))
            if error.to_string() == "METABENCH_INTERNAL_VTUNE_RESULT_DIR was unset after vtune::begin's environment was primed"));
        assert!(
            begin_with(
                |_| Some("result directory".into()),
                |_, _| Err(Error::Metadata("resume refused".into()))
            )
            .is_none()
        );
        assert!(MEASURED.load(Ordering::Relaxed));
        assert!(matches!(finish_worker(), Err(Error::Metadata(message)) if message == "resume refused"));
        let mut guard = Guard {
            result_dir: Some("result directory".into()),
            pause: PauseCommand::Native,
        };
        guard.pause_with(|path, command| {
            assert_eq!(path, std::path::Path::new("result directory"));
            assert_eq!(command, "pause");
            Err(Error::Metadata("pause refused".into()))
        });
        assert!(guard.result_dir.is_none());
        assert!(matches!(finish_worker(), Err(Error::Metadata(message)) if message == "pause refused"));
    }

    #[test]
    fn gamma_round2_vtune_control_commands_keep_argv_and_propagate_status_errors() {
        use std::ffi::OsStr;
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt as _;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt as _;
        use std::path::Path;
        use std::process::ExitStatus;

        for action in ["resume", "pause"] {
            run_command_with(Path::new("result directory"), action, |command| {
                assert_eq!(command.get_program(), "vtune");
                assert_eq!(
                    command.get_args().collect::<Vec<_>>(),
                    ["-command", action, "-r", "result directory"].map(OsStr::new)
                );
                Ok(ExitStatus::from_raw(0))
            })
            .unwrap();
            #[cfg(windows)]
            let failed = ExitStatus::from_raw(3);
            #[cfg(unix)]
            let failed = ExitStatus::from_raw(3 << 8);
            assert!(matches!(run_command_with(Path::new("result directory"), action, |_| Ok(failed)),
                Err(Error::VtuneControl(error)) if error.to_string() == format!("vtune -command {action} exited with status {failed}")));
        }
        assert!(
            matches!(run_command_with(Path::new("result directory"), "resume", |_| Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "collector refused"))),
            Err(Error::VtuneControl(error)) if error.kind() == std::io::ErrorKind::PermissionDenied && error.to_string() == "collector refused")
        );
    }

    static STATE_LOCK: Mutex<()> = Mutex::new(());

    struct ResetVtuneState;

    impl Drop for ResetVtuneState {
        fn drop(&mut self) {
            *ERROR.lock().unwrap() = None;
            MEASURED.store(false, Ordering::Relaxed);
        }
    }

    #[test]
    fn gamma_survivor_first_control_error_and_measured_finish() {
        let _lock = STATE_LOCK.lock().unwrap();
        let _reset = ResetVtuneState;
        assert!(ERROR.lock().unwrap().is_none());
        record_error(Error::VtuneControl(std::io::Error::other("first")));
        record_error(Error::VtuneControl(std::io::Error::other("second")));
        assert!(matches!(finish_worker(), Err(Error::VtuneControl(error)) if error.to_string() == "first"));
        MEASURED.store(true, Ordering::Relaxed);
        finish_worker().unwrap();
    }

    #[test]
    fn finish_worker_reports_workload_not_measured_when_never_measured() {
        let _lock = STATE_LOCK.lock().unwrap();
        assert!(matches!(finish_worker(), Err(Error::VtuneWorkloadNotMeasured)));
    }
}
