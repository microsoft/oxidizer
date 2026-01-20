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
