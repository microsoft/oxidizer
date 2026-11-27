// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]

//! # Observed stdout Destination
//!
//! Provides implementations of [`LogDestination`] and [`MetricDestination`] that write to stdout.
//!
//! # Examples
//!
//! ```
//! use log::LevelFilter;
//! use observed_destination_core::{LogDestination, LoggerProviderBuilderExt};
//! use observed_destination_stdout::stdout_logs;
//! use opentelemetry::logs::LoggerProvider;
//! use opentelemetry_sdk::logs::SdkLoggerProvider;
//!
//! let provider = SdkLoggerProvider::builder()
//!     .register(stdout_logs().level(LevelFilter::Warn))
//!     .unwrap()
//!     .build();
//!
//! let logger = provider.logger("stdout");
//! ```
//!
//! ```
//! use observed_destination_core::MeterProviderBuilderExt;
//! use opentelemetry::metrics::MeterProvider;
//! use opentelemetry_sdk::metrics::SdkMeterProvider;
//!
//! let provider = SdkMeterProvider::builder()
//!     .register(observed_destination_stdout::stdout_metrics())
//!     .unwrap()
//!     .build();
//!
//! provider
//!     .meter("stdout")
//!     .u64_counter("stdout_contract_requests")
//!     .build()
//!     .add(2, &[]);
//!
//! provider.shutdown();
//! ```

#![doc(
    html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/observed_destination_stdout/logo.png"
)]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/observed_destination_stdout/favicon.ico"
)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use std::fmt::Debug;
use std::io::{Write, stdout};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use log::LevelFilter;
use observed_destination_core::{
    DestinationResult, IntoLogDestination, IntoMetricDestination, LogDestination, LogFormatConfig, MetricDestination, ResourceHeader,
    SeverityFilter, print_log_record, severity_of_level,
};
use opentelemetry::InstrumentationScope;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::{OTelSdkError, OTelSdkResult};
use opentelemetry_sdk::logs::{LogProcessor, LoggerProviderBuilder, SdkLogRecord};
use opentelemetry_sdk::metrics::data::ResourceMetrics;
use opentelemetry_sdk::metrics::exporter::PushMetricExporter;
use opentelemetry_sdk::metrics::{MeterProviderBuilder, Temporality};

#[cfg_attr(coverage_nightly, coverage(off))]
fn stdout_writer() -> std::io::StdoutLock<'static> {
    stdout().lock()
}

// Coverage is off because tests cannot make the OS refuse to create a thread.
#[cfg_attr(coverage_nightly, coverage(off))]
#[expect(clippy::needless_pass_by_value, reason = "`Result::map_err` passes the error by value")]
fn flush_spawn_failure(error: std::io::Error) -> OTelSdkError {
    OTelSdkError::InternalFailure(error.to_string())
}

/// Creates a log destination that writes to stdout.
///
/// Optionally filters by severity level (defaults to `Info`). Formatting and writing occur
/// synchronously on the emitting thread which locks the stdout buffer. The destination holds the
/// stdout lock while writing, so a slow write can block that thread and other stdout writers.
///
/// Shutdown stops new records, waits for records that are being written, and then flushes stdout.
/// When this work does not finish within the shutdown timeout, shutdown returns a timeout error.
/// Records that started before shutdown still finish writing after this error.
///
/// # Examples
///
/// ```
/// use log::LevelFilter;
/// use observed_destination_core::{LogDestination, LoggerProviderBuilderExt};
/// use observed_destination_stdout::stdout_logs;
/// use opentelemetry::logs::LoggerProvider;
/// use opentelemetry_sdk::logs::SdkLoggerProvider;
///
/// let provider = SdkLoggerProvider::builder()
///     .register(stdout_logs().level(LevelFilter::Warn))
///     .unwrap()
///     .build();
///
/// let logger = provider.logger("stdout");
/// ```
pub fn stdout_logs() -> StdoutLogDestinationBuilder {
    StdoutLogDestination::builder()
}

/// A log destination that writes records to stdout.
#[derive(bon::Builder, Debug)]
#[builder(derive(Debug))]
pub struct StdoutLogDestination {
    /// Sets the minimum log level (default: `Info`).
    #[builder(default = LevelFilter::Info)]
    level: LevelFilter,

