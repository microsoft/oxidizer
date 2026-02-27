// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::ops::ControlFlow;
#[cfg(any(feature = "tower-service", test))]
use std::pin::Pin;
use std::sync::Arc;
#[cfg(any(feature = "tower-service", test))]
use std::task::{Context, Poll};
use std::time::Duration;

use layered::Service;
use tick::Clock;

use super::*;
use crate::utils::EnableIf;
use crate::{NotSet, RecoveryInfo, RecoveryKind};

/// Applies retry logic to service execution for transient error handling.
///
/// `Retry` wraps an inner [`Service`] and automatically retries failed operations
/// based on configurable recovery classification, backoff strategies, and delay generation.
/// This middleware is designed to be used across services, applications, and libraries
/// to handle transient failures gracefully.
///
/// This middleware requires input cloning capabilities and recovery classification to determine
/// retry eligibility.
///
/// Retry is configured by calling [`Retry::layer`] and using the
/// builder methods on the returned [`RetryLayer`][crate::retry::RetryLayer] instance.
///
/// For comprehensive examples and usage patterns, see the [retry module][crate::retry] documentation.
#[derive(Debug)]
pub struct Retry<In, Out, S> {
    pub(super) shared: Arc<RetryShared<In, Out>>,
    pub(super) inner: S,
}

/// Shared configuration for [`Retry`] middleware.
///
/// This struct is wrapped in an `Arc` to enable cheap cloning of the service.
#[derive(Debug)]
pub(crate) struct RetryShared<In, Out> {
    pub(crate) clock: Clock,
    pub(crate) max_attempts: u32,
    pub(crate) backoff: DelayBackoff,
    pub(crate) clone_input: CloneInput<In>,
    pub(crate) should_recover: ShouldRecover<Out>,
    pub(crate) on_retry: Option<OnRetry<Out>>,
    pub(crate) enable_if: EnableIf<In>,
    #[cfg(any(feature = "logs", feature = "metrics", test))]
    pub(crate) telemetry: crate::utils::TelemetryHelper,
    pub(crate) restore_input: Option<RestoreInput<In, Out>>,
    pub(crate) handle_unavailable: bool,
}

impl<In, Out, S: Clone> Clone for Retry<In, Out, S> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            inner: self.inner.clone(),
        }
    }
}

impl<In, Out> Retry<In, Out, ()> {
    /// Creates a new retry layer with the specified name and options.
    ///
    /// Returns a [`RetryLayer`][crate::retry::RetryLayer] that must be configured with required parameters
    /// before it can be used to build a retry service.
    pub fn layer(
        name: impl Into<std::borrow::Cow<'static, str>>,
        context: &crate::ResilienceContext<In, Out>,
    ) -> crate::retry::RetryLayer<In, Out, NotSet, NotSet> {
        crate::retry::RetryLayer::new(name.into(), context)
    }
}

// IMPORTANT: The `layered::Service` impl below and the `tower_service::Service` impl further
// down in this file contain logic-equivalent orchestration code. Any change to the `execute`
// body MUST be mirrored in the `call` body, and vice versa. See crate-level AGENTS.md.
impl<In, Out: Send, S> Service<In> for Retry<In, Out, S>
where
    In: Send,
    S: Service<In, Out = Out>,
{
    type Out = Out;

    #[cfg_attr(test, mutants::skip)] // Mutating enable_if check causes infinite loops
    async fn execute(&self, mut input: In) -> Self::Out {
        if !self.shared.enable_if.call(&input) {
            return self.inner.execute(input).await;
        }

        let mut attempt = Attempt::first(self.shared.max_attempts);
        let mut delays = self.shared.backoff.delays();
        let mut previous_recovery = None;

        loop {
            let (original_input, attempt_input) = self.shared.clone_input(input, attempt, previous_recovery.clone());

            // execute inner service
            let out = self.inner.execute(attempt_input).await;

            // evaluate whether to retry
            match self.shared.evaluate_attempt(original_input, out, attempt, &mut delays) {
                ControlFlow::Continue(state) => {
                    self.shared.clock.delay(state.delay).await;
                    input = state.input;
                    attempt = state.attempt;
                    previous_recovery = Some(state.recovery);
                }
                ControlFlow::Break(out) => return out,
            }
        }
    }
}

