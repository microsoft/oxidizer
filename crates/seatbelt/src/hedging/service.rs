// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
#[cfg(any(feature = "tower-service", test))]
use std::pin::Pin;
use std::sync::Arc;
#[cfg(any(feature = "tower-service", test))]
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::future::{Either, select};
use futures_util::stream::{FuturesUnordered, StreamExt};
use layered::Service;
use recoverable::RecoveryKind;
use tick::Clock;

use super::args::{CloneArgs, HedgingDelayArgs, OnExecuteArgs, RecoveryArgs};
use super::callbacks::*;
use crate::Attempt;
use crate::typestates::NotSet;
use crate::utils::EnableIf;

/// Applies hedging logic to service execution for tail-latency reduction.
///
/// `Hedging` wraps an inner [`Service`] and launches additional concurrent requests
/// to reduce the impact of slow responses. The first acceptable result is returned
/// and remaining in-flight requests are cancelled.
///
/// Hedging is configured by calling [`Hedging::layer`] and using the
/// builder methods on the returned [`HedgingLayer`][crate::hedging::HedgingLayer] instance.
///
/// For comprehensive examples, see the [hedging module][crate::hedging] documentation.
#[derive(Debug)]
pub struct Hedging<In, Out, S> {
    pub(super) shared: Arc<HedgingShared<In, Out>>,
    pub(super) inner: S,
}

/// Shared configuration for [`Hedging`] middleware.
///
/// This struct is wrapped in an `Arc` to enable cheap cloning of the service.
#[derive(Debug)]
pub(crate) struct HedgingShared<In, Out> {
    pub(crate) clock: Clock,
    pub(crate) max_hedged_attempts: u8,
    pub(crate) delay_fn: DelayFn<In>,
    pub(crate) clone_input: CloneInput<In>,
    pub(crate) should_recover: ShouldRecover<Out>,
    pub(crate) on_execute: Option<OnExecute<In>>,
    pub(crate) handle_unavailable: bool,
    pub(crate) enable_if: EnableIf<In>,
    #[cfg(any(feature = "logs", feature = "metrics", test))]
    pub(crate) telemetry: crate::utils::TelemetryHelper,
}

impl<In, Out, S: Clone> Clone for Hedging<In, Out, S> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            inner: self.inner.clone(),
        }
    }
}

impl<In, Out> Hedging<In, Out, ()> {
    /// Creates a new hedging layer with the specified name and options.
    ///
    /// Returns a [`HedgingLayer`][crate::hedging::HedgingLayer] that must be configured with
    /// required parameters before it can be used to build a hedging service.
    pub fn layer(
        name: impl Into<std::borrow::Cow<'static, str>>,
        context: &crate::ResilienceContext<In, Out>,
    ) -> crate::hedging::HedgingLayer<In, Out, NotSet, NotSet> {
        crate::hedging::HedgingLayer::new(name.into(), context)
    }
}

/// Internal result from the select race between stream next and delay.
enum SelectOutcome<Out> {
    /// A result was produced by one of the in-flight futures.
    Result(Option<Out>),
    /// The delay expired without any result completing.
    DelayExpired,
}

// IMPORTANT: The `layered::Service` impl below and the `tower_service::Service` impl further
// down in this file both delegate to `HedgingShared::run_hedging` for the core orchestration.
// Only the "passthrough" and "launch" mechanics differ between the two.
impl<In, Out: Send, S> Service<In> for Hedging<In, Out, S>
where
    In: Send,
    S: Service<In, Out = Out>,
{
    type Out = Out;

    #[cfg_attr(test, mutants::skip)]
    async fn execute(&self, input: In) -> Self::Out {
        if !self.shared.enable_if.call(&input) {
            return self.inner.execute(input).await;
        }

        self.shared.run_hedging(input, |cloned| self.inner.execute(cloned)).await
    }
}

use super::telemetry::TelemetryGuard;

