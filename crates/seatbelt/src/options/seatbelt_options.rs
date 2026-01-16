// Copyright (c) Microsoft Corporation.

use std::borrow::Cow;

use opentelemetry::metrics::*;
use tick::Clock;

use crate::telemetry::metrics::*;

pub(crate) const DEFAULT_PIPELINE_NAME: &str = "default";

/// Shared options for resilience middleware pipelines.
///
/// `SeatbeltOptions` bundles a clock and telemetry primitives (OpenTelemetry
/// meter) that resilience middleware uses to measure time, emit metrics, and
/// report events. Use a single instance to configure a whole resilience
/// pipeline (retries, timeouts, circuit breakers, hedging, rate limiting,
/// etc.).
///
/// The [`pipeline_name`][`SeatbeltOptions::pipeline_name`] groups resilience middleware under one logical
/// parent for telemetry correlation. When you reuse the same name across all
/// policies in a pipeline, exported metrics and events will carry the same
/// pipeline attribute, making dashboards and analysis easier.
///
/// You can also override the meter provider via [`meter_provider`](Self::meter_provider)
/// if you need a non-global provider (e.g., tests or custom SDK wiring).
///
/// # Examples
///
/// Basic usage:
///
/// ```rust
/// # use opentelemetry::KeyValue;
/// # use opentelemetry::metrics::MeterProvider;
/// # use seatbelt::SeatbeltOptions;
/// # use seatbelt::telemetry::{EVENT_NAME, PIPELINE_NAME, STRATEGY_NAME};
/// # use tick::Clock;
/// # fn example(clock: Clock, meter_provider: &dyn MeterProvider) {
/// let options = SeatbeltOptions::<String, String>::new(&clock)
///     .pipeline_name("auth_pipeline")
///     .meter_provider(meter_provider);
///
/// // Use the clock for timing operations
/// let start = options.get_clock().instant();
///
/// // Create an event reporter
/// let resilience_counter = options.create_resilience_event_counter();
///
/// // Report an event with required attributes
/// resilience_counter.add(
///     1,
///     &[
///         KeyValue::new(PIPELINE_NAME, options.get_pipeline_name().clone()),
///         KeyValue::new(STRATEGY_NAME, "my_strategy"),
///         KeyValue::new(EVENT_NAME, "my_event"),
///     ],
/// );
/// # }
/// ```
///
/// # Authoring Resilience `MIddlewares`
///
/// This crate’s primitives are designed to make writing custom resilience middleware
/// straightforward and observable. Pass a shared [`SeatbeltOptions`] to each
/// middleware that needs telemetry. You can keep a single instance (or derive child
/// scopes from it) to group related middleware under one logical parent so their
/// telemetry automatically shares correlation dimensions (e.g., the pipeline name).
///
/// With `SeatbeltOptions` you can:
///
/// - Share a common `Clock` for consistent timing and timeouts
/// - Create and use telemetry instruments via the OpenTelemetry `Meter`
/// - Set a logical parent (`pipeline_name`) that groups middleware together
///
/// ## Middleware pattern
///
/// Resilience middleware typically follows this pattern:
///
/// 1. Accept a [`SeatbeltOptions`] in your layer/builder to configure observability
/// 2. Create instruments from the options (e.g., counters, histograms)
/// 3. Wrap the next service/layer and forward requests without hidden global state
/// 4. Report events at key decision points (retries, circuit breaker trips, timeouts, …)
/// 5. Use recovery metadata from error types to make informed retry/backoff decisions
///
/// > Note: The canonical, in‑depth description of the Oxidizer middleware / service
/// > layering pattern lives in the [`layered`] crate documentation. Resilience
/// > middleware in this crate is expected to follow that same pattern (layer types
/// > implementing `Layer`, constructors taking the next service, no hidden global
/// > state, etc.). If you need to evolve the pattern itself, update it in
/// > `layered` and reference it here instead of diverging.
#[derive(Debug)]
#[non_exhaustive]
pub struct SeatbeltOptions<In, Out> {
    clock: Clock,
    pipeline_name: Cow<'static, str>,
    meter: Meter,
    _in: std::marker::PhantomData<fn() -> In>,
    _out: std::marker::PhantomData<fn() -> Out>,
}

impl<In, Out> SeatbeltOptions<In, Out> {
    /// Create options with a clock and the global meter provider.
    ///
    /// Initializes with `pipeline_name = "default"` and a meter from the
    /// global provider. Override the provider later via
    /// [`meter_provider`](Self::meter_provider) if needed.
    pub fn new(clock: impl AsRef<Clock>) -> Self {
        #[cfg(any(feature = "metrics", test))]
        let meter = create_meter(opentelemetry::global::meter_provider().as_ref());

        Self {
            clock: clock.as_ref().clone(),
            pipeline_name: Cow::Borrowed(DEFAULT_PIPELINE_NAME),
            meter,
            _in: std::marker::PhantomData,
            _out: std::marker::PhantomData,
        }
    }

