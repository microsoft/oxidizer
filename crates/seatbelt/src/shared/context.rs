// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;

use tick::Clock;

pub(crate) const DEFAULT_PIPELINE_NAME: &str = "default";

/// Shared configuration and dependencies for a pipeline of resilience middleware.
///
/// Pass a single `PipelineContext` to all middleware in a pipeline (retry, timeout,
/// circuit breaker, etc.) to share a clock and telemetry configuration.
#[derive(Debug)]
#[non_exhaustive]
pub struct PipelineContext<In, Out> {
    clock: Clock,
    name: Cow<'static, str>,
    #[cfg(any(feature = "metrics", test))]
    meter: Option<opentelemetry::metrics::Meter>,
    logs_enabled: bool,
    _in: std::marker::PhantomData<fn() -> In>,
    _out: std::marker::PhantomData<fn() -> Out>,
}

impl<In, Out> PipelineContext<In, Out> {
    /// Create a context with a clock. Initializes with `name = "default"`.
    pub fn new(clock: impl AsRef<Clock>) -> Self {
        Self {
            clock: clock.as_ref().clone(),
            name: Cow::Borrowed(DEFAULT_PIPELINE_NAME),
            #[cfg(any(feature = "metrics", test))]
            meter: None,
            logs_enabled: false,
            _in: std::marker::PhantomData,
            _out: std::marker::PhantomData,
        }
    }

    /// Get the configured clock for timing operations.
    #[must_use]
    #[cfg(any(feature = "retry", feature = "circuit", feature = "timeout", test))]
    pub(crate) fn get_clock(&self) -> &Clock {
        &self.clock
    }

    /// Set the pipeline name for telemetry correlation. Prefer `snake_case`.
    #[must_use]
    pub fn name(mut self, name: impl Into<Cow<'static, str>>) -> Self {
        self.name = name.into();
        self
    }

    /// Enable metrics reporting with the given OpenTelemetry meter provider.
    #[must_use]
    #[cfg(any(feature = "metrics", test))]
    pub fn enable_metrics(self, provider: &dyn opentelemetry::metrics::MeterProvider) -> Self {
        Self {
            meter: Some(crate::metrics::create_meter(provider)),
            ..self
        }
    }

    /// Enable structured logging for resilience events.
    #[must_use]
    #[cfg(any(feature = "logs", test))]
    pub fn enable_logs(self) -> Self {
        Self {
            logs_enabled: true,
            ..self
        }
    }

    #[cfg_attr(
        not(any(feature = "metrics", feature = "logs", test)),
        expect(unused_variables, reason = "unused when logs nor metrics are used")
    )]
    #[cfg(any(feature = "retry", feature = "circuit", feature = "timeout", test))]
    pub(crate) fn create_telemetry(&self, strategy_name: Cow<'static, str>) -> crate::utils::TelemetryHelper {
        crate::utils::TelemetryHelper {
            #[cfg(any(feature = "metrics", test))]
            event_reporter: self.meter.as_ref().map(crate::metrics::create_resilience_event_counter),
            #[cfg(any(feature = "metrics", feature = "logs", test))]
            pipeline_name: self.name.clone(),
            #[cfg(any(feature = "metrics", feature = "logs", test))]
            strategy_name,
            #[cfg(any(feature = "logs", test))]
            logs_enabled: self.logs_enabled,
        }
    }
}

impl<In, Out> Clone for PipelineContext<In, Out> {
    fn clone(&self) -> Self {
        Self {
            clock: self.clock.clone(),
            name: self.name.clone(),
            #[cfg(any(feature = "metrics", test))]
            meter: self.meter.clone(),
            _in: std::marker::PhantomData,
            _out: std::marker::PhantomData,
            logs_enabled: self.logs_enabled,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_new_with_clock_sets_default_pipeline_name() {
        let clock = tick::Clock::new_frozen();
        let ctx = PipelineContext::<(), ()>::new(clock);
        let telemetry = ctx.create_telemetry("test".into());
        assert_eq!(telemetry.pipeline_name.as_ref(), DEFAULT_PIPELINE_NAME);
        // Ensure clock reference behaves (timestamp monotonic relative behaviour not required, just accessible)
        let _ = ctx.get_clock().system_time();
    }

    #[test]
    fn test_name_with_custom_value_sets_name_and_is_owned() {
        let clock = tick::Clock::new_frozen();
        let ctx = PipelineContext::<(), ()>::new(clock).name(String::from("custom_pipeline"));
        let telemetry = ctx.create_telemetry("test".into());
        assert_eq!(telemetry.pipeline_name.as_ref(), "custom_pipeline");
        assert!(matches!(telemetry.pipeline_name, Cow::Owned(_)));
    }

    #[cfg(not(miri))]
    #[test]
    fn test_create_event_reporter_with_multiple_clones_accumulates_events() {
        let clock = tick::Clock::new_frozen();
        let (provider, exporter) = test_meter_provider();

        let ctx = PipelineContext::<(), ()>::new(clock).enable_metrics(&provider);
        let telemetry1 = ctx.create_telemetry("test1".into());
        let telemetry2 = ctx.create_telemetry("test2".into());
        let c1 = telemetry1.event_reporter.unwrap();
        let c2 = telemetry2.event_reporter.unwrap();
        c1.add(1, &[]);
        c2.add(2, &[]);

        provider.force_flush().unwrap();
        let metrics = exporter.get_finished_metrics().unwrap();
        let dump = format!("{metrics:?}");
        assert!(dump.contains("resilience.event"));
        // Basic sanity that total of 3 was recorded somewhere in debug output.
        assert!(dump.contains('3'));
    }

    #[cfg(not(miri))]
    fn test_meter_provider() -> (
        opentelemetry_sdk::metrics::SdkMeterProvider,
        opentelemetry_sdk::metrics::InMemoryMetricExporter,
    ) {
        let exporter = opentelemetry_sdk::metrics::InMemoryMetricExporter::default();
        let provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        (provider, exporter)
    }
}
