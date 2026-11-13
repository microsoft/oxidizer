// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs::OpenOptions;
#[cfg(target_os = "linux")]
use std::os::fd::AsFd as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{env, io};

#[cfg(target_os = "linux")]
use nix::poll::{PollFd, PollFlags, poll};

use crate::error::Error;

pub(crate) const ACK_ENV: &str = "METABENCH_INTERNAL_PERF_ACK";
pub(crate) const CONTROL_ENV: &str = "METABENCH_INTERNAL_PERF_CONTROL";

static ACTIVE: AtomicBool = AtomicBool::new(false);
static ERROR: Mutex<Option<io::Error>> = Mutex::new(None);
static MEASURED: AtomicBool = AtomicBool::new(false);

#[derive(Debug)]
pub struct Guard {
    control: Option<Control>,
}

#[derive(Debug)]
struct GuardState<C> {
    control: Option<C>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        // #[gamma::skip(call_result.default, reason = "pause_control returns unit and default_after preserves disabling and error recording")]
        pause_control(&mut self.control, Control::command, record_error);
    }
}

fn pause_control<C>(control: &mut Option<C>, command: impl FnOnce(&mut C, &[u8]) -> io::Result<()>, record: impl FnOnce(io::Error)) {
    if let Some(mut control) = control.take()
        && let Err(error) = command(&mut control, b"disable\n")
    {
        record(error);
    }
}

#[inline]
#[doc(hidden)]
pub fn begin() -> Option<Guard> {
    begin_with(&ACTIVE, &MEASURED, Control::connect, Control::command, record_error).map(|state| Guard { control: state.control })
}

fn begin_with<C>(
    active: &AtomicBool,
    measured: &AtomicBool,
    connect: impl FnOnce() -> io::Result<C>,
    command: impl FnOnce(&mut C, &[u8]) -> io::Result<()>,
    record: impl FnOnce(io::Error),
) -> Option<GuardState<C>> {
    if !active.load(Ordering::Relaxed) || measured.swap(true, Ordering::Relaxed) {
        return None;
    }
    match connect().and_then(|mut control| {
        command(&mut control, b"enable\n")?;
        Ok(control)
    }) {
        Ok(control) => Some(GuardState { control: Some(control) }),
        Err(error) => {
            record(error);
            None
        }
    }
}

/// Enables collection after validating a perf worker's token and mode.
///
/// Initialize the collector before a private probe prepares and invokes its
/// workload. Ordinary Criterion callbacks do not call [`begin`].
#[cfg_attr(coverage_nightly, coverage(off))] // not exercised by the instrumented test binary; see prime()'s doc comment
pub(crate) fn prime() {
    // #[gamma::skip(call_result.default, reason = "prime_with returns unit and default_after preserves the activation store")]
    prime_with(cfg!(target_os = "linux"), |name| env::var_os(name), &ACTIVE);
}

fn prime_with(supported: bool, environment: impl FnOnce(&str) -> Option<std::ffi::OsString>, active: &AtomicBool) {
    active.store(supported && environment(CONTROL_ENV).is_some(), Ordering::Relaxed);
}

pub(crate) fn finish_worker() -> Result<(), Error> {
    let error = ERROR.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    if let Some(error) = error {
        return Err(Error::PerfControl(error));
    }
    if !MEASURED.load(Ordering::Relaxed) {
        return Err(Error::PerfWorkloadNotMeasured);
    }
    Ok(())
}

fn record_error(error: io::Error) {
    let mut stored = ERROR.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if stored.is_none() {
        *stored = Some(error);
    }
}

#[derive(Debug)]
struct NativeControl<F = std::fs::File> {
    control: F,
    acknowledgement: F,
}

enum Control<F = std::fs::File> {
    Native(NativeControl<F>),
    #[cfg(test)]
    Injected {
        commands: std::sync::Arc<Mutex<Vec<Vec<u8>>>>,
        error: Option<io::Error>,
    },
}

