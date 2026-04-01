// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::fmt::Debug;
#[cfg(any(feature = "tower-service", test))]
use std::pin::Pin;
use std::sync::Arc;
#[cfg(any(feature = "tower-service", test))]
use std::task::{Context, Poll};
use std::time::Duration;

use layered::Service;

use crate::ResilienceContext;
use crate::chaos::latency::*;
use crate::rnd::Rnd;
use crate::typestates::NotSet;
use crate::utils::EnableIf;

/// Injects artificial latency with a configurable probability before calling
/// the inner service.
///
/// `Latency` wraps an inner [`Service`] and, on each request, rolls a random
/// number against the configured [`rate`][LatencyLayer::rate] (or the
/// dynamically computed rate from [`rate_with`][LatencyLayer::rate_with]).
/// When the roll triggers, a delay of the configured duration is applied
/// before forwarding the request to the inner service.
///
/// Latency is configured by calling [`Latency::layer`] and using the builder
/// methods on the returned [`LatencyLayer`] instance.
///
/// For comprehensive examples and usage patterns, see the [latency module]
/// documentation.
///
/// [latency module]: crate::chaos::latency
#[derive(Debug)]
pub struct Latency<In, Out, S> {
    pub(super) shared: Arc<LatencyShared<In, Out>>,
    pub(super) inner: S,
}

/// Shared configuration for [`Latency`] middleware.
///
/// This struct is wrapped in an `Arc` to enable cheap cloning of the service.
#[derive(Debug)]
pub(crate) struct LatencyShared<In, Out> {
    pub(crate) clock: tick::Clock,
    pub(crate) rate: LatencyRate<In>,
    pub(crate) enable_if: EnableIf<In>,
    pub(crate) latency_duration: LatencyDuration<In>,
    pub(crate) rnd: Rnd,
    #[cfg(any(feature = "logs", feature = "metrics", test))]
    pub(crate) telemetry: crate::utils::TelemetryHelper,
    _out: std::marker::PhantomData<fn() -> Out>,
}

impl<In, Out, S: Clone> Clone for Latency<In, Out, S> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            inner: self.inner.clone(),
        }
    }
}

impl<In, Out> Latency<In, Out, ()> {
    /// Creates a [`LatencyLayer`] used to configure the chaos latency middleware.
    ///
    /// The instance returned by this call is a builder and cannot be used to
    /// build a service until the required properties are set:
    /// [`rate`][LatencyLayer::rate] / [`rate_with`][LatencyLayer::rate_with]
    /// and one of
    /// [`latency`][LatencyLayer::latency] /
    /// [`latency_with`][LatencyLayer::latency_with] /
    /// [`latency_range`][LatencyLayer::latency_range].
    ///
    /// The `name` identifies the latency strategy in telemetry, while
    /// `context` provides configuration shared across multiple resilience
    /// middleware.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use std::time::Duration;
    /// # use layered::{Execute, Stack};
    /// # use tick::Clock;
    /// # use seatbelt::ResilienceContext;
    /// use seatbelt::chaos::latency::Latency;
    ///
    /// # fn example(context: ResilienceContext<String, String>) {
    /// let latency_layer = Latency::layer("my_latency", &context)
    ///     .rate(0.1)
    ///     .latency(Duration::from_millis(200));
    /// # }
    /// ```
    ///
    /// For comprehensive examples, see the [latency module] documentation.
    ///
    /// [latency module]: crate::chaos::latency
    pub fn layer(name: impl Into<Cow<'static, str>>, context: &ResilienceContext<In, Out>) -> LatencyLayer<In, Out, NotSet, NotSet> {
        LatencyLayer::new(name.into(), context)
    }
}

