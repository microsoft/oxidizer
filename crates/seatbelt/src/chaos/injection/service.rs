// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::fmt::Debug;
#[cfg(any(feature = "tower-service", test))]
use std::pin::Pin;
use std::sync::Arc;
#[cfg(any(feature = "tower-service", test))]
use std::task::{Context, Poll};

use layered::Service;

use crate::ResilienceContext;
use crate::chaos::injection::*;
use crate::rnd::Rnd;
use crate::typestates::NotSet;
use crate::utils::EnableIf;

/// Injects a user-provided output with a configurable probability instead of
/// calling the inner service.
///
/// `Injection` wraps an inner [`Service`] and, on each request, rolls a random
/// number against the configured [`rate`][InjectionLayer::rate] (or the
/// dynamically computed rate from [`rate_with`][InjectionLayer::rate_with]).
/// When the roll triggers, the inner service is skipped entirely and the
/// configured output factory ([`output_with`][InjectionLayer::output_with] /
/// [`output`][InjectionLayer::output]) produces the response.
///
/// Injection is configured by calling [`Injection::layer`] and using the builder
/// methods on the returned [`InjectionLayer`] instance.
///
/// For comprehensive examples and usage patterns, see the [injection module]
/// documentation.
///
/// [injection module]: crate::chaos::injection
#[derive(Debug)]
pub struct Injection<In, Out, S> {
    pub(super) shared: Arc<InjectionShared<In, Out>>,
    pub(super) inner: S,
}

/// Shared configuration for [`Injection`] middleware.
///
/// This struct is wrapped in an `Arc` to enable cheap cloning of the service.
#[derive(Debug)]
pub(crate) struct InjectionShared<In, Out> {
    pub(crate) rate: InjectionRate<In>,
    pub(crate) enable_if: EnableIf<In>,
    pub(crate) injection_output: InjectionOutput<In, Out>,
    pub(crate) rnd: Rnd,
    #[cfg(any(feature = "logs", feature = "metrics", test))]
    pub(crate) telemetry: crate::utils::TelemetryHelper,
}

impl<In, Out, S: Clone> Clone for Injection<In, Out, S> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            inner: self.inner.clone(),
        }
    }
}

impl<In, Out> Injection<In, Out, ()> {
    /// Creates an [`InjectionLayer`] used to configure the chaos injection middleware.
    ///
    /// The instance returned by this call is a builder and cannot be used to
    /// build a service until the required properties are set:
    /// [`rate`][InjectionLayer::rate] / [`rate_with`][InjectionLayer::rate_with]
    /// and one of
    /// [`output_with`][InjectionLayer::output_with] /
    /// [`output`][InjectionLayer::output] /
    /// [`output_error_with`][InjectionLayer::output_error_with] /
    /// [`output_error`][InjectionLayer::output_error].
    ///
    /// The `name` identifies the injection strategy in telemetry, while
    /// `context` provides configuration shared across multiple resilience
    /// middleware.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use layered::{Execute, Stack};
    /// # use tick::Clock;
    /// # use seatbelt::ResilienceContext;
    /// use seatbelt::chaos::injection::Injection;
    ///
    /// # fn example(context: ResilienceContext<String, String>) {
    /// let injection_layer = Injection::layer("my_injection", &context)
    ///     .rate(0.1)
    ///     .output_with(|_input, _args| "injected".to_string());
    /// # }
    /// ```
    ///
    /// For comprehensive examples, see the [injection module] documentation.
    ///
    /// [injection module]: crate::chaos::injection
    pub fn layer(name: impl Into<Cow<'static, str>>, context: &ResilienceContext<In, Out>) -> InjectionLayer<In, Out, NotSet, NotSet> {
        InjectionLayer::new(name.into(), context)
    }
}

// IMPORTANT: The `layered::Service` impl below and the `tower_service::Service` impl further
// down in this file contain logic-equivalent orchestration code. Any change to the `execute`
// body MUST be mirrored in the `call` body, and vice versa. See crate-level AGENTS.md.
impl<In, Out, S> Service<In> for Injection<In, Out, S>
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

        self.shared.handle_injection(input)
    }
}

/// Future returned by [`Injection`] when used as a tower [`Service`](tower_service::Service).
#[cfg(any(feature = "tower-service", test))]
pub struct InjectionFuture<Out> {
    inner: Pin<Box<dyn Future<Output = Out> + Send>>,
}

#[cfg(any(feature = "tower-service", test))]
impl<Out> Debug for InjectionFuture<Out> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InjectionFuture").finish_non_exhaustive()
    }
}

#[cfg(any(feature = "tower-service", test))]
impl<Out> Future for InjectionFuture<Out> {
    type Output = Out;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

// IMPORTANT: The `tower_service::Service` impl below and the `layered::Service` impl above
// contain logic-equivalent orchestration code. Any change to the `call` body MUST be mirrored
// in the `execute` body, and vice versa. See crate-level AGENTS.md.
#[cfg(any(feature = "tower-service", test))]
impl<Req, Res, Err, S> tower_service::Service<Req> for Injection<Req, Result<Res, Err>, S>
where
    Err: Send + 'static,
    Req: Send + 'static,
    Res: Send + 'static,
    S: tower_service::Service<Req, Response = Res, Error = Err> + Send + Sync + 'static,
    S::Future: Send + 'static,
{
    type Response = Res;
    type Error = Err;
    type Future = InjectionFuture<Result<Res, Err>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Req) -> Self::Future {
        if !self.shared.enable_if.call(&req) {
            let future = self.inner.call(req);
            return InjectionFuture { inner: Box::pin(future) };
        }

        if !self.shared.should_inject(&req) {
            let future = self.inner.call(req);
            return InjectionFuture { inner: Box::pin(future) };
        }

        let shared = Arc::clone(&self.shared);

        InjectionFuture {
            inner: Box::pin(async move { shared.handle_injection(req) }),
        }
    }
}

