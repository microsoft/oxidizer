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

use crate::fallback::*;
use crate::utils::EnableIf;
use crate::{NotSet, ResilienceContext};

/// Provides a replacement output when the inner service output is not considered valid.
///
/// `Fallback` wraps an inner [`Service`] and inspects its output via a
/// user-supplied predicate ([`should_fallback`][FallbackLayer::should_fallback]).
/// When the predicate returns `true` the original output is forwarded to a
/// fallback action ([`fallback`][FallbackLayer::fallback] /
/// [`fallback_async`][FallbackLayer::fallback_async]) that produces the
/// replacement value.
///
/// Fallback is configured by calling [`Fallback::layer`] and using the builder
/// methods on the returned [`FallbackLayer`] instance.
///
/// For comprehensive examples and usage patterns, see the [fallback module]
/// documentation.
///
/// [fallback module]: crate::fallback
#[derive(Debug)]
pub struct Fallback<In, Out, S> {
    pub(super) shared: Arc<FallbackShared<In, Out>>,
    pub(super) inner: S,
}

/// Shared configuration for [`Fallback`] middleware.
///
/// This struct is wrapped in an `Arc` to enable cheap cloning of the service.
#[derive(Debug)]
pub(crate) struct FallbackShared<In, Out> {
    pub(crate) enable_if: EnableIf<In>,
    pub(crate) should_fallback: ShouldFallback<Out>,
    pub(crate) fallback_action: FallbackAction<Out>,
    #[cfg(any(feature = "logs", feature = "metrics", test))]
    pub(crate) telemetry: crate::utils::TelemetryHelper,
}

impl<In, Out, S: Clone> Clone for Fallback<In, Out, S> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            inner: self.inner.clone(),
        }
    }
}

impl<In, Out> Fallback<In, Out, ()> {
    /// Creates a [`FallbackLayer`] used to configure the fallback resilience middleware.
    ///
    /// The instance returned by this call is a builder and cannot be used to
    /// build a service until the required properties are set:
    /// [`should_fallback`][FallbackLayer::should_fallback] and one of
    /// [`fallback`][FallbackLayer::fallback] /
    /// [`fallback_async`][FallbackLayer::fallback_async].
    ///
    /// The `name` identifies the fallback strategy in telemetry, while
    /// `context` provides configuration shared across multiple resilience
    /// middleware.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use layered::{Execute, Stack};
    /// # use tick::Clock;
    /// # use seatbelt::ResilienceContext;
    /// use seatbelt::fallback::Fallback;
    ///
    /// # fn example(context: ResilienceContext<String, String>) {
    /// let fallback_layer = Fallback::layer("my_fallback", &context)
    ///     .should_fallback(|output: &String| output == "bad")
    ///     .fallback(|_output: String, _args| "replacement".to_string());
    /// # }
    /// ```
    ///
    /// For comprehensive examples, see the [fallback module] documentation.
    ///
    /// [fallback module]: crate::fallback
    pub fn layer(name: impl Into<Cow<'static, str>>, context: &ResilienceContext<In, Out>) -> FallbackLayer<In, Out, NotSet, NotSet> {
        FallbackLayer::new(name.into(), context)
    }
}

// IMPORTANT: The `layered::Service` impl below and the `tower_service::Service` impl further
// down in this file contain logic-equivalent orchestration code. Any change to the `execute`
// body MUST be mirrored in the `call` body, and vice versa. See crate-level AGENTS.md.
impl<In, Out, S> Service<In> for Fallback<In, Out, S>
where
    In: Send,
    Out: Send + 'static,
    S: Service<In, Out = Out>,
{
    type Out = Out;

    async fn execute(&self, input: In) -> Self::Out {
        if !self.shared.enable_if.call(&input) {
            return self.inner.execute(input).await;
        }

        let output = self.inner.execute(input).await;

        if !self.shared.should_fallback.call(&output) {
            return output;
        }

        self.shared.handle_fallback(output).await
    }
}

/// Future returned by [`Fallback`] when used as a tower [`Service`](tower_service::Service).
#[cfg(any(feature = "tower-service", test))]
pub struct FallbackFuture<Out> {
    inner: Pin<Box<dyn Future<Output = Out> + Send>>,
}

#[cfg(any(feature = "tower-service", test))]
impl<Out> Debug for FallbackFuture<Out> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FallbackFuture").finish_non_exhaustive()
    }
}