/// Wraps an inner future with a [`TelemetryGuard`] so that abandoned (dropped)
/// futures still emit telemetry.
///
/// When the future completes the guard is returned alongside the output so the
/// caller can classify the result. When the future is dropped before completing
/// the guard's [`Drop`] impl reports the attempt as `"abandoned"`.
async fn guarded<F: Future>(inner: F, guard: TelemetryGuard) -> (F::Output, TelemetryGuard) {
    let out = inner.await;
    (out, guard)
}

impl<In, Out> HedgingShared<In, Out> {
    /// Core hedging orchestration shared by both layered and tower service impls.
    ///
    /// Takes ownership of `input` and always returns an `Out`. When hedging is
    /// bypassed (no hedging attempts configured or input clone failed), the `launch` closure
    /// is called directly with the original input.
    async fn run_hedging<F>(&self, mut input: In, mut launch: impl FnMut(In) -> F) -> Out
    where
        F: Future<Output = Out>,
    {
        let total_attempts = u32::from(self.max_hedged_attempts).saturating_add(1);
        let attempt = Attempt::new(0, total_attempts == 1);
        let args = CloneArgs { attempt };

        let Some(mut first_cloned) = self.clone_input.call(&mut input, args) else {
            self.invoke_on_execute(&mut input, attempt, Duration::ZERO);
            return launch(input).await;
        };

        self.invoke_on_execute(&mut first_cloned, attempt, Duration::ZERO);
        let guard = self.create_guard(attempt, Duration::ZERO);
        let mut futs = FuturesUnordered::new();
        futs.push(guarded(launch(first_cloned), guard));

        self.run_delay_loop(&mut futs, &mut input, attempt, total_attempts, |cloned, g| {
            guarded(launch(cloned), g)
        })
        .await
    }

    /// Classifies a result as recoverable or non-recoverable.
    ///
    /// Returns `Some(kind)` for recoverable results (the hedging loop continues),
    /// or `None` for non-recoverable results (accept and return immediately).
    ///
    /// [`RecoveryKind::Unknown`] is treated as non-recoverable: if the recovery
    /// callback cannot classify a result, hedging accepts it rather than risking
    /// waiting for attempts that may also fail classification.
    fn recovery_kind(&self, out: &Out, attempt: Attempt) -> Option<RecoveryKind> {
        let recovery = self.should_recover.call(
            out,
            RecoveryArgs {
                clock: &self.clock,
                attempt,
            },
        );

        match recovery.kind() {
            RecoveryKind::Unavailable if self.handle_unavailable => Some(RecoveryKind::Unavailable),
            RecoveryKind::Retry => Some(RecoveryKind::Retry),
            // Wildcard required because RecoveryKind is #[non_exhaustive].
            // New variants default to non-recoverable; update when adding variants.
            RecoveryKind::Never | RecoveryKind::Unknown | _ => None,
        }
    }

    async fn run_delay_loop<G>(
        &self,
        futs: &mut FuturesUnordered<G>,
        input: &mut In,
        mut attempt: Attempt,
        total_attempts: u32,
        mut guarded_launch: impl FnMut(In, TelemetryGuard) -> G,
    ) -> Out
    where
        G: Future<Output = (Out, TelemetryGuard)>,
    {
        let mut last_result: Option<Out> = None;

        loop {
            if let Some(next_attempt) = attempt.increment(total_attempts) {
                let delay = self.delay_fn.call(input, HedgingDelayArgs { attempt: next_attempt });

                let outcome = {
                    let next = std::pin::pin!(futs.next());
                    let delay_fut = std::pin::pin!(self.clock.delay(delay));
                    match select(next, delay_fut).await {
                        Either::Left((opt, _)) => SelectOutcome::Result(opt),
                        Either::Right(((), _)) => SelectOutcome::DelayExpired,
                    }
                };

                match outcome {
                    SelectOutcome::Result(Some((out, mut guard))) => {
                        let Some(recovery_kind) = self.recovery_kind(&out, guard.attempt) else {
                            guard.disarm();
                            return out;
                        };
                        guard.set_recovery_kind(recovery_kind);
                        drop(guard);
                        last_result = Some(out);
                        // Result was recoverable — launch a hedging attempt immediately
                        // instead of waiting for the delay timer again.
                        self.launch_hedging_attempt(futs, input, next_attempt, Duration::ZERO, &mut guarded_launch);
                        attempt = next_attempt;
                    }
                    SelectOutcome::Result(None) => {
                        return last_result.expect("at least one attempt was launched");
                    }
                    SelectOutcome::DelayExpired => {
                        self.launch_hedging_attempt(futs, input, next_attempt, delay, &mut guarded_launch);
                        attempt = next_attempt;
                    }
                }
            } else {
                // All hedging attempts launched — drain remaining futures, preserving
                // any recoverable result collected during the delay loop.
                while let Some((out, mut guard)) = futs.next().await {
                    let Some(recovery_kind) = self.recovery_kind(&out, guard.attempt) else {
                        guard.disarm();
                        return out;
                    };
                    guard.set_recovery_kind(recovery_kind);
                    drop(guard);
                    last_result = Some(out);
                }
                return last_result.expect("at least one attempt was launched");
            }
        }
    }

