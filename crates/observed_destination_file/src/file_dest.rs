// Copyright (c) Microsoft Corporation.

//! Builder for single-file log destinations.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use log::LevelFilter;
use observed_destination_core::{
    DestinationResult, IntoLogDestination, LogDestination, LogFormatConfig, SeverityFilter, severity_of_level,
};
use opentelemetry_sdk::logs::LoggerProviderBuilder;
use performables::arc::Arc;

use crate::file_writer::FileWriterOpener;
use crate::worker::{WriterOpener, background_procesor};
use crate::{DEFAULT_BUFFER_SIZE, DEFAULT_FLUSH_ACK_TIMEOUT, DEFAULT_FLUSH_INTERVAL, DEFAULT_QUEUE_CAPACITY};

/// Creates a file log destination builder for the given path.
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
///
/// use log::LevelFilter;
/// use observed_destination_core::LogFormatConfig;
/// use observed_destination_file::file_logs;
///
/// let dest = file_logs("app.log")
///     // Drop records below WARN before they reach the file processor.
///     .level(LevelFilter::Warn)
///     // Start a fresh file instead of appending to an existing one.
///     .overwrite(true)
///     // Use the compact formatter: timestamp, severity, and formatted body.
///     .format_config(LogFormatConfig::minimal())
///     // Buffer up to 16K log records on the background writer channel.
///     .queue_capacity(16 * 1024)
///     // Coalesce formatted bytes before writing to the file.
///     .buffer_size(256 * 1024)
///     // Flush pending bytes even when no more records arrive.
///     .flush_interval(Duration::from_secs(5))
///     // Wait up to 30 seconds for force_flush to be acknowledged.
///     .flush_ack_timeout(Duration::from_secs(30))
///     .build();
/// ```
pub fn file_logs(path: impl AsRef<Path>) -> FileLogDestinationBuilder<file_log_destination_builder::SetPath> {
    FileLogDestination::builder().path(path.as_ref().to_path_buf())
}

/// Sets up the background writer as `OTel` `LogProcessor`.
#[derive(bon::Builder, Debug)]
pub struct FileLogDestination {
    #[builder(skip = FileWriterOpener::new())]
    writer_opener: FileWriterOpener,

    /// Path to the log file.
    path: PathBuf,

    /// Sets the minimum log level (default: `Info`).
    #[builder(default = LevelFilter::Info)]
    level: LevelFilter,

    /// If `true`, truncate the file on open instead of appending (default: `false`).
    #[builder(default = false)]
    overwrite: bool,

    /// Sets the log format configuration (default: [`LogFormatConfig::default()`]).
    #[builder(default = LogFormatConfig::default())]
    format_config: LogFormatConfig,

    /// Sets the maximum number of log records buffered in the channel before
    /// new records are dropped and periodically reported in the destination
    /// output as a warning.
    #[builder(default = DEFAULT_QUEUE_CAPACITY)]
    queue_capacity: usize,

    /// Sets the write buffer size in bytes.
    ///
    /// Pending bytes are written when the buffer fills, at `flush_interval`,
    /// on `force_flush`, or on shutdown. Set to zero to disable write buffering.
    #[builder(default = DEFAULT_BUFFER_SIZE)]
    buffer_size: usize,

    /// Sets the automatic flush interval.
    ///
    /// Only pending bytes are flushed. New records do not postpone the deadline,
    /// and the worker wakes to flush even when no more records arrive. Actual
    /// timing depends on worker scheduling and I/O. The interval must be nonzero.
    #[builder(default = DEFAULT_FLUSH_INTERVAL)]
    flush_interval: Duration,

    /// Sets the maximum time `force_flush` waits for the background writer to
    /// acknowledge a flush request.
    #[builder(default = DEFAULT_FLUSH_ACK_TIMEOUT)]
    flush_ack_timeout: Duration,
}

impl LogDestination for FileLogDestination {
    fn register(self, builder: LoggerProviderBuilder) -> DestinationResult<LoggerProviderBuilder> {
        fn file_opener(opener: FileWriterOpener, overwrite: bool) -> WriterOpener {
            Arc::new(move |path: &Path| opener.open_file(path, overwrite))
                .coerce::<dyn Fn(&Path) -> std::io::Result<Box<dyn Write + Send>> + Send + Sync>(|arc| arc)
        }

        let processor = background_procesor(
            self.format_config,
            &file_opener(self.writer_opener, self.overwrite),
            &self.path,
            self.queue_capacity,
            self.buffer_size,
            self.flush_interval,
            self.flush_ack_timeout,
        )?;

        let processor = SeverityFilter::new(severity_of_level(self.level), processor);
        Ok(builder.with_log_processor(processor))
    }
}

#[cfg(test)]
impl FileLogDestination {
    pub(crate) fn writer_opener(mut self, opener: FileWriterOpener) -> Self {
        self.writer_opener = opener;
        self
    }
}

impl<S: file_log_destination_builder::IsComplete> IntoLogDestination for FileLogDestinationBuilder<S> {
    type Destination = FileLogDestination;

    fn into_log_destination(self) -> FileLogDestination {
        self.build()
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Error, ErrorKind};