// IMPORTANT: The `layered::Service` impl below and the `tower_service::Service` impl further
// down in this file contain logic-equivalent orchestration code. Any change to the `execute`
// body MUST be mirrored in the `call` body, and vice versa. See crate-level AGENTS.md.
impl<In, Out, S> Service<In> for Latency<In, Out, S>
where
    In: Send + 'static,
    Out: Send + 'static,
    S: Service<In, Out = Out>,
{
    type Out = Out;

    async fn execute(&self, input: In) -> Self::Out {
        if !self.shared.enable_if.call(&input) {
            return self.inner.execute(input).await;
        }

        if !self.shared.should_inject(&input) {
            return self.inner.execute(input).await;
        }

        let duration = self.shared.get_latency(&input);
        self.shared.handle_latency(duration);
        self.shared.clock.delay(duration).await;
        self.inner.execute(input).await
    }
}

/// Future returned by [`Latency`] when used as a tower [`Service`](tower_service::Service).
#[cfg(any(feature = "tower-service", test))]
pub struct LatencyFuture<Out> {
    inner: Pin<Box<dyn Future<Output = Out> + Send>>,
}

#[cfg(any(feature = "tower-service", test))]
impl<Out> Debug for LatencyFuture<Out> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LatencyFuture").finish_non_exhaustive()
    }
}

#[cfg(any(feature = "tower-service", test))]
impl<Out> Future for LatencyFuture<Out> {
    type Output = Out;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

// IMPORTANT: The `tower_service::Service` impl below and the `layered::Service` impl above
// contain logic-equivalent orchestration code. Any change to the `call` body MUST be mirrored
// in the `execute` body, and vice versa. See crate-level AGENTS.md.
#[cfg(any(feature = "tower-service", test))]
impl<Req, Res, Err, S> tower_service::Service<Req> for Latency<Req, Result<Res, Err>, S>
where
    Err: Send + 'static,
    Req: Send + 'static,
    Res: Send + 'static,
    S: tower_service::Service<Req, Response = Res, Error = Err> + Send + Sync + 'static,
    S::Future: Send + 'static,
{
    type Response = Res;
    type Error = Err;
    type Future = LatencyFuture<Result<Res, Err>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Req) -> Self::Future {
        if !self.shared.enable_if.call(&req) {
            let future = self.inner.call(req);
            return LatencyFuture { inner: Box::pin(future) };
        }

        if !self.shared.should_inject(&req) {
            let future = self.inner.call(req);
            return LatencyFuture { inner: Box::pin(future) };
        }

        let duration = self.shared.get_latency(&req);
        self.shared.handle_latency(duration);

        let shared = Arc::clone(&self.shared);
        let future = self.inner.call(req);

        LatencyFuture {
            inner: Box::pin(async move {
                shared.clock.delay(duration).await;
                future.await
            }),
        }
    }
}

impl<In: Send + 'static, Out: Send + 'static> LatencyShared<In, Out> {
    #[cfg_attr(test, mutants::skip)] // causes test timeouts
    fn should_inject(&self, input: &In) -> bool {
        let rate = self.rate.call(input, LatencyRateArgs {}).clamp(0.0, 1.0);
        self.rnd.next_f64() < rate
    }

    fn get_latency(&self, input: &In) -> Duration {
        self.latency_duration.call(input, LatencyDurationArgs {})
    }

    fn handle_latency(&self, duration: Duration) {
        #[cfg(any(feature = "metrics", test))]
        if self.telemetry.metrics_enabled() {
            use crate::utils::{EVENT_NAME, PIPELINE_NAME, STRATEGY_NAME};

            self.telemetry.report_metrics(&[
                opentelemetry::KeyValue::new(PIPELINE_NAME, self.telemetry.pipeline_name.clone()),
                opentelemetry::KeyValue::new(STRATEGY_NAME, self.telemetry.strategy_name.clone()),
                opentelemetry::KeyValue::new(EVENT_NAME, super::telemetry::LATENCY_EVENT_NAME),
            ]);
        }

        #[cfg(any(feature = "logs", test))]
        if self.telemetry.logs_enabled {
            tracing::event!(
                name: "seatbelt.chaos.latency",
                tracing::Level::WARN,
                pipeline.name = %self.telemetry.pipeline_name,
                strategy.name = %self.telemetry.strategy_name,
                latency.ms = duration.as_millis(),
            );
        }

        // Suppress unused variable warning when neither logs nor metrics are enabled.
        let _ = duration;
    }
}