    #[cfg_attr(test, mutants::skip)] // causes test timeouts
    fn launch_hedging_attempt<G>(
        &self,
        futs: &FuturesUnordered<G>,
        input: &mut In,
        attempt: Attempt,
        hedging_delay: Duration,
        guarded_launch: &mut impl FnMut(In, TelemetryGuard) -> G,
    ) {
        let args = CloneArgs { attempt };

        if let Some(mut cloned) = self.clone_input.call(input, args) {
            self.invoke_on_execute(&mut cloned, attempt, hedging_delay);
            let guard = self.create_guard(attempt, hedging_delay);
            futs.push(guarded_launch(cloned, guard));
        }
    }

    #[cfg_attr(test, mutants::skip)] // causes test timeouts
    fn invoke_on_execute(&self, input: &mut In, attempt: Attempt, delay: Duration) {
        if let Some(on_execute) = &self.on_execute {
            on_execute.call(input, OnExecuteArgs { attempt, delay });
        }
    }

    fn create_guard(&self, attempt: Attempt, hedging_delay: Duration) -> TelemetryGuard {
        TelemetryGuard::new(
            attempt,
            hedging_delay,
            #[cfg(any(feature = "logs", feature = "metrics", test))]
            self.telemetry.clone(),
        )
    }
}

/// Future returned by [`Hedging`] when used as a tower [`Service`](tower_service::Service).
#[cfg(any(feature = "tower-service", test))]
pub struct HedgingFuture<Out> {
    inner: Pin<Box<dyn Future<Output = Out> + Send>>,
}

#[cfg(any(feature = "tower-service", test))]
impl<Out> Debug for HedgingFuture<Out> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HedgingFuture").finish_non_exhaustive()
    }
}

