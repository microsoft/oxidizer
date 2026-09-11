// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::env;
use std::fs::OpenOptions;
use std::io::{self, Read as _, Write as _};
#[cfg(target_os = "linux")]
use std::os::fd::AsFd as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};

#[cfg(target_os = "linux")]
use nix::poll::{PollFd, PollFlags, poll};

use crate::error::Error;

pub(crate) const ACK_ENV: &str = "METABENCH_INTERNAL_PERF_ACK";
pub(crate) const CONTROL_ENV: &str = "METABENCH_INTERNAL_PERF_CONTROL";

// `perf` control is only ever driven by our own Linux-only worker process
// (see `runner::run_perf`), which is the only thing that sets `CONTROL_ENV`.
// Still, force this to `false` off Linux regardless of the environment:
// `Control::command` only guards its FIFO read with a poll timeout under
// `#[cfg(target_os = "linux")]`, so contaminated environment variables on
// another platform (for example inherited across a `cargo bench` re-exec)
// would otherwise attempt an unbounded blocking read on a FIFO that is
// never written to.
static ACTIVE: LazyLock<bool> = LazyLock::new(|| cfg!(target_os = "linux") && env::var_os(CONTROL_ENV).is_some());
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
    if !*ACTIVE || MEASURED.swap(true, Ordering::Relaxed) {
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

/// Resolves the environment probe behind [`begin`] ahead of any measurement.
///
/// [`begin`] runs inside the region a profiler collects, so leaving its
/// `LazyLock` cold would charge the first workload for a `getenv` call and the
/// surrounding one-time synchronization. Callgrind attributes that to the code
/// under test, inflating every reported instruction count by a fixed amount.
#[cfg_attr(coverage_nightly, coverage(off))] // not exercised by the instrumented test binary; see prime()'s doc comment
#[cfg_attr(test, mutants::skip)] // warm-up only affects timing, not any observable behavior
pub(crate) fn prime() {
    let _active = *ACTIVE;
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