impl<F: std::fmt::Debug> std::fmt::Debug for Control<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Native(native) => f
                .debug_struct("Control")
                .field("control", &native.control)
                .field("acknowledgement", &native.acknowledgement)
                .finish(),
            #[cfg(test)]
            Self::Injected { commands, error } => f
                .debug_struct("InjectedControl")
                .field("commands", commands)
                .field("error", error)
                .finish(),
        }
    }
}

impl Control {
    fn connect() -> Result<Self, io::Error> {
        #[cfg(test)]
        if let Some(control) = TEST_PERF_CONTROL.with_borrow_mut(Option::take) {
            return Ok(control);
        }
        let (control, acknowledgement) = Self::connect_with(required_path, |path, read, write| {
            OpenOptions::new().read(read).write(write).open(path)
        })?;
        Ok(Self::Native(NativeControl { control, acknowledgement }))
    }

    fn connect_with<C>(
        mut path: impl FnMut(&str) -> io::Result<PathBuf>,
        mut open: impl FnMut(PathBuf, bool, bool) -> io::Result<C>,
    ) -> io::Result<(C, C)> {
        let control = path(CONTROL_ENV)?;
        let acknowledgement = path(ACK_ENV)?;
        Ok((open(control, false, true)?, open(acknowledgement, true, false)?))
    }

    fn command(&mut self, command: &[u8]) -> Result<(), io::Error> {
        let native = match self {
            Self::Native(native) => native,
            #[cfg(test)]
            Self::Injected { commands, error } => {
                commands.lock().unwrap().push(command.to_vec());
                return error.take().map_or(Ok(()), Err);
            }
        };
        control_command(&mut native.control, &mut native.acknowledgement, command, |acknowledgement| {
            #[cfg(target_os = "linux")]
            {
                let mut descriptors = [PollFd::new(acknowledgement.as_fd(), PollFlags::POLLIN)];
                if poll(&mut descriptors, 30_000_u16).map_err(io::Error::other)? == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "timed out waiting for perf acknowledgement",
                    ));
                }
            }
            #[cfg(not(target_os = "linux"))]
            let _ = acknowledgement;
            Ok(())
        })
    }
}

#[cfg(test)]
thread_local! {
    static TEST_PERF_CONTROL: std::cell::RefCell<Option<Control>> = const { std::cell::RefCell::new(None) };
}

fn control_command<R: io::Read>(
    control: &mut impl io::Write,
    acknowledgement: &mut R,
    command: &[u8],
    wait: impl FnOnce(&R) -> io::Result<()>,
) -> io::Result<()> {
    control.write_all(command)?;
    control.flush()?;
    wait(acknowledgement)?;
    // #[gamma::skip(literal.int_increment, reason = "read_exact overwrites all four bytes or returns an error before the reply is inspected")]
    let mut reply = [0_u8; 4];
    acknowledgement.read_exact(&mut reply)?;
    if reply != *b"ack\n" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected perf acknowledgement {:?}", String::from_utf8_lossy(&reply)),
        ));
    }
    Ok(())
}