impl<In, Out> RetryShared<In, Out> {
    fn clone_input(&self, mut input: In, attempt: Attempt, previous_recovery: Option<RecoveryInfo>) -> (Option<In>, In) {
        let args = CloneArgs {
            attempt,
            previous_recovery,
        };

        match self.clone_input.call(&mut input, args) {
            Some(cloned) => (Some(input), cloned),
            None => (None, input),
        }
    }

    fn evaluate_attempt(
        &self,
        mut original_input: Option<In>,
        mut out: Out,
        attempt: Attempt,
        delays: &mut impl Iterator<Item = Duration>,
    ) -> ControlFlow<Out, ContinueRetry<In>> {
        let recovery = self.should_recover.call(
            &out,
            RecoveryArgs {
                attempt,
                clock: &self.clock,
            },
        );

        if !self.is_recoverable(&recovery) {
            return ControlFlow::Break(out);
        }

        let Some(next_attempt) = attempt.increment(self.max_attempts) else {
            self.emit_telemetry(attempt, Duration::ZERO);
            return ControlFlow::Break(out);
        };

        let retry_delay = compute_retry_delay(&recovery, delays);

        self.emit_telemetry(attempt, retry_delay);

        if let Some(input) = self.try_restore_input(original_input.as_ref(), &mut out, attempt, &recovery) {
            original_input = Some(input);
        }

        match original_input {
            Some(input) => {
                self.invoke_on_retry(&out, attempt, retry_delay, &recovery);
                ControlFlow::Continue(ContinueRetry {
                    input,
                    attempt: next_attempt,
                    recovery,
                    delay: retry_delay,
                })
            }
            None => ControlFlow::Break(out),
        }
    }

    #[cfg_attr(test, mutants::skip)] // causes test timeout
    fn is_recoverable(&self, recovery: &RecoveryInfo) -> bool {
        match recovery.kind() {
            RecoveryKind::Unavailable => self.handle_unavailable,
            RecoveryKind::Retry => true,
            RecoveryKind::Never | RecoveryKind::Unknown | _ => false,
        }
    }

    fn try_restore_input(&self, original_input: Option<&In>, out: &mut Out, attempt: Attempt, recovery: &RecoveryInfo) -> Option<In> {
        if original_input.is_some() {
            return None;
        }

        match &self.restore_input {
            Some(restore) => restore.call(
                out,
                RestoreInputArgs {
                    attempt,
                    recovery: recovery.clone(),
                },
            ),
            None => None,
        }
    }

    fn invoke_on_retry(&self, out: &Out, attempt: Attempt, retry_delay: Duration, recovery: &RecoveryInfo) {
        if let Some(on_retry) = &self.on_retry {
            on_retry.call(
                out,
                OnRetryArgs {
                    attempt,
                    retry_delay,
                    recovery: recovery.clone(),
                },
            );
        }
    }

    #[cfg_attr(
        not(any(feature = "logs", test)),
        expect(unused_variables, clippy::unused_self, reason = "unused when logs feature not used")
    )]
    fn emit_telemetry(&self, attempt: Attempt, retry_delay: Duration) {
        #[cfg(any(feature = "logs", test))]
        if self.telemetry.logs_enabled {
            tracing::event!(
                name: "seatbelt.retry",
                tracing::Level::WARN,
                pipeline.name = %self.telemetry.pipeline_name,
                strategy.name = %self.telemetry.strategy_name,
                resilience.attempt.index = attempt.index(),
                resilience.attempt.is_last = attempt.is_last(),
                resilience.retry.delay = retry_delay.as_secs_f32(),
            );
        }

        #[cfg(any(feature = "metrics", test))]
        if self.telemetry.metrics_enabled() {
            use super::telemetry::{ATTEMPT_INDEX, ATTEMPT_NUMBER_IS_LAST, RETRY_EVENT};
            use crate::utils::{EVENT_NAME, PIPELINE_NAME, STRATEGY_NAME};

            self.telemetry.report_metrics(&[
                opentelemetry::KeyValue::new(PIPELINE_NAME, self.telemetry.pipeline_name.clone()),
                opentelemetry::KeyValue::new(STRATEGY_NAME, self.telemetry.strategy_name.clone()),
                opentelemetry::KeyValue::new(EVENT_NAME, RETRY_EVENT),
                opentelemetry::KeyValue::new(ATTEMPT_INDEX, i64::from(attempt.index())),
                opentelemetry::KeyValue::new(ATTEMPT_NUMBER_IS_LAST, attempt.is_last()),
            ]);
        }
    }
}

