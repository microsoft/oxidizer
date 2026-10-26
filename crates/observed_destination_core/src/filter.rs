// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Log-level filtering for log destinations.

use std::sync::atomic;
use std::time::Duration;

use opentelemetry::logs::Severity;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::{LogProcessor, SdkLogRecord};
/// A log-level (severity) filtering processor that wraps another [`LogProcessor`].
///
/// Records with severity below the configured threshold are dropped.
pub struct SeverityFilter<T: LogProcessor> {
    severity: Option<Severity>,
    inner: T,
    is_shutdown: atomic::AtomicBool,
}

impl<T: LogProcessor> SeverityFilter<T> {
    /// Creates a new [`SeverityFilter`] that wraps the given inner processor.
    /// For `severity`, `None` disables all logging, while `Some(severity)` drops records below that severity.
    pub fn new(severity: Option<Severity>, inner: T) -> Self {
        Self {
            severity,
            inner,
            is_shutdown: atomic::AtomicBool::new(false),
        }
    }
}

impl<T: LogProcessor> std::fmt::Debug for SeverityFilter<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SeverityFilter({}, {:?})",
            self.severity.map_or("None", |s| s.name()),
            self.inner
        )
    }
}

impl<T: LogProcessor> LogProcessor for SeverityFilter<T> {
    fn event_enabled(&self, level: Severity, target: &str, name: Option<&str>) -> bool {
        self.severity.is_some_and(|s| {
            !self.is_shutdown.load(atomic::Ordering::SeqCst) && level >= s && self.inner.event_enabled(level, target, name)
        })
    }

    fn emit(&self, data: &mut SdkLogRecord, instrumentation: &opentelemetry::InstrumentationScope) {
        let Some(severity) = self.severity else {
            return;
        };

        if self.is_shutdown.load(atomic::Ordering::SeqCst) {
            return;
        }

        let record_severity = data.severity_number().unwrap_or(Severity::Fatal);

        if record_severity >= severity {
            self.inner.emit(data, instrumentation);
        }
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }

    fn shutdown(&self) -> OTelSdkResult {
        self.is_shutdown.store(true, atomic::Ordering::SeqCst);
        self.inner.shutdown()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.is_shutdown.store(true, atomic::Ordering::SeqCst);
        self.inner.shutdown_with_timeout(timeout)
    }

    fn set_resource(&mut self, resource: &opentelemetry_sdk::Resource) {
        self.inner.set_resource(resource);
    }
}

