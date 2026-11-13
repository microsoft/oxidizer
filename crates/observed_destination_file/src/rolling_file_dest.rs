// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Rolling file log destination with automatic rotation.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use log::LevelFilter;
use observed_destination_core::{
    DestinationResult, IntoLogDestination, LogDestination, LogFormatConfig, SeverityFilter, severity_of_level,
};
use performables::arc::Arc;

use crate::file_writer::FileWriterOpener;
use crate::worker::{WriterOpener, background_procesor};
use crate::{DEFAULT_BUFFER_SIZE, DEFAULT_FLUSH_ACK_TIMEOUT, DEFAULT_FLUSH_INTERVAL, DEFAULT_QUEUE_CAPACITY};

/// Creates a rolling file log destination builder for the given path.
///
/// Requires the `rolling_file` feature.
///
/// # Examples
///
/// ```no_run
/// # #[cfg(feature = "rolling_file")]
/// # {
/// use std::time::Duration;
///
/// use log::LevelFilter;
/// use observed_destination_core::LogFormatConfig;
/// use observed_destination_file::{FileLimit, rolling_file_logs};
///
/// let dest = rolling_file_logs("app.log")
///     // Drop records below DEBUG before they reach the file processor.
///     .level(LevelFilter::Debug)
///     // Rotate after the active log file grows past 32 MiB.
///     .max_file_size(32 * 1024 * 1024)
///     // Keep at most 10 rotated files. Use FileLimit::Age(...) for age retention.
///     .file_limit(FileLimit::MaxFiles(10))
///     // Compress rotated files with gzip.
///     .compression(true)
///     // Use the compact formatter: timestamp, severity, and formatted body.
///     .format_config(LogFormatConfig::minimal())
///     // Buffer up to 16K log records on the background writer channel.
///     .queue_capacity(16 * 1024)
///     // Coalesce formatted bytes before writing to the rotating file.
///     .buffer_size(256 * 1024)
///     // Flush pending bytes even when no more records arrive.
///     .flush_interval(Duration::from_secs(5))
///     // Wait up to 30 seconds for force_flush to be acknowledged.
///     .flush_ack_timeout(Duration::from_secs(30))
///     // Builds the destination.
///     .build();
/// # }
/// ```
pub fn rolling_file_logs(path: impl AsRef<Path>) -> RollingFileLogDestinationBuilder<rolling_file_log_destination_builder::SetPath> {
    RollingFileLogDestination::builder().path(path.as_ref().to_path_buf())
}

/// Policy for retaining rotated log files.
///
/// Either a maximum number of files or an age threshold can be specified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileLimit {
    /// Delete the oldest files if the rotated file count exceeds this limit.
    MaxFiles(usize),
    /// Delete files whose timestamp suffix is older than this duration.
    Age(Duration),
}

impl Default for FileLimit {
    fn default() -> Self {
        Self::Age(Duration::from_hours(24 * 30))
    }
}

impl From<FileLimit> for file_rotate::suffix::FileLimit {
    fn from(value: FileLimit) -> Self {
        match value {
            FileLimit::MaxFiles(files) => Self::MaxFiles(files),
            // `file-rotate` expresses age retention with `chrono::Duration`. Convert here so the
            // public API stays on `std::time::Duration`. Durations too large for `chrono` are
            // clamped to its maximum rather than panicking.
            FileLimit::Age(age) => Self::Age(chrono::Duration::from_std(age).unwrap_or(chrono::Duration::MAX)),
        }
    }
}

/// Builder for a rolling file log destination.
#[derive(bon::Builder, Debug)]
pub struct RollingFileLogDestination {
    #[builder(skip = FileWriterOpener::new())]
    writer_opener: FileWriterOpener,

    /// Path to the log file.
    path: PathBuf,

    /// Sets the minimum log level (default: `Info`).
    #[builder(default = LevelFilter::Info)]
    level: LevelFilter,

    /// Sets the maximum file size in bytes before rotation (default: 10 MB).
    #[builder(default = 10 * 1024 * 1024)]
    max_file_size: usize,

    /// Sets the retention limit for rotated log files (default: 30 days).
    #[builder(default = FileLimit::default())]
    file_limit: FileLimit,

    /// Whether to compress rotated log files with gzip (default: `false`).
    #[builder(default = false)]
    compression: bool,

    /// Sets the log format configuration controlling which fields are included
    /// in the output (default: [`LogFormatConfig::default()`]).
    #[builder(default = LogFormatConfig::default())]
    format_config: LogFormatConfig,

    /// Sets the maximum number of log records buffered in the channel before
    /// new records are dropped and periodically reported in the destination
    /// output as a warning.
    #[builder(default = DEFAULT_QUEUE_CAPACITY)]
    queue_capacity: usize,