    /// Sets the log format configuration.
    ///
    /// Controls which fields are included in the output (default: [`LogFormatConfig::default()`]).
    #[builder(default = LogFormatConfig::default())]
    format_config: LogFormatConfig,
}

impl StdoutLogDestination {
    fn register_with_output<W: Write>(
        self,
        builder: LoggerProviderBuilder,
        make_writer: impl Fn() -> W + Send + Sync + Clone + 'static,
    ) -> LoggerProviderBuilder {
        let processor = SeverityFilter::new(
            severity_of_level(self.level),
            StdoutLogProcessor::new(self.format_config, make_writer),
        );
        builder.with_log_processor(processor)
    }
}

impl LogDestination for StdoutLogDestination {
    fn register(self, builder: LoggerProviderBuilder) -> DestinationResult<LoggerProviderBuilder> {
        Ok(self.register_with_output(builder, stdout_writer))
    }
}

impl<S: stdout_log_destination_builder::IsComplete> IntoLogDestination for StdoutLogDestinationBuilder<S> {
    type Destination = StdoutLogDestination;

    fn into_log_destination(self) -> StdoutLogDestination {
        self.build()
    }
}

/// A [`LogProcessor`] that writes records straight to stdout from `emit`.
///
/// Stdout writes are synchronous, so there is no need to wrap an async
/// exporter in `block_on` (which panics inside another executor). Records are
/// formatted and printed inline.
#[derive(derive_more::Debug)]
struct StdoutLogProcessor<F> {
    resource_header: ResourceHeader,
    // Bit `SHUTDOWN` marks a shutdown. The other bits count the emits in flight, in
    // units of `ONE_EMISSION`. One atomic holds both values, so its modification order
    // decides each race between an emit and shutdown: shutdown either rejects the emit
    // or sees it and waits for it.
    state: AtomicUsize,
    config: LogFormatConfig,
    #[debug(ignore)]
    make_writer: F,
}

impl<F> StdoutLogProcessor<F> {
    const SHUTDOWN: usize = 1;
    const ONE_EMISSION: usize = 2;

    fn new(config: LogFormatConfig, make_writer: F) -> Self {
        Self {
            resource_header: ResourceHeader::new(),
            state: AtomicUsize::new(0),
            config,
            make_writer,
        }
    }
}