fn required_path(name: &str) -> Result<PathBuf, io::Error> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("missing {name}")))
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    #[test]
    fn november6_native_control_debug_names_both_distinct_channels() {
        let control = Control::Native(NativeControl {
            control: "command-channel",
            acknowledgement: "reply-channel",
        });
        assert_eq!(
            format!("{control:?}"),
            "Control { control: \"command-channel\", acknowledgement: \"reply-channel\" }"
        );
    }

    #[test]
    fn november_actual_perf_begin_retains_the_enabled_owner_and_disables_it_once_without_native_channels() {
        struct Restore(Option<Control>);
        impl Drop for Restore {
            fn drop(&mut self) {
                TEST_PERF_CONTROL.set(self.0.take());
            }
        }
        let _lock = STATE_LOCK.lock().unwrap();
        let _reset = ResetPerfState;
        let commands = std::sync::Arc::new(Mutex::new(Vec::new()));
        let _restore = Restore(TEST_PERF_CONTROL.replace(Some(Control::Injected {
            commands: std::sync::Arc::clone(&commands),
            error: None,
        })));
        ACTIVE.store(true, Ordering::Relaxed);
        MEASURED.store(false, Ordering::Relaxed);
        let guard = begin().expect("authorized first workload must retain its enabled control owner");
        assert_eq!(*commands.lock().unwrap(), [b"enable\n".to_vec()]);
        assert!(begin().is_none());
        drop(guard);
        assert_eq!(*commands.lock().unwrap(), [b"enable\n".to_vec(), b"disable\n".to_vec()]);
        finish_worker().unwrap();
    }

    use std::process::Command;

    use super::*;

    static STATE_LOCK: Mutex<()> = Mutex::new(());

    struct ResetPerfState;

    impl Drop for ResetPerfState {
        fn drop(&mut self) {
            *ERROR.lock().unwrap() = None;
            MEASURED.store(false, Ordering::Relaxed);
            ACTIVE.store(false, Ordering::Relaxed);
        }
    }

    #[test]
    fn november_owned_perf_guard_drop_disables_once_and_retains_the_original_failure() {
        let _lock = STATE_LOCK.lock().unwrap();
        let _reset = ResetPerfState;
        for failure in [None, Some(io::Error::new(io::ErrorKind::BrokenPipe, "owned disable refused"))] {
            let failed = failure.is_some();
            let commands = std::sync::Arc::new(Mutex::new(Vec::new()));
            let guard = Guard {
                control: Some(Control::Injected {
                    commands: std::sync::Arc::clone(&commands),
                    error: failure,
                }),
            };
            assert!(commands.lock().unwrap().is_empty());
            drop(guard);
            assert_eq!(*commands.lock().unwrap(), [b"disable\n".to_vec()]);
            let stored = ERROR.lock().unwrap().take();
            if failed {
                let error = stored.unwrap();
                assert_eq!(
                    (error.kind(), error.to_string()),
                    (io::ErrorKind::BrokenPipe, "owned disable refused".into())
                );
            } else {
                assert!(stored.is_none());
            }
        }
        drop(Guard { control: None });
        assert!(ERROR.lock().unwrap().is_none());
    }

    #[test]
    fn gamma_round2_perf_guard_disarms_and_records_disable_failures_once() {
        let mut control = Some(17);
        let called = std::cell::Cell::new(0);
        let error = std::cell::RefCell::new(None);
        pause_control(
            &mut control,
            |control, bytes| {
                assert_eq!(*control, 17);
                assert_eq!(bytes, b"disable\n");
                called.set(called.get() + 1);
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "disable refused"))
            },
            |source| {
                *error.borrow_mut() = Some((source.kind(), source.to_string()));
            },
        );
        assert!(control.is_none());
        assert_eq!(called.get(), 1);
        assert_eq!(*error.borrow(), Some((io::ErrorKind::BrokenPipe, "disable refused".into())));
        pause_control(&mut control, |_, _| panic!("already disarmed"), |_| panic!("already disarmed"));
        let mut control = Some(19);
        pause_control(
            &mut control,
            |control, bytes| {
                assert_eq!(*control, 19);
                assert_eq!(bytes, b"disable\n");
                Ok(())
            },
            |_| panic!("successful disable has no error"),
        );
        assert!(control.is_none());
        assert_eq!(format!("{:?}", Guard { control: None }), "Guard { control: None }");
    }

    #[test]
    fn gamma_round2_perf_begin_retains_enabled_control_and_records_connection_errors() {
        let active = AtomicBool::new(true);
        let measured = AtomicBool::new(false);
        let state = begin_with(
            &active,
            &measured,
            || Ok(41),
            |control, bytes| {
                assert_eq!(*control, 41);
                assert_eq!(bytes, b"enable\n");
                *control = 43;
                Ok(())
            },
            |_| panic!("enable succeeded"),
        )
        .unwrap();
        assert_eq!(state.control, Some(43));
        assert!(measured.load(Ordering::Relaxed));
        assert!(
            begin_with::<u8>(
                &active,
                &measured,
                || panic!("already measured"),
                |_, _| panic!("already measured"),
                |_| panic!("already measured")
            )
            .is_none()
        );
        for connection_error in [false, true] {
            measured.store(false, Ordering::Relaxed);
            let recorded = std::cell::RefCell::new(None);
            assert!(
                begin_with(
                    &active,
                    &measured,
                    || if connection_error {
                        Err(io::Error::new(io::ErrorKind::NotFound, "connect refused"))
                    } else {
                        Ok(7)
                    },
                    |_, _| {
                        assert!(!connection_error);
                        Err(io::Error::new(io::ErrorKind::BrokenPipe, "enable refused"))
                    },
                    |error| {
                        *recorded.borrow_mut() = Some((error.kind(), error.to_string()));
                    },
                )
                .is_none()
            );
            assert_eq!(
                *recorded.borrow(),
                Some(if connection_error {
                    (io::ErrorKind::NotFound, "connect refused".into())
                } else {
                    (io::ErrorKind::BrokenPipe, "enable refused".into())
                })
            );
        }
        active.store(false, Ordering::Relaxed);
        measured.store(false, Ordering::Relaxed);
        assert!(
            begin_with::<u8>(
                &active,
                &measured,
                || panic!("inactive"),
                |_, _| panic!("inactive"),
                |_| panic!("inactive")
            )
            .is_none()
        );
        assert!(!measured.load(Ordering::Relaxed));
    }

    #[test]
    fn gamma_round2_perf_connect_preserves_environment_open_flags_and_error_order() {
        for failing in [None, Some(0), Some(1), Some(2), Some(3)] {
            let events = std::cell::RefCell::new(Vec::new());
            let result = Control::connect_with(
                |name| {
                    let index = events.borrow().len();
                    events.borrow_mut().push(name.to_owned());
                    if failing == Some(index) {
                        Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("phase {index}")))
                    } else {
                        Ok(PathBuf::from(if index == 0 { "control fifo" } else { "ack fifo" }))
                    }
                },
                |path, read, write| {
                    let index = events.borrow().len();
                    events.borrow_mut().push(path.to_string_lossy().into_owned());
                    assert_eq!((read, write), if index == 2 { (false, true) } else { (true, false) });
                    if failing == Some(index) {
                        Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("phase {index}")))
                    } else {
                        Ok(index)
                    }
                },
            );
            let expected = [
                "METABENCH_INTERNAL_PERF_CONTROL",
                "METABENCH_INTERNAL_PERF_ACK",
                "control fifo",
                "ack fifo",
            ];
            if let Some(index) = failing {
                let error = result.unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
                assert_eq!(error.to_string(), format!("phase {index}"));
                assert_eq!(*events.borrow(), expected[..=index]);
            } else {
                assert_eq!(result.unwrap(), (2, 3));
                assert_eq!(*events.borrow(), expected);
            }
        }
    }

    #[test]
    fn gamma_round2_perf_prime_requires_platform_support_and_control_environment() {
        for supported in [false, true] {
            for configured in [false, true] {
                let active = AtomicBool::new(true);
                let called = std::cell::Cell::new(false);
                prime_with(
                    supported,
                    |name| {
                        assert_eq!(name, "METABENCH_INTERNAL_PERF_CONTROL");
                        called.set(true);
                        configured.then(|| "control".into())
                    },
                    &active,
                );
                assert_eq!(active.load(Ordering::Relaxed), supported && configured);
                assert_eq!(called.get(), supported);
            }
        }
    }

    #[test]
    fn gamma_round2_perf_control_protocol_requires_write_flush_and_exact_acknowledgement() {
        use std::cell::RefCell;

        struct ControlWriter<'a>(&'a RefCell<Vec<&'static str>>, bool, bool);
        impl io::Write for ControlWriter<'_> {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0.borrow_mut().push("write");
                if self.1 {
                    Err(io::Error::new(io::ErrorKind::BrokenPipe, "write refused"))
                } else {
                    assert_eq!(buf, b"enable\n");
                    Ok(buf.len())
                }
            }
            fn flush(&mut self) -> io::Result<()> {
                self.0.borrow_mut().push("flush");
                if self.2 {
                    Err(io::Error::new(io::ErrorKind::PermissionDenied, "flush refused"))
                } else {
                    Ok(())
                }
            }
        }
        let events = RefCell::new(Vec::new());
        let mut reader = io::Cursor::new(b"ack\n");
        control_command(&mut ControlWriter(&events, false, false), &mut reader, b"enable\n", |_| {
            assert_eq!(*events.borrow(), ["write", "flush"]);
            events.borrow_mut().push("wait");
            Ok(())
        })
        .unwrap();
        assert_eq!(*events.borrow(), ["write", "flush", "wait"]);
        assert_eq!(reader.position(), 4);
        for (write_error, flush_error, kind, message) in [
            (true, false, io::ErrorKind::BrokenPipe, "write refused"),
            (false, true, io::ErrorKind::PermissionDenied, "flush refused"),
        ] {
            let error = control_command(
                &mut ControlWriter(&events, write_error, flush_error),
                &mut io::Cursor::new(b"ack\n"),
                b"enable\n",
                |_| panic!("failed output cannot wait"),
            )
            .unwrap_err();
            assert_eq!(error.kind(), kind);
            assert_eq!(error.to_string(), message);
        }
        let error = control_command(&mut Vec::new(), &mut io::Cursor::new(b"ack"), b"enable\n", |_| Ok(())).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        let error = control_command(&mut Vec::new(), &mut io::Cursor::new(b"nack"), b"enable\n", |_| Ok(())).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "unexpected perf acknowledgement \"nack\"");
        let mut reader = io::Cursor::new(b"ack\n");
        let error = control_command(&mut Vec::new(), &mut reader, b"enable\n", |_| {
            Err(io::Error::new(io::ErrorKind::TimedOut, "wait refused"))
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(reader.position(), 0);
    }

    #[test]
    fn gamma_survivor_first_control_error_and_measured_finish() {
        let _lock = STATE_LOCK.lock().unwrap();
        let _reset = ResetPerfState;
        assert!(ERROR.lock().unwrap().is_none());
        record_error(io::Error::new(io::ErrorKind::BrokenPipe, "first"));
        record_error(io::Error::new(io::ErrorKind::PermissionDenied, "second"));
        assert!(matches!(finish_worker(), Err(Error::PerfControl(error))
            if error.kind() == io::ErrorKind::BrokenPipe && error.to_string() == "first"));
        MEASURED.store(true, Ordering::Relaxed);
        finish_worker().unwrap();
    }

    #[test]
    fn collector_activation_and_failed_begin_are_isolated() {
        const FIXTURE: &str = "METABENCH_TEST_PERF_STATE";
        if let Some(scenario) = env::var_os(FIXTURE) {
            if scenario == "forced" {
                ACTIVE.store(true, Ordering::Relaxed);
                assert!(begin().is_none());
                assert!(MEASURED.load(Ordering::Relaxed));
                assert!(matches!(
                    finish_worker(),
                    Err(Error::PerfControl(error)) if error.kind() == io::ErrorKind::NotFound
                ));
                assert!(begin().is_none());
                return;
            }
            ACTIVE.store(true, Ordering::Relaxed);
            prime();
            assert_eq!(
                ACTIVE.load(Ordering::Relaxed),
                scenario == "configured" && cfg!(target_os = "linux")
            );
            assert!(begin().is_none());
            assert_eq!(
                MEASURED.load(Ordering::Relaxed),
                scenario == "configured" && cfg!(target_os = "linux")
            );
            assert!(finish_worker().is_err());
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        for scenario in ["disabled", "configured", "forced"] {
            let mut command = Command::new(env::current_exe().unwrap());
            command
                .args(["--exact", "perf::tests::collector_activation_and_failed_begin_are_isolated"])
                .env(FIXTURE, scenario)
                .env_remove(CONTROL_ENV)
                .env_remove(ACK_ENV);
            if scenario == "configured" {
                command.env(CONTROL_ENV, directory.path().join("missing-control"));
            }
            assert!(command.status().unwrap().success());
        }
    }
}
