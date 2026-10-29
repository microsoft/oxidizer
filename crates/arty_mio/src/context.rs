// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::{fmt, io};

use arty_io_core::{DriverError, DriverOptions, IoContext};
use mio::Interest;
use mio::event::Source;
use performables::arc::Arc;
use thread_aware_core::{Thread, ThreadAware};

use crate::driver::MioDriver;
use crate::options::MioOptions;
use crate::registration::{Registered, register};
use crate::telemetry;
use crate::worker::{ContextState, WorkerState};

/// A thread-aware route to a worker-local Mio driver.
///
/// New registrations use the current worker when it has a driver. Otherwise,
/// they use the last driver selected for this clone. Relocation selects an
/// available destination driver for new registrations. Existing registrations
/// stay with their original driver.
#[derive(Clone)]
pub struct MioContext {
    state: Arc<ContextState>,
    worker: Option<Arc<WorkerState>>,
}

impl fmt::Debug for MioContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MioContext")
            .field("attached", &self.worker.is_some())
            .finish_non_exhaustive()
    }
}

impl MioContext {
    /// Registers an owned Mio source on the calling thread.
    ///
    /// The selected worker holds a [`Registry`](mio::Registry) clone created
    /// with [`Registry::try_clone`](mio::Registry::try_clone). This method uses
    /// that clone to call [`Source::register`] on the calling thread.
    ///
    /// The returned [`Registered`] value owns the source and provides direct
    /// reference access. Driver shutdown closes readiness, but the registration
    /// keeps the source until its owner drops it.
    ///
    /// # Errors
    ///
    /// Returns an error if this context is not attached to a driver, the
    /// driver is closed, or Mio rejects the source.
    pub fn register<S>(&self, source: S, interest: Interest) -> io::Result<Registered<S>>
    where
        S: Source + Send + 'static,
    {
        register(source, interest, self.registration_worker()?)
    }

    fn registration_worker(&self) -> io::Result<Arc<WorkerState>> {
        let current = std::thread::current().id();
        self.worker
            .as_ref()
            .filter(|worker| worker.thread_id == current)
            .cloned()
            .or_else(|| self.state.worker(current))
            .or_else(|| self.worker.clone())
            .ok_or_else(|| {
                let error = io::Error::new(
                    io::ErrorKind::NotConnected,
                    "the Arty Mio context is not attached to a worker driver",
                );
                self.state.report_attachment_failure(&error);
                error
            })
    }
}

impl ThreadAware for MioContext {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        // A missing destination route leaves the previous route usable.
        if let Some(worker) = self.state.try_worker(destination.id()) {
            self.worker = Some(worker);
        }
    }
}

impl IoContext for MioContext {
    type ContextOptions = MioOptions;
    type Driver = MioDriver;

    fn context(options: Self::ContextOptions) -> Self {
        Self {
            state: Arc::new(ContextState::new(options.into_sink())),
            worker: None,
        }
    }

    fn driver(&mut self, options: DriverOptions) -> Result<Self::Driver, DriverError> {
        let role = options.role();
        let thread_id = options.thread().id();
        let (driver, worker) = MioDriver::new(Arc::clone(&self.state), &options).inspect_err(|error| {
            telemetry::report_driver_failed(self.state.sink(), role, thread_id, telemetry::DriverOperation::Creation, error);
        })?;
        self.worker = Some(worker);
        Ok(driver)
    }
}
