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
use tick::{Clock, FutureExt};

use crate::timeout::*;
use crate::utils::EnableIf;
use crate::{NotSet, ResilienceContext};

/// Applies a timeout to service execution for canceling long-running operations.
///
/// `Timeout` wraps an inner [`Service`] and enforces a maximum duration for
/// each call. If the operation doesn't finish in time, an output that represents
/// the timeout is returned. This middleware is designed to be used across
/// services, applications, and libraries to prevent operations from hanging indefinitely.
///
/// Timeouts are configured by calling [`Timeout::layer`](crate::timeout::Timeout::layer)
/// and using the builder methods on the returned [`TimeoutLayer`] instance.
///
/// For comprehensive examples and usage patterns, see the [timeout module] documentation.
///
/// [timeout module]: crate::timeout
#[derive(Debug)]
pub struct Timeout<In, Out, S> {
    pub(super) shared: Arc<TimeoutShared<In, Out>>,
    pub(super) inner: S,
}

/// Shared configuration for [`Timeout`] middleware.
///
/// This struct is wrapped in an `Arc` to enable cheap cloning of the service.
#[derive(Debug)]
pub(crate) struct TimeoutShared<In, Out> {
    pub(crate) clock: Clock,
    pub(crate) timeout: Duration,
    pub(crate) enable_if: EnableIf<In>,
    pub(crate) on_timeout: Option<OnTimeout<Out>>,
    pub(crate) timeout_override: Option<TimeoutOverride<In>>,
    pub(crate) timeout_output: TimeoutOutput<Out>,
    #[cfg(any(feature = "logs", feature = "metrics", test))]
    pub(crate) telemetry: crate::utils::TelemetryHelper,
}

impl<In, Out, S: Clone> Clone for Timeout<In, Out, S> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            inner: self.inner.clone(),
        }
    }
}

impl<In, Out> Timeout<In, Out, ()> {
    /// Creates a [`TimeoutLayer`] used to configure the timeout resilience middleware.
    ///
    /// The instance returned by this call is a builder and cannot be used to build a
    /// service until the required properties are set: `timeout_output` and `timeout`.
    /// The `name` identifies the timeout strategy in telemetry, while `options`
    /// provides configuration shared across multiple resilience middleware.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use std::time::Duration;
    /// # use layered::{Execute, Stack};
    /// # use tick::Clock;
    /// # use seatbelt::ResilienceContext;
    /// use seatbelt::timeout::Timeout;
    ///
    /// # fn example(context: ResilienceContext<String, String>) {
    /// let timeout_layer = Timeout::layer("my_timeout", &context)
    ///     .timeout_output(|args| format!("timed out after {}ms", args.timeout().as_millis()))
    ///     .timeout(Duration::from_secs(30));
    /// # }
    /// ```
    ///
    /// For comprehensive examples, see the [timeout module] documentation.
    ///
    /// [timeout module]: crate::timeout
    pub fn layer(name: impl Into<Cow<'static, str>>, context: &ResilienceContext<In, Out>) -> TimeoutLayer<In, Out, NotSet, NotSet> {
        TimeoutLayer::new(name.into(), context)
    }
}

// IMPORTANT: The `layered::Service` impl below and the `tower_service::Service` impl further
// down in this file contain logic-equivalent orchestration code. Any change to the `execute`
// body MUST be mirrored in the `call` body, and vice versa. See crate-level AGENTS.md.
impl<In, Out, S> Service<In> for Timeout<In, Out, S>
where
    In: Send,
    S: Service<In, Out = Out>,
{
    type Out = Out;

    #[cfg_attr(test, mutants::skip)] // causes test timeouts
    async fn execute(&self, input: In) -> Self::Out {
        if !self.shared.enable_if.call(&input) {
            return self.inner.execute(input).await;
        }

        let timeout = self.shared.get_timeout(&input);

        match self.inner.execute(input).timeout(&self.shared.clock, timeout).await {
            Ok(output) => output,
            Err(_error) => self.shared.handle_timeout_error(timeout),
        }
    }
}

/// Future returned by [`Timeout`] when used as a tower [`Service`](tower_service::Service).
#[cfg(any(feature = "tower-service", test))]
pub struct TimeoutFuture<Out> {
    inner: Pin<Box<dyn Future<Output = Out> + Send>>,
}

#[cfg(any(feature = "tower-service", test))]
impl<Out> Debug for TimeoutFuture<Out> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimeoutFuture").finish_non_exhaustive()
    }
}

#[cfg(any(feature = "tower-service", test))]
impl<Out> Future for TimeoutFuture<Out> {
    type Output = Out;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

// IMPORTANT: The `tower_service::Service` impl below and the `layered::Service` impl above
// contain logic-equivalent orchestration code. Any change to the `call` body MUST be mirrored
// in the `execute` body, and vice versa. See crate-level AGENTS.md.
#[cfg(any(feature = "tower-service", test))]
impl<Req, Res, Err, S> tower_service::Service<Req> for Timeout<Req, Result<Res, Err>, S>
where
    Err: Send + 'static,
    Req: Send + 'static,
    Res: Send + 'static,
    S: tower_service::Service<Req, Response = Res, Error = Err> + Send + Sync + 'static,
    S::Future: Send + 'static,
{
    type Response = Res;
    type Error = Err;
    type Future = TimeoutFuture<Result<Res, Err>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    #[cfg_attr(test, mutants::skip)] // causes test timeout
    fn call(&mut self, req: Req) -> Self::Future {
        if !self.shared.enable_if.call(&req) {
            let future = self.inner.call(req);
            return TimeoutFuture { inner: Box::pin(future) };
        }

        let timeout = self.shared.get_timeout(&req);
        let shared = Arc::clone(&self.shared);
        let future = self.inner.call(req);

        TimeoutFuture {
            inner: Box::pin(async move {
                match future.timeout(&shared.clock, timeout).await {
                    Ok(result) => result,
                    Err(_error) => shared.handle_timeout_error(timeout),
                }
            }),
        }
    }
}

impl<In, Out> TimeoutShared<In, Out> {
    fn get_timeout(&self, input: &In) -> Duration {
        self.timeout_override
            .as_ref()
            .and_then(|provider| {
                provider.call(
                    input,
                    TimeoutOverrideArgs {
                        default_timeout: self.timeout,
                    },
                )
            })
            .unwrap_or(self.timeout)
    }

