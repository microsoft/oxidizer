// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use layered::Service;
use tick::Clock;

use crate::retry::{
    CloneArgs, CloneInput, DelayBackoff, OnRetry, OnRetryArgs, RecoveryArgs, RestoreInput, RestoreInputArgs, ShouldRecover,
};
use crate::utils::EnableIf;
use crate::{NotSet, RecoveryInfo, RecoveryKind, retry::Attempt};

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
pub struct Retry<In, Out, S> {
    pub(super) shared: Arc<RetryShared<In, Out>>,
    pub(super) inner: S,
}

/// Shared configuration for [`Retry`] middleware.
///
/// This struct is wrapped in an `Arc` to enable cheap cloning of the service.
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

impl<In, Out, S: std::fmt::Debug> std::fmt::Debug for Retry<In, Out, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Retry")
            .field("max_attempts", &self.shared.max_attempts)
            .field("inner", &self.inner)
            .finish_non_exhaustive()
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
            let out = self.inner.execute(attempt_input).await;

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
        match self.clone_input.call(
            &mut input,
            CloneArgs {
                attempt,
                previous_recovery,
            },
        ) {
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
        expect(unused_variables, reason = "unused when logs feature not used")
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

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(not(miri))] // Oxidizer runtime does not support Miri.
#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    use layered::Execute;
    use opentelemetry::KeyValue;
    use tick::ClockControl;

    use super::*;
    use crate::retry::{Backoff, RetryLayer};
    use crate::testing::MetricTester;
    use crate::{ResilienceContext, Set};
    use layered::Layer;

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
    async fn retry_disabled_no_inner_calls() {
        let clock = Clock::new_frozen();
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = std::sync::Arc::clone(&counter);

        let service = create_ready_retry_layer(&clock, RecoveryInfo::retry())
            .clone_input_with(move |input, _args| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                Some(input.clone())
            })
            .disable()
            .layer(Execute::new(move |v: String| async move { v }));

        let result = service.execute("test".to_string()).await;

        assert_eq!(result, "test");
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn uncloneable_recovery_called() {
        let clock = Clock::new_frozen();
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = std::sync::Arc::clone(&counter);

        let service = create_ready_retry_layer(&clock, RecoveryInfo::retry())
            .clone_input_with(move |_input, _args| None)
            .recovery_with(move |_input, _args| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                RecoveryInfo::retry()
            })
            .layer(Execute::new(move |v: String| async move { v }));

        let result = service.execute("test".to_string()).await;

        assert_eq!(result, "test");
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn no_recovery_ensure_no_additional_retries() {
        let clock = Clock::new_frozen();
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = std::sync::Arc::clone(&counter);

        let service = create_ready_retry_layer(&clock, RecoveryInfo::retry())
            .clone_input_with(move |input, _args| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                Some(input.clone())
            })
            .recovery_with(move |_input, _args| RecoveryInfo::never())
            .layer(Execute::new(move |v: String| async move { v }));

        let result = service.execute("test".to_string()).await;

        assert_eq!(result, "test");
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_recovery_ensure_retries_exhausted() {
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = std::sync::Arc::clone(&counter);

        let service = create_ready_retry_layer(&clock, RecoveryInfo::retry())
            .clone_input_with(move |input, _args| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                Some(input.clone())
            })
            .recovery_with(move |_input, _args| RecoveryInfo::retry())
            .max_retry_attempts(4)
            .layer(Execute::new(move |v: String| async move { v }));

        let result = service.execute("test".to_string()).await;

        assert_eq!(result, "test");
        assert_eq!(counter.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn retry_recovery_ensure_correct_delays() {
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let delays = Arc::new(Mutex::new(vec![]));
        let delays_clone = Arc::clone(&delays);

        let service = create_ready_retry_layer(&clock, RecoveryInfo::retry())
            .clone_input_with(move |input, _args| Some(input.clone()))
            .use_jitter(false)
            .backoff(Backoff::Linear)
            .recovery_with(move |_input, _args| RecoveryInfo::retry())
            .max_retry_attempts(4)
            .on_retry(move |_output, args| {
                delays_clone.lock().unwrap().push(args.retry_delay());
            })
            .layer(Execute::new(move |v: String| async move { v }));

        let _result = service.execute("test".to_string()).await;

        assert_eq!(
            delays.lock().unwrap().to_vec(),
            vec![
                Duration::from_millis(10),
                Duration::from_millis(20),
                Duration::from_millis(30),
                Duration::from_millis(40),
            ]
        );
    }

    #[tokio::test]
    async fn retry_recovery_ensure_correct_attempts() {
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let attempts = Arc::new(Mutex::new(vec![]));
        let attempts_clone = Arc::clone(&attempts);

        let attempts_for_clone = Arc::new(Mutex::new(vec![]));
        let attempts_for_clone_clone = Arc::clone(&attempts_for_clone);

        let service = create_ready_retry_layer(&clock, RecoveryInfo::retry())
            .clone_input_with(move |input, args| {
                attempts_for_clone_clone.lock().unwrap().push(args.attempt());
                Some(input.clone())
            })
            .recovery_with(move |_input, _args| RecoveryInfo::retry())
            .max_retry_attempts(4)
            .on_retry(move |_output, args| {
                attempts_clone.lock().unwrap().push(args.attempt());
            })
            .layer(Execute::new(move |v: String| async move { v }));

        let _result = service.execute("test".to_string()).await;

        assert_eq!(
            attempts_for_clone.lock().unwrap().to_vec(),
            vec![
                Attempt::new(0, false),
                Attempt::new(1, false),
                Attempt::new(2, false),
                Attempt::new(3, false),
                Attempt::new(4, true),
            ]
        );

        assert_eq!(
            attempts.lock().unwrap().to_vec(),
            vec![
                Attempt::new(0, false),
                Attempt::new(1, false),
                Attempt::new(2, false),
                Attempt::new(3, false),
            ]
        );
    }

    #[tokio::test]
    async fn restore_input_integration_test() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_clone = std::sync::Arc::clone(&call_count);
        let restore_count = Arc::new(AtomicU32::new(0));
        let restore_count_clone = std::sync::Arc::clone(&restore_count);

        // Create a service that fails on first attempt but can restore input
        let service = create_ready_retry_layer(&clock, RecoveryInfo::retry())
            .clone_input_with(|_input, _args| None) // Don't clone - force restore path
            .restore_input(move |output: &mut String, _args| {
                restore_count_clone.fetch_add(1, Ordering::SeqCst);
                output.contains("error:").then(|| {
                    let input = output.replace("error:", "");
                    *output = "restored".to_string();
                    input
                })
            })
            .recovery_with(|output, _args| {
                if output.contains("error:") {
                    RecoveryInfo::retry()
                } else {
                    RecoveryInfo::never()
                }
            })
            .max_retry_attempts(2)
            .layer(Execute::new(move |input: String| {
                let count = call_count_clone.fetch_add(1, Ordering::SeqCst);
                async move {
                    if count == 0 {
                        // First call fails with input stored in error
                        format!("error:{input}")
                    } else {
                        // Subsequent calls succeed
                        format!("success:{input}")
                    }
                }
            }));

        let result = service.execute("test_input".to_string()).await;

        // Verify the restore path was used and retry succeeded
        assert_eq!(result, "success:test_input");
        assert_eq!(call_count.load(Ordering::SeqCst), 2); // Original + 1 retry
        assert_eq!(restore_count.load(Ordering::SeqCst), 1); // Restore called once
    }

    #[tokio::test]
    async fn outage_handling_disabled_no_retries() {
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_clone = Arc::clone(&call_count);

        // Create a service that returns outage (handle_unavailable disabled by default)
        let service = create_ready_retry_layer(&clock, RecoveryInfo::retry())
            .clone_input_with(move |input, _args| {
                call_count_clone.fetch_add(1, Ordering::SeqCst);
                Some(input.clone())
            })
            .recovery_with(|_output, _args| RecoveryInfo::unavailable())
            .layer(Execute::new(move |v: String| async move { v }));

        let result = service.execute("test".to_string()).await;

        // Should not retry when outage handling is disabled
        assert_eq!(result, "test");
        assert_eq!(call_count.load(Ordering::SeqCst), 1); // Only original call, no retries
    }

    #[tokio::test]
    async fn outage_handling_enabled_with_retries() {
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_clone = std::sync::Arc::clone(&call_count);

        // Create a service that returns outage initially, then succeeds
        let service = create_ready_retry_layer(&clock, RecoveryInfo::retry())
            .clone_input_with(move |input, _args| {
                call_count_clone.fetch_add(1, Ordering::SeqCst);
                Some(input.clone())
            })
            .recovery_with(|_output, args| {
                // First attempt returns outage, subsequent attempts succeed
                if args.attempt().index() == 0 {
                    RecoveryInfo::unavailable()
                } else {
                    RecoveryInfo::never()
                }
            })
            .handle_unavailable(true) // Enable outage handling
            .max_retry_attempts(2)
            .layer(Execute::new(move |input: String| async move { format!("processed_{input}") }));

        let result = service.execute("test".to_string()).await;

        // Should retry when outage handling is enabled
        assert_eq!(result, "processed_test");
        assert_eq!(call_count.load(Ordering::SeqCst), 2); // Original + 1 retry
    }

    #[tokio::test]
    async fn outage_handling_with_recovery_hint() {
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let delays = Arc::new(Mutex::new(vec![]));
        let delays_clone = Arc::clone(&delays);

        // Create a service that returns outage with recovery hint
        let service = create_ready_retry_layer(&clock, RecoveryInfo::retry())
            .clone_input_with(move |input, _args| Some(input.clone()))
            .recovery_with(|_output, args| {
                if args.attempt().index() == 0 {
                    RecoveryInfo::unavailable().delay(Duration::from_secs(10)) // 10 second recovery hint
                } else {
                    RecoveryInfo::never()
                }
            })
            .handle_unavailable(true)
            .max_retry_attempts(1)
            .on_retry(move |_output, args| {
                delays_clone.lock().unwrap().push(args.retry_delay());
            })
            .layer(Execute::new(move |v: String| async move { v }));

        let _result = service.execute("test".to_string()).await;

        // Should use the recovery hint as the delay
        assert_eq!(delays.lock().unwrap().to_vec(), vec![Duration::from_secs(10)]);
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

        use crate::testing::LogCapture;

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

    fn create_ready_retry_layer(clock: &Clock, recover: RecoveryInfo) -> RetryLayer<String, String, Set, Set> {
        let context = ResilienceContext::new(clock.clone()).name("test_pipeline");
        create_ready_retry_layer_core(recover, &context)
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
}
