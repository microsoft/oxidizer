#[derive(Debug, Clone)]
pub(crate) struct TelemetryHelper {
    #[cfg(any(feature = "metrics", feature = "logs", test))]
    pub(crate) pipeline_name: std::borrow::Cow<'static, str>,
    #[cfg(any(feature = "metrics", feature = "logs", test))]
    pub(crate) strategy_name: std::borrow::Cow<'static, str>,
    #[cfg(any(feature = "metrics", test))]
    pub(crate) event_reporter: Option<opentelemetry::metrics::Counter<u64>>,
    pub(crate) logs_enabled: bool,
}

impl TelemetryHelper {
    #[cfg(any(feature = "metrics", test))]
    pub fn metrics_enabled(&self) -> bool {
        self.event_reporter.is_some()
    }

    #[cfg(any(feature = "metrics", test))]
    pub fn report_metrics(&self, attributes: &[opentelemetry::KeyValue]) {
        if let Some(reporter) = &self.event_reporter {
            reporter.add(1, attributes);
        }
    }

    #[cfg(not(any(feature = "metrics", test)))]
    pub fn metrics_enabled(&self) -> bool {
        false
    }
}