#[cfg(any(feature = "tower-service", test))]
impl<Out> Future for HedgingFuture<Out> {
    type Output = Out;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

// The `tower_service::Service` impl below and the `layered::Service` impl above both
// delegate to `HedgingShared::run_hedging` for the core orchestration.
#[cfg(any(feature = "tower-service", test))]
impl<Req, Res, Err, S> tower_service::Service<Req> for Hedging<Req, Result<Res, Err>, S>
where
    Err: Send + 'static,
    Req: Send + 'static,
    Res: Send + 'static,
    S: tower_service::Service<Req, Response = Res, Error = Err> + Clone + Send + Sync + 'static,
    S::Future: Send + 'static,
{
    type Response = Res;
    type Error = Err;
    type Future = HedgingFuture<Result<Res, Err>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    #[cfg_attr(test, mutants::skip)]
    fn call(&mut self, req: Req) -> Self::Future {
        if !self.shared.enable_if.call(&req) {
            let future = self.inner.call(req);
            return HedgingFuture { inner: Box::pin(future) };
        }

        let shared = Arc::clone(&self.shared);
        let inner = self.inner.clone();

        HedgingFuture {
            inner: Box::pin(async move {
                shared
                    .run_hedging(req, |cloned| {
                        let mut svc = inner.clone();
                        svc.call(cloned)
                    })
                    .await
            }),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::future::poll_fn;

    use layered::{Execute, Layer};
    use opentelemetry::KeyValue;
    use tick::ClockControl;

    use super::*;
    use crate::hedging::HedgingLayer;
    use crate::testing::{FailReadyService, MetricTester};
    use crate::{RecoveryInfo, ResilienceContext};

    #[test]
    #[cfg_attr(miri, ignore)]
    fn layer_ensure_defaults() {
        let context = ResilienceContext::<String, String>::new(Clock::new_frozen()).name("test_pipeline");
        let layer: HedgingLayer<String, String, NotSet, NotSet> = Hedging::layer("test_hedging", &context);
        let layer = layer.recovery_with(|_, _| RecoveryInfo::never()).clone_input();

        let hedging = layer.layer(Execute::new(|v: String| async move { v }));

        assert_eq!(hedging.shared.telemetry.pipeline_name.to_string(), "test_pipeline");
        assert_eq!(hedging.shared.telemetry.strategy_name.to_string(), "test_hedging");
        assert_eq!(hedging.shared.max_hedged_attempts, 1);
        assert!(!hedging.shared.handle_unavailable);
        assert!(hedging.shared.on_execute.is_none());
        assert!(hedging.shared.enable_if.call(&"str".to_string()));
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn hedging_emits_metrics() {
        let tester = MetricTester::new();
        let context = ResilienceContext::<String, String>::new(ClockControl::default().auto_advance_timers(true).to_clock())
            .name("test_pipeline")
            .use_metrics(tester.meter_provider());

        let service = Hedging::layer("test_hedging", &context)
            .clone_input()
            .recovery_with(|_input, _args| RecoveryInfo::retry())
            .max_hedged_attempts(1)
            .hedging_delay(Duration::ZERO)
            .layer(Execute::new(|v: String| async move { v }));

        let _result = service.execute("test".to_string()).await;

        tester.assert_attributes(
            &[
                KeyValue::new("resilience.pipeline.name", "test_pipeline"),
                KeyValue::new("resilience.strategy.name", "test_hedging"),
                KeyValue::new("resilience.event.name", "hedging"),
                KeyValue::new("resilience.attempt.index", 1i64),
                KeyValue::new("resilience.attempt.is_last", true),
                KeyValue::new("resilience.attempt.recovery.kind", "retry"),
            ],
            // Two events: attempt 0 (recoverable) + attempt 1 (recoverable), 6 attrs each
            Some(12),
        );
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn hedging_emits_log() {
        use tracing_subscriber::util::SubscriberInitExt;

        use crate::testing::LogCapture;

        let log_capture = LogCapture::new();
        let _guard = log_capture.subscriber().set_default();

        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let context = ResilienceContext::<String, String>::new(clock).name("log_test_pipeline").use_logs();

        let service = Hedging::layer("log_test_hedging", &context)
            .clone_input()
            .recovery_with(|_, _| RecoveryInfo::retry())
            .max_hedged_attempts(1)
            .hedging_delay(Duration::ZERO)
            .layer(Execute::new(|v: String| async move { v }));

        let _ = service.execute("test".to_string()).await;

        log_capture.assert_contains("seatbelt::hedging");
        log_capture.assert_contains("log_test_pipeline");
        log_capture.assert_contains("log_test_hedging");
        log_capture.assert_contains("resilience.attempt.index");
        log_capture.assert_contains("resilience.attempt.is_last");
        log_capture.assert_contains("resilience.attempt.recovery.kind");
        log_capture.assert_contains("resilience.hedging.delay");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn hedging_future_debug_contains_struct_name() {
        let future = HedgingFuture::<String> {
            inner: Box::pin(async { "test".to_string() }),
        };
        let debug_output = format!("{future:?}");
        assert!(debug_output.contains("HedgingFuture"));
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn poll_ready_propagates_inner_error() {
        let context = ResilienceContext::<String, Result<String, String>>::new(Clock::new_frozen()).name("test");
        let layer = Hedging::layer("test_hedging", &context)
            .recovery_with(|_, _| RecoveryInfo::never())
            .clone_input();

        let mut service = layer.layer(FailReadyService);

        poll_fn(|cx| tower_service::Service::poll_ready(&mut service, cx))
            .await
            .unwrap_err();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn execute_future_size_is_bounded() {
        let context = ResilienceContext::<String, String>::new(Clock::new_frozen());
        let service = Hedging::layer("bench", &context)
            .clone_input()
            .recovery_with(|_, _| RecoveryInfo::never())
            .layer(Execute::new(|v: String| async move { v }));

        let future = service.execute("test".to_string());
        let size = std::mem::size_of_val(&future);

        // Print the size so CI logs capture it for tracking over time.
        println!("hedging execute future size: {size} bytes");

        // Guard against accidental future bloat. Update this threshold
        // deliberately if a change legitimately increases the future size.
        let max_bytes = 512;
        assert!(
            size <= max_bytes,
            "hedging execute future is {size} bytes, which exceeds the {max_bytes}-byte threshold"
        );
    }

    /// Verifies that:
    /// - recoverable attempts emit telemetry with the actual recovery kind
    /// - abandoned (dropped) attempts emit telemetry with recovery kind "abandoned"
    /// - successful (non-recoverable) attempts do NOT emit telemetry
    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn telemetry_reports_recoverable_and_abandoned_not_successful() {
        use std::sync::atomic::AtomicU32;
        use tokio::sync::Notify;

        let tester = MetricTester::new();
        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let context = ResilienceContext::<String, Result<String, String>>::new(clock)
            .name("test_pipeline")
            .use_metrics(tester.meter_provider());

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = Arc::clone(&counter);

        // Attempt 0 blocks forever (will be abandoned when a success arrives).
        let block_forever = Arc::new(Notify::new());
        let block_clone = Arc::clone(&block_forever);

        let service = Hedging::layer("test_hedging", &context)
            .clone_input()
            .recovery_with(|result: &Result<String, String>, _| match result {
                Ok(_) => RecoveryInfo::never(),
                Err(_) => RecoveryInfo::retry(),
            })
            .max_hedged_attempts(2)
            .hedging_delay(Duration::ZERO)
            .layer(Execute::new(move |_v: String| {
                let idx = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let block = Arc::clone(&block_clone);
                async move {
                    match idx {
                        // Attempt 0: blocks forever → will be abandoned
                        0 => {
                            block.notified().await;
                            Ok::<_, String>("never_reached".into())
                        }
                        // Attempt 1: transient error → recoverable
                        1 => Err("transient".into()),
                        // Attempt 2: success → non-recoverable (accepted)
                        _ => Ok("success".into()),
                    }
                }
            }));

        let result = service.execute("input".to_string()).await;
        assert_eq!(result, Ok("success".to_string()));

        let attributes = tester.collect_attributes();

        // Expect exactly 2 metric events (6 attrs each = 12 total):
        //   1. Attempt 1: recoverable with recovery kind "retry"
        //   2. Attempt 0: abandoned with recovery kind "abandoned"
        // Attempt 2 (success) must NOT produce telemetry.
        assert_eq!(
            attributes.len(),
            12,
            "expected 12 attributes (2 events × 6 attrs), got {}: {attributes:?}",
            attributes.len()
        );

        // Verify the recoverable attempt is reported
        assert!(
            attributes.contains(&KeyValue::new("resilience.attempt.recovery.kind", "retry")),
            "expected 'retry' recovery kind in attributes: {attributes:?}"
        );

        // Verify the abandoned attempt is reported
        assert!(
            attributes.contains(&KeyValue::new("resilience.attempt.recovery.kind", "abandoned")),
            "expected 'abandoned' recovery kind in attributes: {attributes:?}"
        );

        // Verify that the successful attempt index (2) is NOT in the attributes,
        // confirming no telemetry was emitted for the accepted result.
        assert!(
            !attributes.contains(&KeyValue::new("resilience.attempt.index", 2i64)),
            "attempt index 2 (success) should not appear in telemetry: {attributes:?}"
        );
    }
}