impl<F, W> LogProcessor for StdoutLogProcessor<F>
where
    F: Fn() -> W + Send + Sync + Clone + 'static,
    W: Write,
{
    fn emit(&self, data: &mut SdkLogRecord, instrumentation: &InstrumentationScope) {
        // Relaxed: admission depends only on the modification order of `state`, and
        // the increment publishes no data.
        let admitted = self.state.try_update(Ordering::Relaxed, Ordering::Relaxed, |state| {
            if state & Self::SHUTDOWN == 0 {
                state.checked_add(Self::ONE_EMISSION)
            } else {
                None
            }
        });
        if admitted.is_err() {
            return;
        }

        // The guard also runs when formatting or writing panics, so shutdown does not wait for an emit that ended.
        let _in_flight = scopeguard::guard((), |()| {
            // Release: shutdown acquires this decrement before its flush, so the flush covers the writes of this emit.
            self.state.fetch_sub(Self::ONE_EMISSION, Ordering::Release);
        });

        let mut w = (self.make_writer)();
        let _ = self.resource_header.print_once(&mut w);
        let _ = print_log_record(&mut w, data, instrumentation, self.config);
    }

    fn force_flush(&self) -> OTelSdkResult {
        (self.make_writer)()
            .flush()
            .map_err(|e| OTelSdkError::InternalFailure(e.to_string()))
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        // Acquire: it pairs with the Release decrement of each finished emit, so the flush below covers their writes.
        let mut state = self.state.fetch_or(Self::SHUTDOWN, Ordering::Acquire);

        let start = Instant::now();

        // Wait for the ongoing emissions to finish
        while state & !Self::SHUTDOWN != 0 {
            let remaining = timeout.saturating_sub(start.elapsed());
            // #[gamma::skip(cond.always_false, tag = "timeout", reason = "Without this exit, shutdown polls a blocked emit forever, so the timeout test hangs.")]
            if remaining.is_zero() {
                return Err(OTelSdkError::Timeout(timeout));
            }

            // #[gamma::skip(stmt.delete_call, literal.int_decrement, literal.int_increment, literal.int_to_one, literal.int_to_zero, tag = "equivalent", reason = "The poll interval changes only how often shutdown checks the count; results and timeouts stay the same.")]
            // #[gamma::skip(iter.min_to_max, tag = "timeout", reason = "Sleeping at least 10 ms past the remaining time stalls the one-minute shutdown tests.")]
            std::thread::sleep(remaining.min(Duration::from_millis(10)));
            // #[gamma::skip(stmt.delete_assign, tag = "timeout", reason = "Without a reload, shutdown never sees the emit finish and waits until its one-minute timeout.")]
            state = self.state.load(Ordering::Acquire);
        }

        // Start a new thread and wait until the `timeout`. When no time remains,
        // `recv_timeout` reports a timeout unless the flush has already finished.
        let (tx, rx) = mpsc::channel();
        let make_writer_clone = self.make_writer.clone();
        // #[gamma::skip(try.propagate_to_unwrap, tag = "equivalent", reason = "Tests cannot make the OS refuse to create a thread, so the error path is unreachable in tests.")]
        std::thread::Builder::new()
            .spawn(move || {
                _ = tx.send(make_writer_clone().flush());
            })
            .map_err(flush_spawn_failure)?;
        match rx.recv_timeout(timeout.saturating_sub(start.elapsed())) {
            Ok(result) => result.map_err(|e| OTelSdkError::InternalFailure(e.to_string())),
            // The flush thread drops the sender without a result only when it panics.
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(OTelSdkError::InternalFailure("stdout flush panicked".to_string())),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(OTelSdkError::Timeout(timeout)),
        }
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.resource_header.set_resource(resource);
    }
}

/// Creates a metric destination that writes to stdout.
///
/// Writes cumulative metric batches in a readable debug format. The output
/// includes resource data, instrumentation scopes, attributes, and measurements.
/// This destination is for debugging and development. Its output format can change.
///
/// Metrics are exported periodically, every 60 seconds by default. Set
/// `OTEL_METRIC_EXPORT_INTERVAL` to configure the interval (in milliseconds). Recording a
/// measurement does not write it immediately. Call `force_flush` to export
/// pending measurements before exit; `shutdown` flushes pending measurements
/// and stops the periodic reader.
///
/// Write errors return to the `OpenTelemetry` SDK, which reports export failures.
///
/// # Examples
///
/// ```
/// use observed_destination_core::MeterProviderBuilderExt;
/// use opentelemetry::metrics::MeterProvider;
/// use opentelemetry_sdk::metrics::SdkMeterProvider;
///
/// let provider = SdkMeterProvider::builder()
///     .register(observed_destination_stdout::stdout_metrics())
///     .unwrap()
///     .build();
///
/// provider
///     .meter("stdout")
///     .u64_counter("stdout_contract_requests")
///     .build()
///     .add(2, &[]);
///
/// provider.force_flush().unwrap();
/// provider.shutdown().unwrap();
/// ```
pub fn stdout_metrics() -> StdoutMetricDestinationBuilder {
    StdoutMetricDestination::builder()
}

/// A metric destination that writes to stdout via the `OTel` stdout exporter.
#[derive(bon::Builder, Debug)]
#[builder(derive(Debug))]
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "This exception is due to Bon's macros that don't allow empty structs to be defined without brackets."
)]
pub struct StdoutMetricDestination {}

impl StdoutMetricDestination {
    fn register_with_exporter(builder: MeterProviderBuilder, exporter: impl PushMetricExporter) -> MeterProviderBuilder {
        builder.with_periodic_exporter(exporter)
    }
}

impl MetricDestination for StdoutMetricDestination {
    fn register(self, builder: MeterProviderBuilder) -> DestinationResult<MeterProviderBuilder> {
        Ok(Self::register_with_exporter(builder, StdoutMetricExporter::new(stdout_writer)))
    }
}

