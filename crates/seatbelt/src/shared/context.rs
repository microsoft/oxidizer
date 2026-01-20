// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;

use tick::Clock;

pub(crate) const DEFAULT_PIPELINE_NAME: &str = "default";

/// Shared options for resilience middleware pipelines.
///
/// `Context` bundles a clock and telemetry primitives (OpenTelemetry
/// meter) that resilience middleware uses to measure time, emit metrics, and
/// report events. Use a single instance to configure a whole resilience
/// pipeline (retries, timeouts, circuit breakers, hedging, rate limiting,
/// etc.).
///
/// The [`pipeline_name`][`Context::pipeline_name`] groups resilience middleware under one logical
/// parent for telemetry correlation. When you reuse the same name across all
/// policies in a pipeline, exported metrics and events will carry the same
/// pipeline attribute, making dashboards and analysis easier.
///
/// You can enable metrics via [`enable_metrics`](Self::enable_metrics) and logs via
/// [`enable_logs`](Self::enable_logs) if you need telemetry for observability.
///
/// # Examples
///
/// Basic usage:
///
/// ```rust
/// # #[cfg(feature = "metrics")]
/// # fn example() {
/// # use opentelemetry::metrics::MeterProvider;
/// # use seatbelt::Context;
/// # use tick::Clock;
/// # fn inner(clock: Clock, meter_provider: &dyn MeterProvider) {
/// // Create a context for a resilience pipeline
/// let ctx = Context::<String, String>::new(&clock)
///     .pipeline_name("auth_pipeline")
///     .enable_metrics(meter_provider);
///
/// // Pass the context to resilience middleware layers
/// // (e.g., Retry::layer("my_retry", &ctx), Timeout::layer("my_timeout", &ctx))
/// # }
/// # }
/// ```
///
/// # Authoring Resilience `Middlewares`
///
/// This crate primitives are designed to make writing custom resilience middleware
/// straightforward and observable. Pass a shared [`Context`] to each
/// middleware that needs telemetry. You can keep a single instance (or derive child
/// scopes from it) to group related middleware under one logical parent so their
/// telemetry automatically shares correlation dimensions (e.g., the pipeline name).
///
/// With `Context` you can:
///
/// - Share a common `Clock` for consistent timing and timeouts
/// - Create and use telemetry instruments via the OpenTelemetry `Meter`
/// - Set a logical parent (`pipeline_name`) that groups middleware together
///
/// ## Middleware pattern
///
/// Resilience middleware typically follows this pattern:
///
/// 1. Accept a [`Context`] in your layer/builder to configure observability
/// 2. Create instruments from the options (e.g., counters, histograms)
/// 3. Wrap the next service/layer and forward requests without hidden global state
/// 4. Report events at key decision points (retries, circuit breaker trips, timeouts, …)
/// 5. Use recovery metadata from error types to make informed retry/backoff decisions
///
/// > Note: The canonical, in depth description of the Oxidizer middleware / service
/// > layering pattern lives in the [`layered`] crate documentation. Resilience
/// > middleware in this crate is expected to follow that same pattern (layer types
/// > implementing `Layer`, constructors taking the next service, no hidden global
/// > state, etc.). If you need to evolve the pattern itself, update it in
/// > `layered` and reference it here instead of diverging.
#[derive(Debug)]
#[non_exhaustive]
pub struct Context<In, Out> {
    clock: Clock,
    pipeline_name: Cow<'static, str>,
    #[cfg(any(feature = "metrics", test))]
    meter: Option<opentelemetry::metrics::Meter>,
    logs_enabled: bool,
    _in: std::marker::PhantomData<fn() -> In>,
    _out: std::marker::PhantomData<fn() -> Out>,
}

