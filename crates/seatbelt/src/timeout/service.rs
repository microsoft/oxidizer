// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::sync::Arc;
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
    use layered::{Execute, Stack};
    use tick::ClockControl;

    use super::*;

    #[tokio::test]
    async fn no_timeout() {
        let clock = Clock::new_frozen();
        let context = ResilienceContext::new(clock);

        let stack = (
            Timeout::layer("test_timeout", &context)
                .timeout_output(|args| format!("timed out after {}ms", args.timeout().as_millis()))
                .timeout(Duration::from_secs(5)),
            Execute::new(|input: String| async move { input }),
        );

        let service = stack.into_service();

        let output = service.execute("test input".to_string()).await;

        assert_eq!(output, "test input".to_string());
    }

    #[tokio::test]
    async fn timeout() {
        let clock = ClockControl::default()
            .auto_advance(Duration::from_millis(200))
            .auto_advance_limit(Duration::from_millis(500))
            .to_clock();
        let context = ResilienceContext::new(clock.clone());
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_clone = std::sync::Arc::clone(&called);

        let stack = (
            Timeout::layer("test_timeout", &context)
                .timeout_output(|args| format!("timed out after {}ms", args.timeout().as_millis()))
                .timeout(Duration::from_millis(200))
                .on_timeout(move |out, args| {
                    assert_eq!("timed out after 200ms", out.as_str());
                    assert_eq!(200, args.timeout().as_millis());
                    called.store(true, std::sync::atomic::Ordering::SeqCst);
                }),
            Execute::new(move |input| {
                let clock = clock.clone();
                async move {
                    clock.delay(Duration::from_secs(1)).await;
                    input
                }
            }),
        );

        let service = stack.into_service();

        let output = service.execute("test input".to_string()).await;

        assert_eq!(output, "timed out after 200ms");
        assert!(called_clone.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn timeout_override_ensure_respected() {
        let clock = ClockControl::default()
            .auto_advance(Duration::from_millis(200))
            .auto_advance_limit(Duration::from_millis(5000))
            .to_clock();

        let stack = (
            Timeout::layer("test_timeout", &ResilienceContext::new(clock.clone()))
                .timeout_output(|args| format!("timed out after {}ms", args.timeout().as_millis()))
                .timeout(Duration::from_millis(200))
                .timeout_override(|input, _args| {
                    if input == "ignore" {
                        return None;
                    }

                    Some(Duration::from_millis(150))
                }),
            Execute::new(move |input| {
                let clock = clock.clone();
                async move {
                    clock.delay(Duration::from_secs(10)).await;
                    input
                }
            }),
        );

        let service = stack.into_service();

        assert_eq!(service.execute("test input".to_string()).await, "timed out after 150ms");
        assert_eq!(service.execute("ignore".to_string()).await, "timed out after 200ms");
    }

    #[tokio::test]
    async fn no_timeout_if_disabled() {
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let stack = (
            Timeout::layer("test_timeout", &ResilienceContext::new(&clock))
                .timeout_output(|_args| "timed out".to_string())
                .timeout(Duration::from_millis(200))
                .disable(),
            Execute::new({
                let clock = clock.clone();
                move |input| {
                    let clock = clock.clone();
                    async move {
                        clock.delay(Duration::from_secs(1)).await;
                        input
                    }
                }
            }),
        );

        let service = stack.into_service();
        let output = service.execute("test input".to_string()).await;

        assert_eq!(output, "test input");
    }

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
}