    use observed_destination_core::LoggerProviderBuilderExt;
    use ohno::ErrorExt as _;
    use opentelemetry::logs::{AnyValue, LogRecord as _, Logger as _, LoggerProvider as _, Severity};
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::logs::SdkLoggerProvider;

    use super::*;
    use crate::FileLogInitError;
    use crate::file_writer::fakes::{FakeFileWriterOpener, OpenCall};
    use crate::worker::InvalidFlushInterval;

    #[test]
    fn buffer_size_defaults_to_256_kib_and_is_configurable() {
        assert_eq!(file_logs("app.log").build().buffer_size, 256 * 1024);
        for size in [0, 64, 1024 * 1024] {
            assert_eq!(file_logs("app.log").buffer_size(size).build().buffer_size, size);
        }
    }

    #[test]
    fn flush_interval_defaults_to_five_seconds_and_is_configurable() {
        assert_eq!(file_logs("app.log").build().flush_interval, Duration::from_secs(5));
        let interval = Duration::from_millis(25);
        assert_eq!(file_logs("app.log").flush_interval(interval).build().flush_interval, interval);
    }

    #[test]
    fn registration_rejects_zero_flush_interval_before_opening_file() {
        let factory = FakeFileWriterOpener::new("");
        let error = SdkLoggerProvider::builder()
            .register(
                file_logs("application.log")
                    .flush_interval(Duration::ZERO)
                    .build()
                    .writer_opener(FileWriterOpener::fake(factory.clone())),
            )
            .unwrap_err();

        let init_error = error.downcast_ref::<FileLogInitError>().unwrap();
        assert!(init_error.find_source::<InvalidFlushInterval>().is_some());
        assert!(init_error.find_source::<Error>().is_none());
        assert!(factory.calls().is_empty());
    }

    #[test]
    fn building_destination_does_not_open_writer() {
        let factory = FakeFileWriterOpener::new("existing contents\n");

        let _destination = file_logs("application.log")
            .overwrite(true)
            .into_log_destination()
            .writer_opener(FileWriterOpener::fake(factory.clone()));

        assert!(factory.calls().is_empty());
        assert_eq!(factory.output(), "existing contents\n");
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry default resource detection calls current_exe, unsupported in Miri isolation"
    )]
    fn registration_appends_or_overwrites_existing_contents() {
        let path = Path::new("application.log");

        for overwrite in [false, true] {
            let factory = FakeFileWriterOpener::new("existing contents\n");

            let provider = SdkLoggerProvider::builder()
                .register(
                    file_logs(path)
                        .overwrite(overwrite)
                        .format_config(LogFormatConfig::minimal())
                        .flush_ack_timeout(Duration::from_secs(12))
                        .build()
                        .writer_opener(FileWriterOpener::fake(factory.clone())),
                )
                .unwrap()
                .build();
            let logger = provider.logger("test");
            let mut record = logger.create_log_record();
            record.set_severity_number(Severity::Warn);
            record.set_body(AnyValue::String("new record".into()));

            logger.emit(record);
            provider.force_flush().unwrap();

            let output = factory.output();
            assert_eq!(output.starts_with("existing contents\n"), !overwrite);
            assert!(output.contains("WARN: new record\n"));
            assert_eq!(
                factory.calls(),
                [OpenCall::File {
                    path: path.to_path_buf(),
                    overwrite
                }]
            );

            provider.shutdown().unwrap();
        }
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry default resource detection calls current_exe, unsupported in Miri isolation"
    )]
    fn registration_applies_level_and_format_configuration() {
        let factory = FakeFileWriterOpener::new("");
        let destination = file_logs("application.log")
            .level(LevelFilter::Warn)
            .format_config(LogFormatConfig::minimal())
            .flush_ack_timeout(Duration::from_secs(12))
            .build()
            .writer_opener(FileWriterOpener::fake(factory.clone()));

        let provider = SdkLoggerProvider::builder()
            .with_resource(Resource::builder_empty().build())
            .register(destination)
            .unwrap()
            .build();
        // Force the worker to apply the empty resource before emitting.
        provider.force_flush().unwrap();
        let logger = provider.logger("test");

        for severity in [Severity::Info, Severity::Warn] {
            let mut record = logger.create_log_record();
            record.set_severity_number(severity);
            record.set_body(AnyValue::String("request {id}".into()));
            record.add_attribute("id", 42);
            logger.emit(record);
        }

        provider.force_flush().unwrap();

        let output = factory.output();
        assert!(output.contains("WARN: request 42\n"));
        assert!(!output.contains("INFO:"));
        assert!(!output.contains("id="));

        provider.shutdown().unwrap();
    }

    #[test]
    fn registration_reports_file_open_errors() {
        let factory = FakeFileWriterOpener::new("").open_error(ErrorKind::PermissionDenied);

        let err = SdkLoggerProvider::builder()
            .register(
                file_logs("application.log")
                    .build()
                    .writer_opener(FileWriterOpener::fake(factory.clone())),
            )
            .unwrap_err();

        let init_error = err.downcast_ref::<FileLogInitError>().unwrap();
        assert_eq!(init_error.find_source::<Error>().unwrap().kind(), ErrorKind::PermissionDenied);
        assert!(init_error.find_source::<InvalidFlushInterval>().is_none());

        assert_eq!(factory.calls().len(), 1);
    }
}