    /// Sets the write buffer size in bytes.
    ///
    /// Pending bytes reach the rotating writer when the buffer fills, at
    /// `flush_interval`, on `force_flush`, or on shutdown. Set to zero to disable
    /// write buffering.
    /// The capacity is capped at `max_file_size` to keep write batches
    /// proportional to the rotation threshold.
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

impl LogDestination for RollingFileLogDestination {
    fn register(
        self,
        builder: opentelemetry_sdk::logs::LoggerProviderBuilder,
    ) -> DestinationResult<opentelemetry_sdk::logs::LoggerProviderBuilder> {
        fn rolling_opener(opener: FileWriterOpener, max_file_size: usize, file_limit: FileLimit, compression: bool) -> WriterOpener {
            Arc::new(move |path: &Path| opener.open_rolling(path, max_file_size, file_limit, compression))
                .coerce::<dyn Fn(&Path) -> std::io::Result<Box<dyn Write + Send>> + Send + Sync>(|arc| arc)
        }

        background_procesor(
            self.format_config,
            &rolling_opener(self.writer_opener, self.max_file_size, self.file_limit, self.compression),
            &self.path,
            self.queue_capacity,
            self.buffer_size.min(self.max_file_size),
            self.flush_interval,
            self.flush_ack_timeout,
        )
        .map(|processor| {
            let processor = SeverityFilter::new(severity_of_level(self.level), processor);
            builder.with_log_processor(processor)
        })
    }
}

#[cfg(test)]
impl RollingFileLogDestination {
    pub(crate) fn writer_opener(mut self, opener: FileWriterOpener) -> Self {
        self.writer_opener = opener;
        self
    }
}

impl<S: rolling_file_log_destination_builder::IsComplete> IntoLogDestination for RollingFileLogDestinationBuilder<S> {
    type Destination = RollingFileLogDestination;

    fn into_log_destination(self) -> RollingFileLogDestination {
        self.build()
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Error, ErrorKind};

    use chrono::Duration as ChronoDuration;
    use file_rotate::suffix::FileLimit as RotationFileLimit;
    use observed_destination_core::LoggerProviderBuilderExt;
    use ohno::ErrorExt as _;
    use opentelemetry::logs::{AnyValue, LogRecord as _, Logger as _, LoggerProvider as _, Severity};
    use opentelemetry_sdk::logs::SdkLoggerProvider;

    use super::*;
    use crate::FileLogInitError;
    use crate::file_writer::fakes::{FakeFileWriterOpener, OpenCall};
    use crate::worker::InvalidFlushInterval;

    #[test]
    fn retention_limits_convert_to_the_rotating_writer() {
        assert_eq!(FileLimit::default(), FileLimit::Age(Duration::from_hours(24 * 30)));
        assert!(matches!(
            RotationFileLimit::from(FileLimit::MaxFiles(3)),
            RotationFileLimit::MaxFiles(3)
        ));
        for (age, expected) in [
            (Duration::from_mins(1), ChronoDuration::minutes(1)),
            (Duration::MAX, ChronoDuration::MAX),
        ] {
            let RotationFileLimit::Age(actual) = RotationFileLimit::from(FileLimit::Age(age)) else {
                panic!("expected age-based retention");
            };
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn buffer_size_defaults_to_256_kib_and_is_configurable() {
        assert_eq!(rolling_file_logs("app.log").build().buffer_size, 256 * 1024);
        for size in [0, 64, 1024 * 1024] {
            assert_eq!(rolling_file_logs("app.log").buffer_size(size).build().buffer_size, size);
        }
    }

    #[test]
    fn flush_interval_defaults_to_five_seconds_and_is_configurable() {
        assert_eq!(rolling_file_logs("app.log").build().flush_interval, Duration::from_secs(5));
        let interval = Duration::from_millis(25);
        assert_eq!(
            rolling_file_logs("app.log").flush_interval(interval).build().flush_interval,
            interval
        );
    }

    #[test]
    fn registration_rejects_zero_flush_interval_before_opening_file() {
        let factory = FakeFileWriterOpener::new("");

        let error = SdkLoggerProvider::builder()
            .register(
                rolling_file_logs("application.log")
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
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry default resource detection calls current_exe, unsupported in Miri isolation"
    )]
    fn registration_passes_rotation_options_and_applies_level_and_format() {
        let path = Path::new("application.log");
        for file_limit in [FileLimit::MaxFiles(1), FileLimit::Age(Duration::from_mins(1))] {
            for compression in [false, true] {
                let factory = FakeFileWriterOpener::new("");
                let destination = rolling_file_logs(path)
                    .level(LevelFilter::Warn)
                    .max_file_size(128)
                    .file_limit(file_limit)
                    .compression(compression)
                    .format_config(LogFormatConfig::minimal())
                    .into_log_destination()
                    .writer_opener(FileWriterOpener::fake(factory.clone()));
                let provider = SdkLoggerProvider::builder().register(destination).unwrap().build();
                let logger = provider.logger("test");
                for severity in [Severity::Info, Severity::Warn] {
                    let mut record = logger.create_log_record();
                    record.set_severity_number(severity);
                    record.set_body(AnyValue::String("request {id}".into()));
                    record.add_attribute("id", 42);
                    logger.emit(record);
                }
                provider.shutdown().unwrap();

                assert_eq!(
                    factory.calls(),
                    [OpenCall::Rolling {
                        path: path.to_path_buf(),
                        max_file_size: 128,
                        file_limit,
                        compression,
                    }]
                );
                let output = factory.output();
                assert!(output.contains("WARN: request 42\n"));
                assert!(!output.contains("INFO:"));
                assert!(!output.contains("id="));
            }
        }
    }

    #[test]
    fn registration_reports_file_open_errors() {
        let factory = FakeFileWriterOpener::new("").open_error(ErrorKind::PermissionDenied);
        let error = SdkLoggerProvider::builder()
            .register(
                rolling_file_logs("application.log")
                    .build()
                    .writer_opener(FileWriterOpener::fake(factory.clone())),
            )
            .unwrap_err();

        let init_error = error.downcast_ref::<FileLogInitError>().unwrap();
        assert_eq!(init_error.find_source::<Error>().unwrap().kind(), ErrorKind::PermissionDenied);
        assert!(init_error.find_source::<InvalidFlushInterval>().is_none());
        assert_eq!(factory.calls().len(), 1);
    }
}