impl<S: stdout_metric_destination_builder::IsComplete> IntoMetricDestination for StdoutMetricDestinationBuilder<S> {
    type Destination = StdoutMetricDestination;

    fn into_metric_destination(self) -> StdoutMetricDestination {
        self.build()
    }
}

#[derive(derive_more::Debug)]
struct StdoutMetricExporter<F> {
    is_shutdown: AtomicBool,
    #[debug(ignore)]
    make_writer: F,
}

impl<F> StdoutMetricExporter<F> {
    fn new(make_writer: F) -> Self {
        Self {
            is_shutdown: AtomicBool::new(false),
            make_writer,
        }
    }
}

impl<F, W> PushMetricExporter for StdoutMetricExporter<F>
where
    F: Fn() -> W + Send + Sync + 'static,
    W: Write,
{
    async fn export(&self, metrics: &ResourceMetrics) -> OTelSdkResult {
        if self.is_shutdown.load(Ordering::SeqCst) {
            return Err(OTelSdkError::AlreadyShutdown);
        }
        writeln!((self.make_writer)(), "{metrics:#?}").map_err(|error| OTelSdkError::InternalFailure(error.to_string()))
    }

    fn force_flush(&self) -> OTelSdkResult {
        (self.make_writer)()
            .flush()
            .map_err(|error| OTelSdkError::InternalFailure(error.to_string()))
    }

    fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
        self.is_shutdown.store(true, Ordering::SeqCst);
        Ok(())
    }

    // #[gamma::skip(fn_value.default, reason = "The SDK implements Default for Temporality as Cumulative, so this mutation returns the same value.")]
    fn temporality(&self) -> Temporality {
        Temporality::Cumulative
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::fmt::Write as StdFmtWrite;
    use std::io::{Error, ErrorKind, Result as IoResult, sink};
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::Barrier;
    use std::thread;
    use std::time::SystemTime;

    use opentelemetry::KeyValue;
    use opentelemetry::logs::{LogRecord, Logger, LoggerProvider, Severity};
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::logs::SdkLoggerProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
    use performables::arc::Arc;
    use performables::sync::mutex::Mutex;

    use super::*;

    // 2026-10-30T09:42:47Z
    const TEST_TIMESTAMP_OFFSET: Duration = Duration::from_secs(1_793_353_367);

    static_assertions::assert_impl_all!(StdoutLogDestination: UnwindSafe, RefUnwindSafe, Send, Sync);
    static_assertions::assert_impl_all!(StdoutLogDestinationBuilder: UnwindSafe, RefUnwindSafe, Send, Sync);
    static_assertions::assert_impl_all!(StdoutMetricDestination: UnwindSafe, RefUnwindSafe, Send, Sync);
    static_assertions::assert_impl_all!(StdoutMetricDestinationBuilder: UnwindSafe, RefUnwindSafe, Send, Sync);

    #[derive(Clone, Default)]
    struct RecordingWriter(Arc<Mutex<WriterState>>);

    impl RecordingWriter {
        fn into_factory(self) -> impl Fn() -> Self + Clone {
            move || self.clone()
        }

        fn contents(&self) -> String {
            String::from_utf8(self.0.lock().bytes.clone()).unwrap()
        }
    }

    impl Write for RecordingWriter {
        fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
            let mut state = self.0.lock();
            if state.fail_write {
                return Err(ErrorKind::BrokenPipe.into());
            }
            state.bytes.write(buf)
        }

        fn flush(&mut self) -> IoResult<()> {
            let mut state = self.0.lock();
            state.flushes += 1;
            state.flushed_len = state.bytes.len();
            if state.fail_flush {
                Err(Error::from(ErrorKind::BrokenPipe))
            } else {
                Ok(())
            }
        }
    }

    #[derive(Default)]
    struct WriterState {
        bytes: Vec<u8>,
        flushes: usize,
        // Output length at the last flush. Shows which writes a flush covered.
        flushed_len: usize,
        fail_flush: bool,
        fail_write: bool,
    }

    /// Blocks the first thread that passes until the test opens the gate.
    #[derive(Clone)]
    struct Gate(Arc<GateState>);

    struct GateState {
        armed: AtomicBool,
        entered: Barrier,
        opened: Barrier,
    }

    impl Gate {
        fn new() -> Self {
            Self(Arc::new(GateState {
                armed: AtomicBool::new(true),
                entered: Barrier::new(2),
                opened: Barrier::new(2),
            }))
        }

        fn pass(&self) {
            if self.0.armed.swap(false, Ordering::SeqCst) {
                self.0.entered.wait();
                self.0.opened.wait();
            }
        }

        fn wait_until_blocked(&self) {
            self.0.entered.wait();
        }

        fn open(&self) {
            self.0.opened.wait();
        }
    }

    /// Accepts all writes and blocks its first flush on a gate.
    struct GatedFlushWriter(Gate);

    impl Write for GatedFlushWriter {
        fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> IoResult<()> {
            self.0.pass();
            Ok(())
        }
    }

    struct PanickingFlushWriter;

    impl Write for PanickingFlushWriter {
        fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> IoResult<()> {
            panic!("simulated stdout flush failure")
        }
    }

    /// Creates a processor whose first writer request blocks on `gate`.
    fn gated_processor(output: &RecordingWriter, gate: &Gate) -> StdoutLogProcessor<impl Fn() -> RecordingWriter + Clone + use<>> {
        let output = output.clone();
        let gate = gate.clone();
        StdoutLogProcessor::new(LogFormatConfig::default(), move || {
            gate.pass();
            output.clone()
        })
    }

    fn emit_record(processor: &impl LogProcessor, body: &'static str) {
        let scope = InstrumentationScope::builder("stdout").build();
        let mut record = SdkLoggerProvider::builder().build().logger("stdout").create_log_record();
        record.set_body(body.into());
        record.set_timestamp(SystemTime::UNIX_EPOCH + TEST_TIMESTAMP_OFFSET);
        processor.emit(&mut record, &scope);
    }

    #[test]
    fn formats_processor_debug() {
        let processor = StdoutLogProcessor::new(LogFormatConfig::default(), sink);

        assert!(format!("{processor:?}").contains("StdoutLogProcessor"));
    }

    #[test]
    #[expect(
        clippy::write_with_newline,
        reason = "the newline is part of the contract and should be kept explicit"
    )]
    fn log_destination_filters_records() {
        for (level_filter, outcomes) in [
            // Ensures that the default level filter is Info
            (
                None,
                &[
                    (Severity::Debug, "debug", false),
                    (Severity::Info, "info", true),
                    (Severity::Warn, "warn", true),
                    (Severity::Error, "error", true),
                    (Severity::Fatal, "fatal", true),
                ],
            ),
            // Ensures that the level filter is applied correctly
            (
                Some(LevelFilter::Error),
                &[
                    (Severity::Debug, "debug", false),
                    (Severity::Info, "info", false),
                    (Severity::Warn, "warn", false),
                    (Severity::Error, "error", true),
                    (Severity::Fatal, "fatal", true),
                ],
            ),
        ] {
            let output = RecordingWriter::default();

            let destination = match level_filter {
                Some(level) => stdout_logs().level(level).build(),
                None => stdout_logs().build(),
            };

            let provider = destination
                .register_with_output(SdkLoggerProvider::builder(), output.clone().into_factory())
                .with_resource(Resource::builder_empty().build())
                .build();
            let logger = provider.logger("stdout");

            let mut expected = "<Resource: >\n".to_string();

            for (severity, body, should_emit) in outcomes {
                let mut record = logger.create_log_record();
                record.set_timestamp(SystemTime::UNIX_EPOCH + TEST_TIMESTAMP_OFFSET);
                record.set_observed_timestamp(SystemTime::UNIX_EPOCH + TEST_TIMESTAMP_OFFSET);
                record.set_severity_number(*severity);
                record.set_body((*body).into());
                logger.emit(record);

                if *should_emit {
                    write!(
                        expected,
                        "[2026-10-30T09:42:47.000000000Z] {}: {} [stdout-?]\n",
                        severity.name(),
                        *body
                    )
                    .unwrap();
                }
            }

            let written = output.contents();
            assert_eq!(written, expected, "Failed for level_filter={level_filter:?}");
        }
    }

    #[test]
    fn log_destination_applies_format_configuration() {
        let output = RecordingWriter::default();
        let provider = stdout_logs()
            .format_config(LogFormatConfig {
                format_body: true,
                ..LogFormatConfig::default()
            })
            .build()
            .register_with_output(SdkLoggerProvider::builder(), output.clone().into_factory())
            .with_resource(Resource::builder_empty().build())
            .build();

        let logger = provider.logger("stdout");
        let mut record = logger.create_log_record();
        record.set_timestamp(SystemTime::UNIX_EPOCH + TEST_TIMESTAMP_OFFSET);
        record.set_observed_timestamp(SystemTime::UNIX_EPOCH + TEST_TIMESTAMP_OFFSET);
        record.set_body("request {id}".into());
        record.add_attribute("id", 42);

        logger.emit(record);

        let written = output.contents();
        assert_eq!(written, "<Resource: >\n[2026-10-30T09:42:47.000000000Z] request 42 [stdout-?]\n");
    }

    #[test]
    fn log_destination_forwards_provider_resource() {
        let output = RecordingWriter::default();
        let provider = stdout_logs()
            .build()
            .register_with_output(
                SdkLoggerProvider::builder().with_resource(
                    Resource::builder_empty()
                        .with_attributes([KeyValue::new("service.name", "stdout-contract-resource")])
                        .build(),
                ),
                output.clone().into_factory(),
            )
            .build();

        let logger = provider.logger("stdout");

        let mut record = logger.create_log_record();
        record.set_timestamp(SystemTime::UNIX_EPOCH + TEST_TIMESTAMP_OFFSET);
        record.set_observed_timestamp(SystemTime::UNIX_EPOCH + TEST_TIMESTAMP_OFFSET);
        logger.emit(record);

        // Also emit a second record to verify that the resource header is only printed once.
        let mut record = logger.create_log_record();
        record.set_timestamp(SystemTime::UNIX_EPOCH + TEST_TIMESTAMP_OFFSET);
        record.set_observed_timestamp(SystemTime::UNIX_EPOCH + TEST_TIMESTAMP_OFFSET);
        logger.emit(record);

        let written = output.contents();
        assert_eq!(
            written,
            "<Resource:  service.name=stdout-contract-resource>\n[2026-10-30T09:42:47.000000000Z]  [stdout-?]\n[2026-10-30T09:42:47.000000000Z]  [stdout-?]\n"
        );
    }

    #[test]
    fn processor_shutdown_stops_exporting() {
        let output = RecordingWriter::default();
        let mut processor = StdoutLogProcessor::new(LogFormatConfig::default(), output.clone().into_factory());
        processor.set_resource(&Resource::builder_empty().build());

        let scope = InstrumentationScope::builder("stdout").build();
        let mut record = SdkLoggerProvider::builder().build().logger("stdout").create_log_record();
        record.set_body("before-shutdown".into());
        record.set_timestamp(SystemTime::UNIX_EPOCH + TEST_TIMESTAMP_OFFSET);

        processor.emit(&mut record, &scope);

        let before_shutdown = output.contents();
        assert_eq!(
            before_shutdown,
            "<Resource: >\n[2026-10-30T09:42:47.000000000Z] before-shutdown [stdout-?]\n"
        );

        processor.shutdown_with_timeout(Duration::from_secs(1)).unwrap();
        processor.emit(&mut record, &scope);

        assert_eq!(output.contents(), before_shutdown);
    }

    #[test]
    fn shutdown_waits_for_started_emit_then_flushes_it() {
        let output = RecordingWriter::default();
        let gate = Gate::new();
        let processor = gated_processor(&output, &gate);

        let result = thread::scope(|s| {
            s.spawn(|| emit_record(&processor, "started-before-shutdown"));
            // The emit is admitted and is blocked while it gets its writer.
            gate.wait_until_blocked();

            let shutdown = s.spawn(|| processor.shutdown_with_timeout(Duration::from_mins(1)));
            // Shutdown rejects emits after it starts. A rejected probe shows that shutdown
            // started while the first emit was in flight, so shutdown must wait for it.
            loop {
                let before = output.contents().len();
                emit_record(&processor, "probe");
                if output.contents().len() == before {
                    break;
                }
                thread::yield_now();
            }
            gate.open();
            shutdown.join().unwrap()
        });

        result.unwrap();
        let state = output.0.lock();
        assert!(String::from_utf8_lossy(&state.bytes).contains("started-before-shutdown"));
        // The shutdown flush must cover the record that was in flight when shutdown started.
        assert_eq!(state.flushed_len, state.bytes.len());
    }

    #[test]
    fn shutdown_times_out_while_started_emit_is_blocked() {
        let output = RecordingWriter::default();
        let gate = Gate::new();
        let processor = gated_processor(&output, &gate);
        let timeout = Duration::from_millis(10);

        let result = thread::scope(|s| {
            let emitter = s.spawn(|| emit_record(&processor, "started-before-shutdown"));
            gate.wait_until_blocked();

            // The emit stays blocked during the full shutdown, so shutdown can only give up.
            let result = processor.shutdown_with_timeout(timeout);

            gate.open();
            emitter.join().unwrap();
            result
        });

        let Err(OTelSdkError::Timeout(reported)) = result else {
            panic!("expected Timeout, got {result:?}")
        };
        assert_eq!(reported, timeout);
        // A timed-out shutdown does not cancel a record that was already admitted.
        assert!(output.contents().contains("started-before-shutdown"));
        assert_eq!(output.0.lock().flushes, 0);
    }

    #[test]
    fn shutdown_flushes_writer() {
        let output = RecordingWriter::default();
        let processor = StdoutLogProcessor::new(LogFormatConfig::default(), output.clone().into_factory());

        processor.shutdown_with_timeout(Duration::from_mins(1)).unwrap();

        assert_eq!(output.0.lock().flushes, 1);
    }

    #[test]
    fn shutdown_reports_flush_error() {
        let output = RecordingWriter::default();
        output.0.lock().fail_flush = true;
        let processor = StdoutLogProcessor::new(LogFormatConfig::default(), output.into_factory());

        let error = processor.shutdown_with_timeout(Duration::from_mins(1)).unwrap_err();

        let OTelSdkError::InternalFailure(error_message) = &error else {
            panic!("expected InternalFailure, got {error:?}")
        };
        assert_eq!(error_message, "broken pipe");
    }

    #[test]
    fn shutdown_reports_flush_panic() {
        let processor = StdoutLogProcessor::new(LogFormatConfig::default(), || PanickingFlushWriter);

        let error = processor.shutdown_with_timeout(Duration::from_mins(1)).unwrap_err();

        let OTelSdkError::InternalFailure(error_message) = &error else {
            panic!("expected InternalFailure, got {error:?}")
        };
        assert_eq!(error_message, "stdout flush panicked");
    }

    #[test]
    fn shutdown_times_out_while_flush_is_blocked() {
        let gate = Gate::new();
        let processor = StdoutLogProcessor::new(LogFormatConfig::default(), {
            let gate = gate.clone();
            move || GatedFlushWriter(gate.clone())
        });
        let timeout = Duration::from_millis(10);

        let result = processor.shutdown_with_timeout(timeout);

        // Release the flush thread so that it does not outlive the test.
        gate.wait_until_blocked();
        gate.open();

        let Err(OTelSdkError::Timeout(reported)) = result else {
            panic!("expected Timeout, got {result:?}")
        };
        assert_eq!(reported, timeout);
    }

    #[test]
    fn force_flush_reaches_writer() {
        let output = RecordingWriter::default();
        let processor = StdoutLogProcessor::new(LogFormatConfig::default(), output.clone().into_factory());

        processor.force_flush().unwrap();

        assert_eq!(output.0.lock().flushes, 1);
    }

    #[test]
    fn force_flush_reports_writer_error() {
        let output = RecordingWriter::default();
        output.0.lock().fail_flush = true;
        let processor = StdoutLogProcessor::new(LogFormatConfig::default(), output.into_factory());

        let error = processor.force_flush().unwrap_err();

        let OTelSdkError::InternalFailure(error_message) = &error else {
            panic!("expected InternalFailure, got {error:?}")
        };

        assert_eq!(error_message, "broken pipe");
    }

    fn export_now(exporter: &impl PushMetricExporter, metrics: &ResourceMetrics) -> OTelSdkResult {
        let export = std::pin::pin!(exporter.export(metrics));
        let std::task::Poll::Ready(result) = export.poll(&mut std::task::Context::from_waker(std::task::Waker::noop())) else {
            panic!("a synchronous stdout export must finish on its first poll");
        };
        result
    }

    #[test]
    fn metric_exporter_stops_writing_after_shutdown() {
        let output = RecordingWriter::default();
        let exporter = StdoutMetricExporter::new(output.clone().into_factory());
        let metrics = ResourceMetrics::default();

        export_now(&exporter, &metrics).unwrap();
        exporter.force_flush().unwrap();
        let before_shutdown = output.contents();
        exporter.shutdown_with_timeout(Duration::ZERO).unwrap();

        assert!(matches!(export_now(&exporter, &metrics), Err(OTelSdkError::AlreadyShutdown)));
        assert_eq!(output.contents(), before_shutdown);
        assert!(!before_shutdown.is_empty());
        assert_eq!(output.0.lock().flushes, 1);
        assert_eq!(exporter.temporality(), Temporality::Cumulative);
    }

    #[test]
    fn metric_exporter_reports_write_and_flush_errors() {
        let output = RecordingWriter::default();
        output.0.lock().fail_write = true;
        output.0.lock().fail_flush = true;
        let exporter = StdoutMetricExporter::new(output.into_factory());

        for result in [export_now(&exporter, &ResourceMetrics::default()), exporter.force_flush()] {
            let OTelSdkError::InternalFailure(message) = result.unwrap_err() else {
                panic!("writer failures must reach the SDK");
            };
            assert_eq!(message, Error::from(ErrorKind::BrokenPipe).to_string());
        }
    }

    #[test]
    // Flushing must export pending measurements.
    #[cfg_attr(
        miri,
        ignore = "opentelemetry_sdk metric aggregation reads the realtime clock, unavailable under Miri isolation; native and cargo careful runs retain this coverage"
    )]
    fn metric_destination_eventually_exports_recorded_metrics() {
        const SERVICE_NAME: &str = "test service name";
        const SCOPE_NAME: &str = "stdout";
        const METRIC_NAME: &str = "stdout_contract_requests";

        fn matches_expected_batch(metrics: &ResourceMetrics) -> bool {
            let service_name = metrics.resource().get(&opentelemetry::Key::from_static_str("service.name"));
            let Some(opentelemetry::Value::String(service_name)) = service_name else {
                return false;
            };
            if service_name.as_str() != SERVICE_NAME {
                return false;
            }

            let scope_metrics = metrics.scope_metrics().next().unwrap();
            if scope_metrics.scope().name() != SCOPE_NAME {
                return false;
            }

            let metric = scope_metrics.metrics().next().unwrap();
            if metric.name() != METRIC_NAME {
                return false;
            }

            let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
                return false;
            };

            let mut points = sum.data_points();
            if points.next().is_none_or(|point| point.value() != 2) {
                return false;
            }
            if points.next().is_some() {
                return false;
            }

            true
        }

        let exporter = InMemoryMetricExporter::default();
        let provider = StdoutMetricDestination::register_with_exporter(
            SdkMeterProvider::builder().with_resource(Resource::builder_empty().with_service_name(SERVICE_NAME).build()),
            exporter.clone(),
        )
        .build();

        provider
            .meter(SCOPE_NAME)
            .u64_counter(METRIC_NAME)
            .build()
            .add(2, &[KeyValue::new("route", "/orders")]);

        provider.shutdown().unwrap();

        let metrics = exporter.get_finished_metrics().unwrap();

        assert!(
            metrics.iter().any(matches_expected_batch),
            "exported metrics did not match expected batch"
        );

        let output = RecordingWriter::default();
        let exporter = StdoutMetricExporter::new(output.clone().into_factory());
        for batch in &metrics {
            export_now(&exporter, batch).unwrap();
        }
        let rendered = output.contents();
        for expected in [SERVICE_NAME, SCOPE_NAME, METRIC_NAME, "route", "/orders", "value: 2"] {
            assert!(rendered.contains(expected), "missing {expected} in {rendered}");
        }
    }
}
