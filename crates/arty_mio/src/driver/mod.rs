// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc as StdArc;
use std::task::Waker;
use std::time::Duration;
use std::{fmt, thread};

use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, DriverRole};
use mio::Poll;
use performables::arc::Arc;

use self::poll_wake::PollWake;
use self::primary::PrimaryDriver;
use self::secondary::SecondaryDriver;
use crate::telemetry::{self, DriverOperation, DriverOutcome};
use crate::worker::{ContextState, WorkerState};

mod poll_wake;
mod primary;
mod secondary;

const COMPLETION_BUDGET: usize = 256;
const EVENT_CAPACITY: usize = 1_024;
const SHUTDOWN_LIMIT: Duration = Duration::from_secs(1);

/// A Mio poll driver for one Arty worker.
///
/// This driver owns the [`Poll`] lifecycle and all driver-side registration
/// state. A primary driver polls on the Arty worker. A secondary driver owns a
/// helper thread that waits for Mio events.
///
/// Shutdown closes readiness for each live registration. It does not take or
/// drop the registered sources.
pub struct MioDriver {
    inner: Option<DriverInner>,
}

impl fmt::Debug for MioDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MioDriver")
            .field("active", &self.inner.is_some())
            .finish_non_exhaustive()
    }
}

impl MioDriver {
    pub(crate) fn new(state: Arc<ContextState>, options: &DriverOptions) -> Result<(Self, Arc<WorkerState>), DriverError> {
        if options.thread().id() != thread::current().id() {
            return Err(DriverError::from_message(
                "the caller must create an Arty Mio driver on the worker named by DriverOptions",
            ));
        }

        let (mode, poll_waker, registry) = match options.role() {
            DriverRole::Primary => {
                let poll = Poll::new().map_err(DriverError::from_source)?;
                let poll_waker = StdArc::new(PollWake::new(poll.registry()).map_err(DriverError::from_source)?);
                let registry = poll.registry().try_clone().map_err(DriverError::from_source)?;
                (DriverMode::Primary(PrimaryDriver::new(poll)), poll_waker, registry)
            }
            DriverRole::Secondary => {
                let (driver, poll_waker, registry) = SecondaryDriver::new().map_err(DriverError::from_source)?;
                (DriverMode::Secondary(driver), poll_waker, registry)
            }
        };

        let role = options.role();
        let thread_id = options.thread().id();
        let worker = Arc::new(WorkerState::new(
            thread_id,
            registry,
            options.owner_waker().clone(),
            state.sink().clone(),
        ));
        if let DriverMode::Secondary(driver) = &mode {
            driver.attach(Arc::clone(&worker));
        }
        if let Err(error) = state.install(Arc::clone(&worker)) {
            worker.close_registrations();
            let cleanup = match mode {
                DriverMode::Primary(_) => Ok(()),
                DriverMode::Secondary(driver) => driver.shutdown(&poll_waker, &worker),
            };
            poll_waker.deactivate();
            cleanup?;
            return Err(DriverError::from_source(error));
        }

        telemetry::report_driver_started(state.sink(), role, thread_id);
        let driver = Self {
            inner: Some(DriverInner {
                context: state,
                worker: Arc::clone(&worker),
                poll_waker,
                mode,
                role,
                thread_id,
                failure_reported: false,
            }),
        };
        Ok((driver, worker))
    }

    fn inner(&self) -> &DriverInner {
        self.inner
            .as_ref()
            .expect("Driver::shutdown consumes the Mio driver before its inner state is removed")
    }

    fn inner_mut(&mut self) -> &mut DriverInner {
        self.inner
            .as_mut()
            .expect("Driver::shutdown consumes the Mio driver before its inner state is removed")
    }
}

impl Driver for MioDriver {
    fn waker(&self) -> Waker {
        Waker::from(StdArc::clone(&self.inner().poll_waker))
    }

    fn execute_cycle(&mut self, cycle: &mut Cycle) -> Result<(), DriverError> {
        self.inner_mut().execute_cycle(cycle)
    }

    fn shutdown(mut self) -> Result<(), DriverError> {
        self.inner
            .take()
            .expect("the Driver trait calls shutdown at most once because shutdown consumes self")
            .shutdown()
    }
}

impl Drop for MioDriver {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            let _ = inner.shutdown();
        }
    }
}

struct DriverInner {
    context: Arc<ContextState>,
    worker: Arc<WorkerState>,
    poll_waker: StdArc<PollWake>,
    mode: DriverMode,
    role: DriverRole,
    thread_id: thread::ThreadId,
    failure_reported: bool,
}

impl DriverInner {
    fn execute_cycle(&mut self, cycle: &Cycle) -> Result<(), DriverError> {
        let result = match &mut self.mode {
            DriverMode::Primary(driver) => driver.execute_cycle(&self.worker, cycle.max_wait()),
            DriverMode::Secondary(driver) => driver.execute_cycle(&self.worker),
        };
        if let Err(error) = &result
            && !self.failure_reported
        {
            telemetry::report_driver_failed(self.worker.sink(), self.role, self.thread_id, DriverOperation::Cycle, error);
            self.failure_reported = true;
        }
        result
    }

    fn shutdown(self) -> Result<(), DriverError> {
        let Self {
            context,
            worker,
            poll_waker,
            mode,
            role,
            thread_id,
            failure_reported,
        } = self;
        context.remove(&worker);
        worker.close_registrations();
        let result = match mode {
            DriverMode::Primary(driver) => {
                let control_error = worker.take_failure().map(DriverError::from_source);
                poll_waker.deactivate();
                drop(driver);
                match control_error {
                    Some(error) => Err(error),
                    None => Ok(()),
                }
            }
            DriverMode::Secondary(driver) => {
                let result = driver.shutdown(&poll_waker, &worker);
                poll_waker.deactivate();
                result
            }
        };
        if let Err(error) = &result
            && !failure_reported
        {
            telemetry::report_driver_failed(worker.sink(), role, thread_id, DriverOperation::Shutdown, error);
        }
        let outcome = if result.is_ok() {
            DriverOutcome::Succeeded
        } else {
            DriverOutcome::Failed
        };
        telemetry::report_driver_stopped(worker.sink(), role, thread_id, outcome);
        result
    }
}

enum DriverMode {
    Primary(PrimaryDriver),
    Secondary(SecondaryDriver),
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
