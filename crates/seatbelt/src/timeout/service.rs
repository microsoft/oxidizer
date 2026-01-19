// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::time::Duration;

use futures::future::Either;
use layered::Service;
use opentelemetry::StringValue;
use tick::{Clock, FutureExt};

use crate::telemetry::{EVENT_NAME, PIPELINE_NAME, STRATEGY_NAME};
use crate::timeout::telemetry::TIMEOUT_EVENT_NAME;
use crate::timeout::{OnTimeout, OnTimeoutArgs, TimeoutLayer, TimeoutOutput, TimeoutOutputArgs, TimeoutOverride, TimeoutOverrideArgs};
use crate::{Context, EnableIf, NotSet};

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
#[expect(clippy::struct_field_names, reason = "fields are named for clarity")]
pub struct Timeout<In, Out, S> {
    pub(super) inner: S,
    pub(super) clock: Clock,
    pub(super) timeout: Duration,
    pub(super) enable_if: EnableIf<In>,
    pub(super) on_timeout: Option<OnTimeout<Out>>,
    pub(super) timeout_override: Option<TimeoutOverride<In>>,
    pub(super) timeout_output: TimeoutOutput<Out>,
    pub(super) name: StringValue,
    pub(super) pipeline_name: StringValue,
    pub(super) event_reporter: opentelemetry::metrics::Counter<u64>,
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
    /// # use seatbelt::Context;
    /// use seatbelt::timeout::Timeout;
    ///
    /// # fn example(context: Context<String, String>) {
    /// let timeout_layer = Timeout::layer("my_timeout", &context)
    ///     .timeout_output(|args| format!("timed out after {}ms", args.timeout().as_millis()))
    ///     .timeout(Duration::from_secs(30));
    /// # }
    /// ```
    ///
    /// For comprehensive examples, see the [timeout module] documentation.
    ///
    /// [timeout module]: crate::timeout
    pub fn layer(name: impl Into<Cow<'static, str>>, context: &Context<In, Out>) -> TimeoutLayer<In, Out, NotSet, NotSet> {
        TimeoutLayer::new(name.into().into(), context)
    }
}

impl<In, Out, S> Service<In> for Timeout<In, Out, S>
where
    In: Send,
    S: Service<In, Out = Out>,
{
    type Out = Out;

    #[cfg_attr(test, mutants::skip)] // causes test timeouts
    fn execute(&self, input: In) -> impl Future<Output = Self::Out> + Send {
        if !self.enable_if.call(&input) {
            return Either::Left(self.inner.execute(input));
        }

        let timeout = self
            .timeout_override
            .as_ref()
            .and_then(|provider| {
                provider.call(
                    &input,
                    TimeoutOverrideArgs {
                        default_timeout: self.timeout,
                    },
                )
            })
            .unwrap_or(self.timeout);

        Either::Right(async move {
            match self.inner.execute(input).timeout(&self.clock, timeout).await {
                Ok(output) => output,
                Err(_error) => {
                    self.event_reporter.add(
                        1,
                        &[
                            opentelemetry::KeyValue::new(PIPELINE_NAME, self.pipeline_name.clone()),
                            opentelemetry::KeyValue::new(STRATEGY_NAME, self.name.clone()),
                            opentelemetry::KeyValue::new(EVENT_NAME, TIMEOUT_EVENT_NAME),
                        ],
                    );

                    let output = self.timeout_output.call(TimeoutOutputArgs { timeout });

                    tracing::event!(
                        name: "seatbelt.timeout",
                        tracing::Level::WARN,
                        pipeline.name = self.pipeline_name.as_str(),
                        strategy.name = self.name.as_str(),
                        timeout.ms = timeout.as_millis(),
                    );

                    if let Some(on_timeout) = &self.on_timeout {
                        on_timeout.call(&output, OnTimeoutArgs { timeout });
                    }

                    output
                }
            }
        })
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
        let context = Context::new(clock);

        let stack = (
            Timeout::layer("test_timeout", &context)
                .timeout_output(|args| format!("timed out after {}ms", args.timeout().as_millis()))
                .timeout(Duration::from_secs(5)),
            Execute::new(|input: String| async move { input }),
        );

        let service = stack.build();

        let output = service.execute("test input".to_string()).await;

        assert_eq!(output, "test input".to_string());
    }

    #[tokio::test]
    async fn timeout() {
        let clock = ClockControl::default()
            .auto_advance(Duration::from_millis(200))
            .auto_advance_limit(Duration::from_millis(500))
            .to_clock();
        let context = Context::new(clock.clone());
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

        let service = stack.build();

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
            Timeout::layer("test_timeout", &Context::new(clock.clone()))
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

        let service = stack.build();

        assert_eq!(service.execute("test input".to_string()).await, "timed out after 150ms");
        assert_eq!(service.execute("ignore".to_string()).await, "timed out after 200ms");
    }

    #[tokio::test]
    async fn no_timeout_if_disabled() {
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let stack = (
            Timeout::layer("test_timeout", &Context::new(&clock))
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

        let service = stack.build();
        let output = service.execute("test input".to_string()).await;

        assert_eq!(output, "test input");
    }
}