impl<In: Send + 'static, Out: Send + 'static> InjectionShared<In, Out> {
    fn should_inject(&self, input: &In) -> bool {
        let rate = self.rate.call(input, InjectionRateArgs {}).clamp(0.0, 1.0);
        self.rnd.next_f64() < rate
    }

    fn handle_injection(&self, input: In) -> Out {
        let output = self.injection_output.call(input, InjectionOutputArgs {});

        #[cfg(any(feature = "metrics", test))]
        if self.telemetry.metrics_enabled() {
            use crate::utils::{EVENT_NAME, PIPELINE_NAME, STRATEGY_NAME};

            self.telemetry.report_metrics(&[
                opentelemetry::KeyValue::new(PIPELINE_NAME, self.telemetry.pipeline_name.clone()),
                opentelemetry::KeyValue::new(STRATEGY_NAME, self.telemetry.strategy_name.clone()),
                opentelemetry::KeyValue::new(EVENT_NAME, super::telemetry::INJECTION_EVENT_NAME),
            ]);
        }

        #[cfg(any(feature = "logs", test))]
        if self.telemetry.logs_enabled {
            tracing::event!(
                name: "seatbelt.injection",
                tracing::Level::WARN,
                pipeline.name = %self.telemetry.pipeline_name,
                strategy.name = %self.telemetry.strategy_name,
            );
        }

        output
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(not(miri))] // tokio runtime does not support Miri.
#[cfg(test)]
mod tests {
    use std::future::poll_fn;

    use layered::{Execute, Layer, Stack};

    use super::*;
    use crate::testing::FailReadyService;

    #[tokio::test]
    async fn injection_emits_log() {
        use tracing_subscriber::util::SubscriberInitExt;

        use crate::testing::LogCapture;

        let log_capture = LogCapture::new();
        let _guard = log_capture.subscriber().set_default();

        let clock = tick::Clock::new_frozen();
        let context = ResilienceContext::new(clock).use_logs().name("log_test_pipeline");

        let stack = (
            Injection::layer("log_test_injection", &context)
                .rate(1.0)
                .output_with(|_input, _args| "injected".to_string()),
            Execute::new(|_input: String| async { "normal".to_string() }),
        );

        let service = stack.into_service();
        let _ = service.execute("test".to_string()).await;

        log_capture.assert_contains("seatbelt::chaos::injection");
        log_capture.assert_contains("log_test_pipeline");
        log_capture.assert_contains("log_test_injection");
    }

    #[tokio::test]
    async fn injection_emits_metrics() {
        use opentelemetry::KeyValue;

        use crate::testing::MetricTester;
        use crate::utils::{EVENT_NAME, PIPELINE_NAME, STRATEGY_NAME};

        let metrics = MetricTester::new();
        let clock = tick::Clock::new_frozen();
        let context = ResilienceContext::new(clock)
            .use_metrics(metrics.meter_provider())
            .name("metrics_pipeline");

        let stack = (
            Injection::layer("metrics_injection", &context)
                .rate(1.0)
                .output_with(|_input, _args| "injected".to_string()),
            Execute::new(|_input: String| async { "normal".to_string() }),
        );

        let service = stack.into_service();
        let _ = service.execute("test".to_string()).await;

        metrics.assert_attributes(
            &[
                KeyValue::new(PIPELINE_NAME, "metrics_pipeline"),
                KeyValue::new(STRATEGY_NAME, "metrics_injection"),
                KeyValue::new(EVENT_NAME, "chaos_injection"),
            ],
            Some(3),
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn injection_future_debug_snapshot() {
        let future = InjectionFuture::<String> {
            inner: Box::pin(async { "test".to_string() }),
        };

        insta::assert_debug_snapshot!(future);
    }

    #[tokio::test]
    async fn no_injection_when_rnd_equals_rate() {
        let clock = tick::Clock::new_frozen();
        let context = ResilienceContext::new(clock).name("boundary_test");

        let mut layer = Injection::layer("boundary_injection", &context)
            .rate(0.5)
            .output_with(|_input, _args| "injected".to_string());

        // rnd returns exactly the rate value: 0.5 < 0.5 is false, so no injection.
        layer.rnd = crate::rnd::Rnd::new_fixed(0.5);

        let stack = (layer, Execute::new(|input: String| async move { input }));

        let service = stack.into_service();
        let output = service.execute("original".to_string()).await;
        assert_eq!(output, "original");
    }

    #[tokio::test]
    async fn poll_ready_propagates_inner_error() {
        let context = crate::ResilienceContext::<String, Result<String, String>>::new(tick::Clock::new_frozen()).name("test");
        let layer = Injection::layer("test_injection", &context)
            .rate(0.5)
            .output_with(|_input, _args| Err("injected".to_string()));

        let mut service = layer.layer(FailReadyService);

        poll_fn(|cx| tower_service::Service::poll_ready(&mut service, cx))
            .await
            .unwrap_err();
    }
}