impl<In, Out> Context<In, Out> {
    /// Create options with a clock.
    ///
    /// Initializes with `pipeline_name = "default"`. Enable metrics via
    /// [`enable_metrics`](Self::enable_metrics) and logs via
    /// [`enable_logs`](Self::enable_logs) if needed.
    pub fn new(clock: impl AsRef<Clock>) -> Self {
        Self {
            clock: clock.as_ref().clone(),
            pipeline_name: Cow::Borrowed(DEFAULT_PIPELINE_NAME),
            #[cfg(any(feature = "metrics", test))]
            meter: None,
            logs_enabled: false,
            _in: std::marker::PhantomData,
            _out: std::marker::PhantomData,
        }
    }

    /// Get the configured clock for timing operations.
    ///
    /// Middleware use this to measure duration, track timeouts, and perform
    /// other time-related operations from a consistent source.
    #[must_use]
    #[cfg(any(feature = "retry", feature = "circuit", feature = "timeout", test))]
    pub(crate) fn get_clock(&self) -> &Clock {
        &self.clock
    }

    /// Set the logical pipeline name used to group resilience middleware.
    ///
    /// Use the same `pipeline_name` across the policies (retry, timeout,
    /// circuit breaker, etc.) that form one execution pipeline. The name is
    /// attached to emitted metrics/events so they can be correlated. Prefer
    /// `snake_case`, e.g., `user_auth`, `data_ingest`.
    #[must_use]
    pub fn pipeline_name(mut self, name: impl Into<Cow<'static, str>>) -> Self {
        self.pipeline_name = name.into();
        self
    }

    /// Enable metrics reporting with a custom meter provider.
    ///
    /// Metrics are disabled by default. Call this method to enable metrics
    /// reporting using the provided OpenTelemetry meter provider.
    #[must_use]
    #[cfg(any(feature = "metrics", test))]
    pub fn enable_metrics(self, provider: &dyn opentelemetry::metrics::MeterProvider) -> Self {
        Self {
            meter: Some(crate::metrics::create_meter(provider)),
            ..self
        }
    }

    /// Enable structured logging for resilience events.
    ///
    /// Logs are disabled by default. Call this method to enable structured
    /// logging for resilience events like retries, timeouts, and circuit breaker
    /// state changes.
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
            pipeline_name: self.pipeline_name.clone(),
            #[cfg(any(feature = "metrics", feature = "logs", test))]
            strategy_name,
            #[cfg(any(feature = "logs", test))]
            logs_enabled: self.logs_enabled,
        }
    }
}

impl<In, Out> Clone for Context<In, Out> {
    fn clone(&self) -> Self {
        Self {
            clock: self.clock.clone(),
            pipeline_name: self.pipeline_name.clone(),
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
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

    use super::*;

    #[test]
    fn test_new_with_clock_sets_default_pipeline_name() {
        let clock = tick::Clock::new_frozen();
        let ctx = Context::<(), ()>::new(clock);
        let telemetry = ctx.create_telemetry("test".into());
        assert_eq!(telemetry.pipeline_name.as_ref(), DEFAULT_PIPELINE_NAME);
        // Ensure clock reference behaves (timestamp monotonic relative behaviour not required, just accessible)
        let _ = ctx.get_clock().system_time();
    }

    #[test]
    fn test_pipeline_name_with_custom_value_sets_name_and_is_owned() {
        let clock = tick::Clock::new_frozen();
        let ctx = Context::<(), ()>::new(clock).pipeline_name(String::from("custom_pipeline"));
        let telemetry = ctx.create_telemetry("test".into());
        assert_eq!(telemetry.pipeline_name.as_ref(), "custom_pipeline");
        assert!(matches!(telemetry.pipeline_name, Cow::Owned(_)));
    }

    #[cfg(not(miri))]
    #[test]
    fn test_create_event_reporter_with_multiple_clones_accumulates_events() {
        let clock = tick::Clock::new_frozen();
        let (provider, exporter) = test_meter_provider();

        let ctx = Context::<(), ()>::new(clock).enable_metrics(&provider);
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

    fn test_meter_provider() -> (SdkMeterProvider, InMemoryMetricExporter) {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();
        (provider, exporter)
    }
}