fn compute_retry_delay(recovery: &RecoveryInfo, delays: &mut impl Iterator<Item = Duration>) -> Duration {
    let backoff_delay = delays.next().unwrap_or(Duration::ZERO);
    recovery.get_delay().unwrap_or(backoff_delay)
}

/// State passed between retry attempts when continuing the retry loop.
struct ContinueRetry<In> {
    input: In,
    attempt: Attempt,
    recovery: RecoveryInfo,
    delay: Duration,
}

/// Future returned by [`Retry`] when used as a tower [`Service`](tower_service::Service).
#[cfg(any(feature = "tower-service", test))]
pub struct RetryFuture<Out> {
    inner: Pin<Box<dyn Future<Output = Out> + Send>>,
}

#[cfg(any(feature = "tower-service", test))]
impl<Out> Debug for RetryFuture<Out> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetryFuture").finish_non_exhaustive()
    }
}

#[cfg(any(feature = "tower-service", test))]
impl<Out> Future for RetryFuture<Out> {
    type Output = Out;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

// IMPORTANT: The `tower_service::Service` impl below and the `layered::Service` impl above
// contain logic-equivalent orchestration code. Any change to the `call` body MUST be mirrored
// in the `execute` body, and vice versa. See crate-level AGENTS.md.
#[cfg(any(feature = "tower-service", test))]
impl<Req, Res, Err, S> tower_service::Service<Req> for Retry<Req, Result<Res, Err>, S>
where
    Err: Send + 'static,
    Req: Send + 'static,
    Res: Send + 'static,
    S: tower_service::Service<Req, Response = Res, Error = Err> + Clone + Send + Sync + 'static,
    S::Future: Send + 'static,
{
    type Response = Res;
    type Error = Err;
    type Future = RetryFuture<Result<Res, Err>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    #[cfg_attr(test, mutants::skip)] // causes test timeout
    fn call(&mut self, req: Req) -> Self::Future {
        if !self.shared.enable_if.call(&req) {
            let future = self.inner.call(req);
            return RetryFuture { inner: Box::pin(future) };
        }

        let shared = Arc::clone(&self.shared);
        let inner = self.inner.clone();

        RetryFuture {
            inner: Box::pin(async move {
                let mut input = req;
                let mut inner = inner;
                let mut attempt = Attempt::first(shared.max_attempts);
                let mut delays = shared.backoff.delays();
                let mut previous_recovery = None;

                loop {
                    let (original_input, attempt_input) = shared.clone_input(input, attempt, previous_recovery.clone());

                    let out = inner.call(attempt_input).await;

                    // evaluate whether to retry
                    match shared.evaluate_attempt(original_input, out, attempt, &mut delays) {
                        ControlFlow::Continue(state) => {
                            shared.clock.delay(state.delay).await;
                            input = state.input;
                            attempt = state.attempt;
                            previous_recovery = Some(state.recovery);
                        }
                        ControlFlow::Break(out) => return out,
                    }
                }
            }),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(not(miri))] // Oxidizer runtime does not support Miri.
#[cfg(test)]
mod tests {
    use std::future::poll_fn;

    use layered::Execute;
    use opentelemetry::KeyValue;
    use tick::ClockControl;

    use super::*;
    use crate::testing::FailReadyService;
    use crate::{ResilienceContext, Set};
    use layered::Layer;
    use testing_aids::MetricTester;

    #[test]
    fn layer_ensure_defaults() {
        let context = ResilienceContext::<String, String>::new(Clock::new_frozen()).name("test_pipeline");
        let layer: RetryLayer<String, String, NotSet, NotSet> = Retry::layer("test_retry", &context);
        let layer = layer.recovery_with(|_, _| RecoveryInfo::never()).clone_input();

        let retry = layer.layer(Execute::new(|v: String| async move { v }));

        assert_eq!(retry.shared.telemetry.pipeline_name.to_string(), "test_pipeline");
        assert_eq!(retry.shared.telemetry.strategy_name.to_string(), "test_retry");
        assert_eq!(retry.shared.max_attempts, 4);
        assert_eq!(retry.shared.backoff.0.base_delay, Duration::from_millis(10));
        assert_eq!(retry.shared.backoff.0.backoff_type, Backoff::Exponential);
        assert!(retry.shared.backoff.0.use_jitter);
        assert!(retry.shared.on_retry.is_none());
        assert!(retry.shared.enable_if.call(&"str".to_string()));
    }

    #[tokio::test]
    async fn retries_exhausted_ensure_telemetry_reported() {
        let tester = MetricTester::new();
        let context = ResilienceContext::<String, String>::new(ClockControl::default().auto_advance_timers(true).to_clock())
            .name("test_pipeline")
            .use_metrics(tester.meter_provider());

        let service = create_ready_retry_layer_core(RecoveryInfo::retry(), &context)
            .clone_input_with(move |input, _args| Some(input.clone()))
            .max_retry_attempts(2)
            .recovery_with(move |_input, _args| RecoveryInfo::retry())
            .layer(Execute::new(move |v: String| async move { v }));

        let _result = service.execute("test".to_string()).await;

        tester.assert_attributes(
            &[
                KeyValue::new("resilience.attempt.index", 0),
                KeyValue::new("resilience.attempt.index", 1),
                KeyValue::new("resilience.attempt.is_last", false),
                KeyValue::new("resilience.attempt.is_last", true),
                KeyValue::new("resilience.pipeline.name", "test_pipeline"),
                KeyValue::new("resilience.strategy.name", "test_retry"),
                KeyValue::new("resilience.event.name", "retry"),
            ],
            Some(15),
        );
    }

    #[tokio::test]
    async fn retry_emits_log() {
        use tracing_subscriber::util::SubscriberInitExt;

        use testing_aids::LogCapture;

        let log_capture = LogCapture::new();
        let _guard = log_capture.subscriber().set_default();

        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let context = ResilienceContext::<String, String>::new(clock).name("log_test_pipeline").use_logs();

        let service = Retry::layer("log_test_retry", &context)
            .clone_input()
            .recovery_with(|_, _| RecoveryInfo::retry())
            .max_retry_attempts(2)
            .layer(Execute::new(|v: String| async move { v }));

        let _ = service.execute("test".to_string()).await;

        log_capture.assert_contains("seatbelt::retry");
        log_capture.assert_contains("log_test_pipeline");
        log_capture.assert_contains("log_test_retry");
        log_capture.assert_contains("resilience.attempt.index");
        log_capture.assert_contains("resilience.retry.delay");
    }

    fn create_ready_retry_layer_core(
        recover: RecoveryInfo,
        context: &ResilienceContext<String, String>,
    ) -> RetryLayer<String, String, Set, Set> {
        Retry::layer("test_retry", context)
            .recovery_with(move |_, _| recover.clone())
            .clone_input()
            .max_delay(Duration::from_secs(9999)) // protect against infinite backoff
    }

    #[test]
    fn retry_future_debug_contains_struct_name() {
        let future = RetryFuture::<String> {
            inner: Box::pin(async { "test".to_string() }),
        };
        let debug_output = format!("{future:?}");

        assert!(debug_output.contains("RetryFuture"));
    }

    #[tokio::test]
    async fn poll_ready_propagates_inner_error() {
        let context = ResilienceContext::<String, Result<String, String>>::new(Clock::new_frozen()).name("test");
        let layer = Retry::layer("test_retry", &context)
            .recovery_with(|_, _| RecoveryInfo::never())
            .clone_input();

        let mut service = layer.layer(FailReadyService);

        poll_fn(|cx| tower_service::Service::poll_ready(&mut service, cx))
            .await
            .unwrap_err();
    }
}