/// Creates a new [`LatencyShared`] instance. Used by [`LatencyLayer::layer`].
impl<In, Out> LatencyShared<In, Out> {
    pub(super) fn new(
        clock: tick::Clock,
        rate: LatencyRate<In>,
        enable_if: EnableIf<In>,
        latency_duration: LatencyDuration<In>,
        rnd: Rnd,
        #[cfg(any(feature = "logs", feature = "metrics", test))] telemetry: crate::utils::TelemetryHelper,
    ) -> Self {
        Self {
            clock,
            rate,
            enable_if,
            latency_duration,
            rnd,
            #[cfg(any(feature = "logs", feature = "metrics", test))]
            telemetry,
            _out: std::marker::PhantomData,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::future::poll_fn;

    use layered::{Execute, Layer, Stack};

    use super::*;
    use crate::testing::FailReadyService;

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn latency_emits_log() {
        use tracing_subscriber::util::SubscriberInitExt;

        use crate::testing::LogCapture;

        let log_capture = LogCapture::new();
        let _guard = log_capture.subscriber().set_default();

        let clock = tick::ClockControl::default().auto_advance(Duration::from_millis(200)).to_clock();
        let context = ResilienceContext::new(&clock).use_logs().name("log_test_pipeline");

        let stack = (
            Latency::layer("log_test_latency", &context)
                .rate(1.0)
                .latency(Duration::from_millis(100)),
            Execute::new(|input: String| async move { input }),
        );

        let service = stack.into_service();
        let _ = service.execute("test".to_string()).await;

        log_capture.assert_contains("seatbelt::chaos::latency");
        log_capture.assert_contains("log_test_pipeline");
        log_capture.assert_contains("log_test_latency");
        log_capture.assert_contains("latency.ms=100");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn latency_emits_metrics() {
        use opentelemetry::KeyValue;

        use crate::testing::MetricTester;
        use crate::utils::{EVENT_NAME, PIPELINE_NAME, STRATEGY_NAME};

        let metrics = MetricTester::new();
        let clock = tick::ClockControl::default().auto_advance(Duration::from_millis(200)).to_clock();
        let context = ResilienceContext::new(&clock)
            .use_metrics(metrics.meter_provider())
            .name("metrics_pipeline");

        let stack = (
            Latency::layer("metrics_latency", &context)
                .rate(1.0)
                .latency(Duration::from_millis(50)),
            Execute::new(|input: String| async move { input }),
        );

        let service = stack.into_service();
        let _ = service.execute("test".to_string()).await;

        metrics.assert_attributes(
            &[
                KeyValue::new(PIPELINE_NAME, "metrics_pipeline"),
                KeyValue::new(STRATEGY_NAME, "metrics_latency"),
                KeyValue::new(EVENT_NAME, "chaos_latency"),
            ],
            Some(3),
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn latency_future_debug_snapshot() {
        let future = LatencyFuture::<String> {
            inner: Box::pin(async { "test".to_string() }),
        };

        insta::assert_debug_snapshot!(future);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn no_latency_when_rnd_equals_rate() {
        let clock = tick::Clock::new_frozen();
        let context = ResilienceContext::new(&clock).name("boundary_test");

        let mut layer = Latency::layer("boundary_latency", &context)
            .rate(0.5)
            .latency(Duration::from_millis(100));

        // rnd returns exactly the rate value: 0.5 < 0.5 is false, so no latency.
        layer.rnd = crate::rnd::Rnd::new_fixed(0.5);

        let stack = (layer, Execute::new(|input: String| async move { input }));

        let service = stack.into_service();
        let output = service.execute("original".to_string()).await;
        assert_eq!(output, "original");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn poll_ready_propagates_inner_error() {
        let context = crate::ResilienceContext::<String, Result<String, String>>::new(tick::Clock::new_frozen()).name("test");
        let layer = Latency::layer("test_latency", &context)
            .rate(0.5)
            .latency(Duration::from_millis(100));

        let mut service = layer.layer(FailReadyService);

        poll_fn(|cx| tower_service::Service::poll_ready(&mut service, cx))
            .await
            .unwrap_err();
    }
}