    /// Get the configured clock for timing operations.
    ///
    /// Middlewares use this to measure durations, track timeouts, and perform
    /// other time-related operations from a consistent source.
    #[must_use]
    pub fn get_clock(&self) -> &Clock {
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

    /// Get the configured pipeline name (`default` if not set).
    #[must_use]
    pub fn get_pipeline_name(&self) -> &Cow<'static, str> {
        &self.pipeline_name
    }

    /// Override the global meter provider with a custom one.
    #[must_use]
    pub fn meter_provider(self, provider: &dyn MeterProvider) -> Self {
        let meter = create_meter(provider);

        Self { meter, ..self }
    }

    /// Get the configured OpenTelemetry meter.
    ///
    /// Use this to create counters, histograms, and gauges for middleware
    /// observability. The meter uses:
    ///
    /// - Name: `seatbelt`
    /// - Version: `v0.1.0`
    /// - Schema URL: `https://opentelemetry.io/schemas/1.47.0`
    #[must_use]
    pub fn get_meter(&self) -> &Meter {
        &self.meter
    }

    /// Creates standardized counter for resilience events.
    ///
    /// Returns a counter that can be used to report events occurring within
    /// resilience middleware.
    ///
    /// # Required Attributes
    ///
    /// When reporting events, the following attributes MUST be added:
    ///
    /// - [`PIPELINE_NAME`][crate::telemetry::PIPELINE_NAME]: The name of the pipeline this middleware belongs to
    /// - [`STRATEGY_NAME`][crate::telemetry::STRATEGY_NAME]: The name of the specific strategy/middleware
    /// - [`EVENT_NAME`][crate::telemetry::EVENT_NAME]: The name of the event being reported
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use tick::Clock;
    /// # use opentelemetry::KeyValue;
    /// # use opentelemetry::metrics::Meter;
    /// # use seatbelt::SeatbeltOptions;
    /// # use seatbelt::telemetry::{PIPELINE_NAME, STRATEGY_NAME, EVENT_NAME};
    /// # fn example(seatbelt_options: &SeatbeltOptions<(), ()>) {
    /// let counter = seatbelt_options.create_resilience_event_counter();
    ///
    /// counter.add(
    ///     1,
    ///     &[
    ///         KeyValue::new(PIPELINE_NAME, "my_pipeline"),
    ///         KeyValue::new(STRATEGY_NAME, "my_strategy"),
    ///         KeyValue::new(EVENT_NAME, "my_event"),
    ///     ],
    /// );
    /// # }
    /// ```
    #[must_use]
    pub fn create_resilience_event_counter(&self) -> Counter<u64> {
        create_resilience_event_counter(self.get_meter())
    }
}

impl<In, Out> Clone for SeatbeltOptions<In, Out> {
    fn clone(&self) -> Self {
        Self {
            clock: self.clock.clone(),
            pipeline_name: self.pipeline_name.clone(),
            meter: self.meter.clone(),
            _in: std::marker::PhantomData,
            _out: std::marker::PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

    use super::*;

    #[test]
    fn test_new_with_clock_sets_default_pipeline_name() {
        let clock = tick::Clock::new_frozen();
        let options = SeatbeltOptions::<(), ()>::new(clock);
        assert_eq!(options.get_pipeline_name().as_ref(), DEFAULT_PIPELINE_NAME);
        // Ensure clock reference behaves (timestamp monotonic relative behaviour not required, just accessible)
        let _ = options.get_clock().system_time();
    }

    #[test]
    fn test_pipeline_name_with_custom_value_sets_name_and_is_owned() {
        let clock = tick::Clock::new_frozen();
        let options =
            SeatbeltOptions::<(), ()>::new(clock).pipeline_name(String::from("custom_pipeline"));
        assert_eq!(options.get_pipeline_name().as_ref(), "custom_pipeline");
        assert!(matches!(options.get_pipeline_name(), Cow::Owned(_)));
    }

    #[cfg(not(miri))]
    #[test]
    fn test_create_event_reporter_with_multiple_clones_accumulates_events() {
        let clock = tick::Clock::new_frozen();
        let (provider, exporter) = test_meter_provider();

        let options = SeatbeltOptions::<(), ()>::new(clock).meter_provider(&provider);
        let c1 = create_resilience_event_counter(options.get_meter());
        let c2 = create_resilience_event_counter(options.get_meter());
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
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        (provider, exporter)
    }
}