/// Converts a [`log::LevelFilter`] to an `OTel` [`Severity`].
/// Returns `None` for `log::LevelFilter::Off` which disables all logging.
#[must_use]
pub const fn severity_of_level(level: log::LevelFilter) -> Option<Severity> {
    match level {
        log::LevelFilter::Trace => Some(Severity::Trace),
        log::LevelFilter::Debug => Some(Severity::Debug),
        log::LevelFilter::Info => Some(Severity::Info),
        log::LevelFilter::Warn => Some(Severity::Warn),
        log::LevelFilter::Error => Some(Severity::Error),
        log::LevelFilter::Off => None,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use log::LevelFilter;
    use opentelemetry::InstrumentationScope;
    use opentelemetry::logs::{LogRecord as _, Logger as _, LoggerProvider as _};
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::logs::SdkLoggerProvider;
    use performables::arc::Arc;
    use performables::sync::mutex::Mutex;

    use super::*;

    #[derive(Debug, Default)]
    struct RecordingProcessor(Arc<Mutex<Vec<&'static str>>>);

    impl LogProcessor for RecordingProcessor {
        fn emit(&self, _: &mut SdkLogRecord, _: &InstrumentationScope) {
            self.0.lock().push("emit");
        }

        fn force_flush(&self) -> OTelSdkResult {
            self.0.lock().push("flush");
            Ok(())
        }

        fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
            assert_eq!(timeout, Duration::from_secs(5));
            self.0.lock().push("shutdown_with_timeout");
            Ok(())
        }

        fn set_resource(&mut self, resource: &Resource) {
            assert_eq!(resource.get(&"service.name".into()), Some("example".into()));
            self.0.lock().push("resource");
        }
    }

    #[test]
    fn shutdown_with_timeout_propagates_to_inner() {
        let inner = RecordingProcessor::default();
        let calls = Arc::clone(&inner.0);
        let filter = SeverityFilter::new(Some(Severity::Warn), inner);

        filter.shutdown_with_timeout(Duration::from_secs(5)).unwrap();

        assert_eq!(*calls.lock(), ["shutdown_with_timeout"]);
        assert!(filter.is_shutdown.into_inner());
    }

    #[test]
    fn severity_threshold_preserves_lifecycle_and_accepts_unspecified_severity() {
        let inner = RecordingProcessor::default();
        let calls = Arc::clone(&inner.0);
        let mut filter = SeverityFilter::new(Some(Severity::Warn), inner);
        assert!(format!("{filter:?}").contains("WARN"));

        filter.set_resource(&Resource::builder_empty().with_service_name("example").build());

        let provider = SdkLoggerProvider::builder().build();
        let logger = provider.logger("test");
        let scope = InstrumentationScope::builder("test").build();
        let mut record = logger.create_log_record();

        filter.emit(&mut record, &scope);

        for (level, severity) in [
            (LevelFilter::Trace, Some(Severity::Trace)),
            (LevelFilter::Debug, Some(Severity::Debug)),
            (LevelFilter::Info, Some(Severity::Info)),
            (LevelFilter::Warn, Some(Severity::Warn)),
            (LevelFilter::Error, Some(Severity::Error)),
            (LevelFilter::Off, None),
        ] {
            assert_eq!(severity_of_level(level), severity);
        }

        for (severity, should_emit) in [(Severity::Info, false), (Severity::Warn, true)] {
            record.set_severity_number(severity);
            let calls_before = calls.lock().len();

            filter.emit(&mut record, &scope);

            assert_eq!(
                calls.lock().len(),
                calls_before + usize::from(should_emit),
                "unexpected filtering for {severity:?}"
            );
        }

        filter.force_flush().unwrap();
        filter.shutdown().unwrap();
        filter.emit(&mut record, &scope);

        assert_eq!(*calls.lock(), ["resource", "emit", "emit", "flush", "shutdown_with_timeout"]);
    }

    #[test]
    fn severity_interest_matches_emission_for_each_log_level() {
        use opentelemetry_sdk::logs::{InMemoryLogExporter, SimpleLogProcessor};

        for level in [
            LevelFilter::Trace,
            LevelFilter::Debug,
            LevelFilter::Info,
            LevelFilter::Warn,
            LevelFilter::Error,
            LevelFilter::Off,
        ] {
            let exporter = InMemoryLogExporter::default();
            let filter = SeverityFilter::new(severity_of_level(level), SimpleLogProcessor::new(exporter.clone()));
            let provider = SdkLoggerProvider::builder().build();
            let logger = provider.logger("severity-test");
            let scope = InstrumentationScope::builder("severity-test").build();
            let mut expected_count = 0;

            for severity in [
                Severity::Trace,
                Severity::Debug,
                Severity::Info,
                Severity::Warn,
                Severity::Error,
                Severity::Fatal,
            ] {
                let expected = severity_of_level(level).map_or_else(|| false, |s| severity >= s);
                assert_eq!(
                    filter.event_enabled(severity, "target", Some("event")),
                    expected,
                    "{level:?} / {severity:?}"
                );
                let mut record = logger.create_log_record();
                record.set_severity_number(severity);
                filter.emit(&mut record, &scope);
                expected_count += usize::from(expected);
            }

            assert_eq!(exporter.get_emitted_logs().unwrap().len(), expected_count);
            filter.shutdown().unwrap();
            assert!(!filter.event_enabled(Severity::Fatal, "target", Some("event")));
            let mut record = logger.create_log_record();
            record.set_severity_number(Severity::Fatal);
            filter.emit(&mut record, &scope);
            assert!(exporter.get_emitted_logs().unwrap().is_empty());
        }
    }

    #[derive(Debug)]
    struct InterestProbe(Arc<AtomicUsize>);

    impl LogProcessor for InterestProbe {
        fn emit(&self, _: &mut SdkLogRecord, _: &InstrumentationScope) {}

        fn force_flush(&self) -> OTelSdkResult {
            Ok(())
        }

        fn event_enabled(&self, level: Severity, target: &str, name: Option<&str>) -> bool {
            self.0.fetch_add(1, Ordering::Relaxed);
            level == Severity::Error && target == "included" && name == Some("included")
        }
    }

    #[test]
    fn severity_interest_preserves_the_inner_filter() {
        let calls = Arc::new(AtomicUsize::new(0));
        let filter = SeverityFilter::new(Some(Severity::Warn), InterestProbe(Arc::clone(&calls)));

        assert!(!filter.event_enabled(Severity::Info, "included", Some("included")));
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert!(!filter.event_enabled(Severity::Error, "excluded", Some("included")));
        assert!(!filter.event_enabled(Severity::Error, "included", Some("excluded")));
        assert!(!filter.event_enabled(Severity::Warn, "included", Some("included")));
        assert!(filter.event_enabled(Severity::Error, "included", Some("included")));
        assert_eq!(calls.load(Ordering::Relaxed), 4);

        filter.shutdown().unwrap();
        assert!(!filter.event_enabled(Severity::Error, "included", Some("included")));
        assert_eq!(calls.load(Ordering::Relaxed), 4);
    }
}
