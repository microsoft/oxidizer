// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph::monitor::Monitor;
use seismograph_runtime::{RuntimeHandle, RuntimeMetadata, RuntimeRegistration};

use crate::runtime::{Error, MonitorPolicy};

#[derive(Debug)]
pub(crate) struct RuntimeDiagnostics {
    monitor: Monitor,
    registration: RuntimeRegistration,
}

impl RuntimeDiagnostics {
    pub(crate) fn start(policy: &MonitorPolicy, workers: usize) -> Result<Option<Self>, Error> {
        if !policy.is_enabled() {
            return Ok(None);
        }
        let workers = u32::try_from(workers).map_err(Error::new)?;
        let builder = Monitor::builder().name(policy.runtime_name());
        #[cfg(test)]
        let builder = builder.io(seismograph_protocol::io::MonitorIo::new_mocked().0);
        let monitor = builder.start().map_err(Error::new)?;
        let registration = seismograph_runtime::register_runtime(RuntimeMetadata::new(policy.runtime_name(), workers));
        Ok(Some(Self { monitor, registration }))
    }

    pub(crate) fn handle(&self) -> RuntimeHandle {
        self.registration.handle()
    }

    #[cfg_attr(test, mutants::skip)] // A monitor failure fixture would duplicate Seismograph's protocol tests.
    pub(crate) fn finish(self) -> Result<(), Error> {
        self.registration.stopped();
        let error = self.monitor.last_error();
        drop(self);
        error.map_or(Ok(()), |message| Err(Error::new(MonitorFailure::new(message))))
    }
}

#[ohno::error]
#[display("the runtime monitor failed: {message}")]
struct MonitorFailure {
    message: String,
}