#[cfg(any(feature = "tower-service", test))]
impl<Out> Future for FallbackFuture<Out> {
    type Output = Out;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

// IMPORTANT: The `tower_service::Service` impl below and the `layered::Service` impl above
// contain logic-equivalent orchestration code. Any change to the `call` body MUST be mirrored
// in the `execute` body, and vice versa. See crate-level AGENTS.md.
#[cfg(any(feature = "tower-service", test))]
impl<Req, Res, Err, S> tower_service::Service<Req> for Fallback<Req, Result<Res, Err>, S>
where
    Err: Send + 'static,
    Req: Send + 'static,
    Res: Send + 'static,
    S: tower_service::Service<Req, Response = Res, Error = Err> + Send + Sync + 'static,
    S::Future: Send + 'static,
{
    type Response = Res;
    type Error = Err;
    type Future = FallbackFuture<Result<Res, Err>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Req) -> Self::Future {
        if !self.shared.enable_if.call(&req) {
            let future = self.inner.call(req);
            return FallbackFuture { inner: Box::pin(future) };
        }

        let shared = Arc::clone(&self.shared);
        let future = self.inner.call(req);

        FallbackFuture {
            inner: Box::pin(async move {
                let output = future.await;

                if !shared.should_fallback.call(&output) {
                    return output;
                }

                shared.handle_fallback(output).await
            }),
        }
    }
}

impl<In, Out: Send + 'static> FallbackShared<In, Out> {
    async fn handle_fallback(&self, output: Out) -> Out {
        let new_output = self.fallback_action.call(output, FallbackActionArgs {}).await;

        #[cfg(any(feature = "metrics", test))]
        if self.telemetry.metrics_enabled() {
            use crate::utils::{EVENT_NAME, PIPELINE_NAME, STRATEGY_NAME};

            self.telemetry.report_metrics(&[
                opentelemetry::KeyValue::new(PIPELINE_NAME, self.telemetry.pipeline_name.clone()),
                opentelemetry::KeyValue::new(STRATEGY_NAME, self.telemetry.strategy_name.clone()),
                opentelemetry::KeyValue::new(EVENT_NAME, super::telemetry::FALLBACK_EVENT_NAME),
            ]);
        }

        #[cfg(any(feature = "logs", test))]
        if self.telemetry.logs_enabled {
            tracing::event!(
                name: "seatbelt.fallback",
                tracing::Level::WARN,
                pipeline.name = %self.telemetry.pipeline_name,
                strategy.name = %self.telemetry.strategy_name,
            );
        }

        new_output
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
    async fn fallback_emits_log() {
        use tracing_subscriber::util::SubscriberInitExt;

        use crate::testing::LogCapture;

        let log_capture = LogCapture::new();
        let _guard = log_capture.subscriber().set_default();

        let clock = tick::Clock::new_frozen();
        let context = ResilienceContext::new(clock).use_logs().name("log_test_pipeline");

        let stack = (
            Fallback::layer("log_test_fallback", &context)
                .should_fallback(|output: &String| output == "bad")
                .fallback(|_output: String, _args| "replaced".to_string()),
            Execute::new(|_input: String| async { "bad".to_string() }),
        );

        let service = stack.into_service();
        let _ = service.execute("test".to_string()).await;

        log_capture.assert_contains("seatbelt::fallback");
        log_capture.assert_contains("log_test_pipeline");
        log_capture.assert_contains("log_test_fallback");
    }

    #[tokio::test]
    async fn fallback_emits_metrics() {
        use opentelemetry::KeyValue;

        use crate::testing::MetricTester;
        use crate::utils::{EVENT_NAME, PIPELINE_NAME, STRATEGY_NAME};

        let metrics = MetricTester::new();
        let clock = tick::Clock::new_frozen();
        let context = ResilienceContext::new(clock)
            .use_metrics(metrics.meter_provider())
            .name("metrics_pipeline");

        let stack = (
            Fallback::layer("metrics_fallback", &context)
                .should_fallback(|output: &String| output == "bad")
                .fallback(|_output: String, _args| "replaced".to_string()),
            Execute::new(|_input: String| async { "bad".to_string() }),
        );

        let service = stack.into_service();
        let _ = service.execute("test".to_string()).await;

        metrics.assert_attributes(
            &[
                KeyValue::new(PIPELINE_NAME, "metrics_pipeline"),
                KeyValue::new(STRATEGY_NAME, "metrics_fallback"),
                KeyValue::new(EVENT_NAME, "fallback"),
            ],
            Some(3),
        );
    }

    #[test]
    fn fallback_future_debug_contains_struct_name() {
        let future = FallbackFuture::<String> {
            inner: Box::pin(async { "test".to_string() }),
        };
        let debug_output = format!("{future:?}");

        assert!(debug_output.contains("FallbackFuture"));
    }

    #[tokio::test]
    async fn poll_ready_propagates_inner_error() {
        let context = crate::ResilienceContext::<String, Result<String, String>>::new(tick::Clock::new_frozen()).name("test");
        let layer = Fallback::layer("test_fallback", &context)
            .should_fallback(|output: &Result<String, String>| output.is_err())
            .fallback(|_output, _args| Ok("fallback".to_string()));

        let mut service = layer.layer(FailReadyService);

        poll_fn(|cx| tower_service::Service::poll_ready(&mut service, cx))
            .await
            .unwrap_err();
    }
}
