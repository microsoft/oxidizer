// Copyright (c) Microsoft Corporation.

use std::any::type_name;
use std::fmt;

use opentelemetry::Context;
use opentelemetry_sdk::logs::SdkLogRecord;

use crate::sampling::SamplingBatch;

/// A log record and captured OpenTelemetry context for a sampler to inspect,
/// save, or discard.
///
/// The processor enriches and redacts the record and keeps the original event time.
/// It captures the full OpenTelemetry context before sampling.
/// It uses that context and its telemetry suppression state when it sends the record.
///
/// Context values are shared and are not redacted. Saving this value can keep
/// application objects and spans alive, delaying spans that end when dropped.
pub struct LogSamplingContext {
    pub(crate) record: SdkLogRecord,
    pub(crate) context: Context,
}

impl LogSamplingContext {
    pub(crate) fn new(record: SdkLogRecord, context: Context) -> Self {
        Self { record, context }
    }

    /// Borrows the enriched and redacted log record.
    #[must_use]
    pub fn record(&self) -> &SdkLogRecord {
        &self.record
    }

    /// Borrows the OpenTelemetry context captured for this log.
    #[must_use]
    pub fn otel_context(&self) -> &Context {
        &self.context
    }
}

impl fmt::Debug for LogSamplingContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("record", &self.record)
            .finish_non_exhaustive()
    }
}

/// Chooses which log records to send to one destination.
///
/// The sampler receives enriched and redacted records before the destination's
/// first `OTel` processor. Its returned records are sent to that destination.
/// Metrics are unaffected.
///
/// Any calls to [`Self::sample`] and [`Self::flush`] can run at the same time.
/// This includes two `flush` calls, for example when two threads call
/// [`Sink::flush`](observed::Sink::flush).
/// Synchronize access to saved records inside the sampler.
/// After either method returns, the processor sends each returned record once
/// through this destination's logger. It does not sample those records again.
///
/// A sampler can save records and return them from a later call.
/// The processor sends only the records that the sampler returns.
///
/// # Nested events
///
/// Do not emit `observed` events from [`Self::sample`] or [`Self::flush`].
/// An OpenTelemetry [`LogProcessor`](opentelemetry_sdk::logs::LogProcessor) must not
/// emit `observed` events while it processes a returned record.
/// A [`Sink`](observed::Sink) does not send nested events to processors while it calls
/// [`EventProcessor::process`](observed::processing::EventProcessor::process).
/// This protection does not apply when you call `EventProcessor::process` directly
/// or during a flush.
pub trait LogSampler: Send + Sync + 'static {
    /// Receives a log record and returns the records to send.
    ///
    /// Return an empty batch to send nothing. The input can be discarded or
    /// saved for a later call.
    ///
    /// Records are already redacted for this destination. Do not return records
    /// received for other destinations.
    fn sample(&self, log: LogSamplingContext) -> SamplingBatch<LogSamplingContext>;

    /// Returns saved records for the processor to send.
    ///
    /// When you flush the destination's [`LogEventProcessor`](crate::LogEventProcessor),
    /// for example with [`Sink::flush`](observed::Sink::flush), it calls this method.
    /// Return or discard every record saved before this call.
    /// Records saved later can wait for the next flush.
    ///
    /// The processor sends the returned records after this method returns. It then
    /// flushes the destination's provider and returns any provider error to the caller.
    ///
    /// If a `sample` or `flush` call on another thread returns records that were saved
    /// before this call, this flush does not wait for them. The processor sends them
    /// after that other call returns.
    ///
    /// The processor does not call this method when it is dropped. To send saved
    /// records, call `Sink::flush` before you drop the last sink handle.
    fn flush(&self) -> SamplingBatch<LogSamplingContext>;
}

#[cfg(test)]
mod tests {
    use std::any::type_name;
    use std::time::SystemTime;

    use opentelemetry::Context;
    use opentelemetry::logs::{LogRecord, Logger, LoggerProvider, Severity};
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::logs::SdkLoggerProvider;

    use crate::sampling::LogSamplingContext;

    #[test]
    fn debug_includes_record_but_omits_captured_context() {
        let provider = SdkLoggerProvider::builder()
            .with_resource(Resource::builder_empty().build())
            .build();
        let logger = provider.logger("sampling.debug");
        let mut record = logger.create_log_record();
        record.set_event_name("sampling.debug");
        record.set_severity_number(Severity::Info);
        record.set_timestamp(SystemTime::UNIX_EPOCH);
        record.set_body("sanitized record body".into());
        let otel_context = Context::new().with_value("captured context");
        let otel_context_debug = format!("{otel_context:?}");
        let context = LogSamplingContext::new(record, otel_context);

        let debug = format!("{context:?}");

        assert!(debug.starts_with(type_name::<LogSamplingContext>()));
        assert!(debug.contains("record:"));
        assert!(debug.contains("sampling.debug"));
        assert!(debug.contains("sanitized record body"));
        assert!(!debug.contains(&otel_context_debug));
    }
}
