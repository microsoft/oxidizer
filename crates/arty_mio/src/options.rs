// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use observed::Sink;

/// Options for creating a [`MioContext`](crate::MioContext).
///
/// The default configuration uses a no-op telemetry sink.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct MioOptions {
    sink: Sink,
}

impl MioOptions {
    /// Sets the telemetry sink for driver lifecycle and failure events.
    ///
    /// The default is [`Sink::noop`]. Mio does not emit events for successful
    /// registration, readiness, polling, or I/O operations.
    #[must_use]
    pub fn sink(mut self, sink: Sink) -> Self {
        self.sink = sink;
        self
    }

    pub(crate) fn into_sink(self) -> Sink {
        self.sink
    }
}

impl Default for MioOptions {
    fn default() -> Self {
        Self { sink: Sink::noop() }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn default_options_disable_telemetry() {
        assert!(MioOptions::default().sink.is_noop());
    }
}
