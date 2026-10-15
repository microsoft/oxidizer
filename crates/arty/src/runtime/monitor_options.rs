// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use observed::{Sink, emit};

use crate::runtime::Error;
use crate::runtime::telemetry::events::{MonitorFailure, MonitorMessage};

/// Configures this runtime's locally accessible diagnostic endpoint.
///
/// Pass these options to [`RuntimeBuilder::monitor`](super::RuntimeBuilder::monitor)
/// to opt a runtime into monitoring. The endpoint starts only when the runtime
/// is built. Omitting that setter leaves runtime/task diagnostics inactive.
///
/// Each opted-in runtime owns a separate authenticated localhost endpoint.
/// Recording settings, event buffers and allocator observations remain
/// process-wide; separate endpoints do not create isolated recorders.
///
/// The endpoint protocol is a diagnostic-tooling implementation detail, not
/// part of Arty's public Rust API compatibility contract.
#[derive(Clone, Debug, Default)]
pub struct MonitorOptions {
    name: Option<String>,
    instance: Option<String>,
    #[cfg(all(test, miri))]
    instrumentation_only: bool,
}

impl MonitorOptions {
    pub(in crate::runtime) fn runtime_name(&self) -> &str {
        self.name.as_deref().unwrap_or("arty")
    }

    /// Sets the human-readable endpoint name shown by local diagnostic tools.
    ///
    /// The same label identifies the runtime in diagnostic data. The default
    /// runtime label is `arty`; the endpoint uses the process executable's name.
    /// Empty names are rejected
    /// when the runtime is built.
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Sets an optional label distinguishing this runtime's endpoint.
    ///
    /// Empty labels are rejected when the runtime is built.
    #[must_use]
    pub fn instance(mut self, instance: impl Into<String>) -> Self {
        self.instance = Some(instance.into());
        self
    }

    #[cfg(all(test, miri))]
    pub(crate) fn instrumentation_only_for_tests() -> Self {
        Self {
            instrumentation_only: true,
            ..Self::default()
        }
    }

    pub(in crate::runtime) fn start(self, sink: Sink) -> Result<Option<seismograph::monitor::Monitor>, Error> {
        #[cfg(all(test, miri))]
        if self.instrumentation_only {
            return Ok(None);
        }
        let mut builder = seismograph::monitor::Monitor::builder();
        if let Some(name) = self.name {
            builder = builder.name(name);
        }
        if let Some(instance) = self.instance {
            builder = builder.instance(instance);
        }
        builder
            .error_handler(move |message| {
                emit!(
                    &sink,
                    MonitorFailure {
                        message: MonitorMessage(message.to_owned())
                    }
                );
            })
            .start()
            .map(Some)
            .map_err(Error::new)
    }
}
