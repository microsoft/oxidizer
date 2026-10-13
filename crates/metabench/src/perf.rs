// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::env;
use std::fs::OpenOptions;
use std::io::{self, Read as _, Write as _};
#[cfg(target_os = "linux")]
use std::os::fd::AsFd as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

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

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(mut control) = self.control.take()
            && let Err(error) = control.command(b"disable\n")
        {
            record_error(error);
        }
    }
}

#[inline]
#[doc(hidden)]
pub fn begin() -> Option<Guard> {
    if !ACTIVE.load(Ordering::Relaxed) || MEASURED.swap(true, Ordering::Relaxed) {
        return None;
    }
    match Control::connect().and_then(|mut control| {
        control.command(b"enable\n")?;
        Ok(control)
    }) {
        Ok(control) => Some(Guard { control: Some(control) }),
        Err(error) => {
            record_error(error);
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
    ACTIVE.store(cfg!(target_os = "linux") && env::var_os(CONTROL_ENV).is_some(), Ordering::Relaxed);
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
struct Control {
    control: std::fs::File,
    acknowledgement: std::fs::File,
}

impl Control {
    fn connect() -> Result<Self, io::Error> {
        let control = required_path(CONTROL_ENV)?;
        let acknowledgement = required_path(ACK_ENV)?;
        Ok(Self {
            control: OpenOptions::new().write(true).open(control)?,
            acknowledgement: OpenOptions::new().read(true).open(acknowledgement)?,
        })
    }

    fn command(&mut self, command: &[u8]) -> Result<(), io::Error> {
        self.control.write_all(command)?;
        self.control.flush()?;
        #[cfg(target_os = "linux")]
        {
            let mut descriptors = [PollFd::new(self.acknowledgement.as_fd(), PollFlags::POLLIN)];
            if poll(&mut descriptors, 30_000_u16).map_err(io::Error::other)? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timed out waiting for perf acknowledgement",
                ));
            }
        }
        let mut acknowledgement = [0_u8; 4];
        self.acknowledgement.read_exact(&mut acknowledgement)?;
        if acknowledgement != *b"ack\n" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected perf acknowledgement {:?}", String::from_utf8_lossy(&acknowledgement)),
            ));
        }
        Ok(())
    }
}

fn required_path(name: &str) -> Result<PathBuf, io::Error> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("missing {name}")))
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

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