    fn handle_timeout_error(&self, timeout: Duration) -> Out {
        #[cfg(any(feature = "metrics", test))]
        if self.telemetry.metrics_enabled() {
            use crate::utils::{EVENT_NAME, PIPELINE_NAME, STRATEGY_NAME};

            self.telemetry.report_metrics(&[
                opentelemetry::KeyValue::new(PIPELINE_NAME, self.telemetry.pipeline_name.clone()),
                opentelemetry::KeyValue::new(STRATEGY_NAME, self.telemetry.strategy_name.clone()),
                opentelemetry::KeyValue::new(EVENT_NAME, super::telemetry::TIMEOUT_EVENT_NAME),
            ]);
        }

        let output = self.timeout_output.call(TimeoutOutputArgs { timeout });

        #[cfg(any(feature = "logs", test))]
        if self.telemetry.logs_enabled {
            tracing::event!(
                name: "seatbelt.timeout",
                tracing::Level::WARN,
                pipeline.name = %self.telemetry.pipeline_name,
                strategy.name = %self.telemetry.strategy_name,
                timeout.ms = timeout.as_millis(),
            );
        }

        if let Some(on_timeout) = &self.on_timeout {
            on_timeout.call(&output, OnTimeoutArgs { timeout });
        }

        output
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(not(miri))] // tokio runtime does not support Miri.
#[cfg(test)]
mod tests {
    use std::future::poll_fn;

    use layered::{Execute, Stack};
    use tick::ClockControl;

    use super::*;
    use crate::testing::FailReadyService;
    use layered::Layer;

    #[tokio::test]
    async fn timeout_emits_log() {
        use tracing_subscriber::util::SubscriberInitExt;

        use crate::testing::LogCapture;

        let log_capture = LogCapture::new();
        let _guard = log_capture.subscriber().set_default();

        let clock = ClockControl::default()
            .auto_advance(Duration::from_millis(200))
            .auto_advance_limit(Duration::from_millis(500))
            .to_clock();
        let context = ResilienceContext::new(clock.clone()).use_logs().name("log_test_pipeline");

        let stack = (
            Timeout::layer("log_test_timeout", &context)
                .timeout_output(|_| "timed out".to_string())
                .timeout(Duration::from_millis(100)),
            Execute::new(move |input| {
                let clock = clock.clone();
                async move {
                    clock.delay(Duration::from_secs(1)).await;
                    input
                }
            }),
        );

        let service = stack.into_service();
        let _ = service.execute("test".to_string()).await;

        log_capture.assert_contains("seatbelt::timeout");
        log_capture.assert_contains("log_test_pipeline");
        log_capture.assert_contains("log_test_timeout");
        log_capture.assert_contains("timeout.ms=100");
    }

    #[tokio::test]
    async fn timeout_emits_metrics() {
        use opentelemetry::KeyValue;

        use crate::testing::MetricTester;
        use crate::utils::{EVENT_NAME, PIPELINE_NAME, STRATEGY_NAME};

        let metrics = MetricTester::new();
        let clock = ClockControl::default()
            .auto_advance(Duration::from_millis(200))
            .auto_advance_limit(Duration::from_millis(500))
            .to_clock();
        let context = ResilienceContext::new(clock.clone())
            .use_metrics(metrics.meter_provider())
            .name("metrics_pipeline");

        let stack = (
            Timeout::layer("metrics_timeout", &context)
                .timeout_output(|_| "timed out".to_string())
                .timeout(Duration::from_millis(100)),
            Execute::new(move |input| {
                let clock = clock.clone();
                async move {
                    clock.delay(Duration::from_secs(1)).await;
                    input
                }
            }),
        );

        let service = stack.into_service();
        let _ = service.execute("test".to_string()).await;

        metrics.assert_attributes(
            &[
                KeyValue::new(PIPELINE_NAME, "metrics_pipeline"),
                KeyValue::new(STRATEGY_NAME, "metrics_timeout"),
                KeyValue::new(EVENT_NAME, "timeout"),
            ],
            Some(3),
        );
    }

    #[test]
    fn timeout_future_debug_contains_struct_name() {
        let future = TimeoutFuture::<String> {
            inner: Box::pin(async { "test".to_string() }),
        };
        let debug_output = format!("{future:?}");

        assert!(debug_output.contains("TimeoutFuture"));
    }

    #[tokio::test]
    async fn poll_ready_propagates_inner_error() {
        let context = crate::ResilienceContext::<String, Result<String, String>>::new(tick::Clock::new_frozen()).name("test");
        let layer = Timeout::layer("test_timeout", &context)
            .timeout_error(|_| "timed out".to_string())
            .timeout(Duration::from_millis(100));

        let mut service = layer.layer(FailReadyService);

        poll_fn(|cx| tower_service::Service::poll_ready(&mut service, cx))
            .await
            .unwrap_err();
    }
}
