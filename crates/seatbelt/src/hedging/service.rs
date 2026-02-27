// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
#[cfg(any(feature = "tower-service", test))]
use std::pin::Pin;
use std::sync::Arc;
#[cfg(any(feature = "tower-service", test))]
use std::task::{Context, Poll};

use futures_util::future::{Either, select};
use futures_util::stream::{FuturesUnordered, StreamExt};
use layered::Service;
use tick::Clock;

use super::args::{OnHedgeArgs, RecoveryArgs, TryCloneArgs};
use super::callbacks::*;
use super::mode::HedgingMode;
use crate::Attempt;
use crate::utils::EnableIf;
use crate::{NotSet, RecoveryKind};

/// Applies hedging logic to service execution for tail-latency reduction.
///
/// `Hedging` wraps an inner [`Service`] and launches speculative concurrent requests
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
    pub(crate) max_hedged_attempts: u32,
    pub(crate) hedging_mode: HedgingMode,
    pub(crate) try_clone: TryClone<In>,
    pub(crate) should_recover: ShouldRecover<Out>,
    pub(crate) on_hedge: Option<OnHedge>,
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
// down in this file contain logic-equivalent orchestration code. Any change to the `execute`
// body MUST be mirrored in the `call` body, and vice versa. See crate-level AGENTS.md.
impl<In, Out: Send, S> Service<In> for Hedging<In, Out, S>
where
    In: Send,
    S: Service<In, Out = Out>,
{
    type Out = Out;

    #[cfg_attr(test, mutants::skip)]
    async fn execute(&self, mut input: In) -> Self::Out {
        if !self.shared.enable_if.call(&input) {
            return self.inner.execute(input).await;
        }

        let max_hedged = self.shared.max_hedged_attempts;
        let total_attempts = max_hedged.saturating_add(1);

        // If no hedges configured, pass through directly.
        if max_hedged == 0 {
            return self.inner.execute(input).await;
        }

        let mut futs = FuturesUnordered::new();

        // Clone input for first attempt; if clone fails, fall back to direct execution.
        let args = TryCloneArgs {
            attempt: Attempt::new(0, total_attempts == 1),
        };
        match self.shared.try_clone.call(&mut input, args) {
            Some(cloned) => futs.push(self.inner.execute(cloned)),
            None => return self.inner.execute(input).await,
        }

        if self.shared.hedging_mode.is_immediate() {
            // Launch all hedges immediately.
            for i in 1..total_attempts {
                let args = TryCloneArgs {
                    attempt: Attempt::new(i, i == total_attempts.saturating_sub(1)),
                };
                self.shared.invoke_on_hedge(i.saturating_sub(1));
                self.shared.emit_telemetry(i);
                if let Some(cloned) = self.shared.try_clone.call(&mut input, args) {
                    futs.push(self.inner.execute(cloned));
                }
            }

            return self.shared.drain_for_first_acceptable(&mut futs).await;
        }

        // Delay / Dynamic mode: race between results and hedge timers.
        self.shared
            .run_delay_loop(&mut futs, &mut input, max_hedged, |cloned| self.inner.execute(cloned))
            .await
    }
}

impl<In, Out> HedgingShared<In, Out> {
    fn is_recoverable(&self, out: &Out) -> bool {
        let recovery = self.should_recover.call(out, RecoveryArgs { clock: &self.clock });

        matches!(recovery.kind(), RecoveryKind::Retry | RecoveryKind::Unavailable)
    }

    async fn drain_for_first_acceptable<F>(&self, futs: &mut FuturesUnordered<F>) -> Out
    where
        F: Future<Output = Out>,
    {
        let mut last_result = None;
        while let Some(out) = futs.next().await {
            if !self.is_recoverable(&out) {
                return out;
            }
            last_result = Some(out);
        }
        last_result.expect("at least one attempt was launched")
    }

    async fn run_delay_loop<F>(
        &self,
        futs: &mut FuturesUnordered<F>,
        input: &mut In,
        max_hedged: u32,
        mut launch: impl FnMut(In) -> F,
    ) -> Out
    where
        F: Future<Output = Out>,
    {
        let mut hedges_launched = 0u32;
        let mut last_result: Option<Out> = None;

        loop {
            if hedges_launched < max_hedged {
                let delay = self.hedging_mode.delay_for(hedges_launched);

                let outcome = {
                    let next = std::pin::pin!(futs.next());
                    let delay_fut = std::pin::pin!(self.clock.delay(delay));
                    match select(next, delay_fut).await {
                        Either::Left((opt, _)) => SelectOutcome::Result(opt),
                        Either::Right(((), _)) => SelectOutcome::DelayExpired,
                    }
                };

                match outcome {
                    SelectOutcome::Result(Some(out)) => {
                        if !self.is_recoverable(&out) {
                            return out;
                        }
                        last_result = Some(out);
                        // Result was recoverable — launch a hedge immediately
                        // instead of waiting for the delay timer again.
                        self.launch_hedge(futs, input, &mut hedges_launched, max_hedged, &mut launch);
                    }
                    SelectOutcome::Result(None) => {
                        return last_result.expect("at least one attempt was launched");
                    }
                    SelectOutcome::DelayExpired => {
                        self.launch_hedge(futs, input, &mut hedges_launched, max_hedged, &mut launch);
                    }
                }
            } else {
                match futs.next().await {
                    Some(out) => {
                        if !self.is_recoverable(&out) {
                            return out;
                        }
                        last_result = Some(out);
                    }
                    None => {
                        return last_result.expect("at least one attempt was launched");
                    }
                }
            }
        }
    }

    fn launch_hedge<F>(
        &self,
        futs: &FuturesUnordered<F>,
        input: &mut In,
        hedges_launched: &mut u32,
        max_hedged: u32,
        launch: &mut impl FnMut(In) -> F,
    ) {
        *hedges_launched = hedges_launched.saturating_add(1);
        let args = TryCloneArgs {
            attempt: Attempt::new(*hedges_launched, *hedges_launched >= max_hedged),
        };

        self.invoke_on_hedge(hedges_launched.saturating_sub(1));
        self.emit_telemetry(*hedges_launched);

        if let Some(cloned) = self.try_clone.call(input, args) {
            futs.push(launch(cloned));
        }
    }

    fn invoke_on_hedge(&self, hedge_index: u32) {
        if let Some(on_hedge) = &self.on_hedge {
            on_hedge.call(OnHedgeArgs { hedge_index });
        }
    }

    #[cfg_attr(
        not(any(feature = "logs", test)),
        expect(unused_variables, clippy::unused_self, reason = "unused when logs feature not used")
    )]
    fn emit_telemetry(&self, attempt_index: u32) {
        #[cfg(any(feature = "logs", test))]
        if self.telemetry.logs_enabled {
            tracing::event!(
                name: "seatbelt.hedge",
                tracing::Level::INFO,
                pipeline.name = %self.telemetry.pipeline_name,
                strategy.name = %self.telemetry.strategy_name,
                resilience.attempt.index = attempt_index,
            );
        }

        #[cfg(any(feature = "metrics", test))]
        if self.telemetry.metrics_enabled() {
            use super::telemetry::{ATTEMPT_INDEX, HEDGE_EVENT};
            use crate::utils::{EVENT_NAME, PIPELINE_NAME, STRATEGY_NAME};

            self.telemetry.report_metrics(&[
                opentelemetry::KeyValue::new(PIPELINE_NAME, self.telemetry.pipeline_name.clone()),
                opentelemetry::KeyValue::new(STRATEGY_NAME, self.telemetry.strategy_name.clone()),
                opentelemetry::KeyValue::new(EVENT_NAME, HEDGE_EVENT),
                opentelemetry::KeyValue::new(ATTEMPT_INDEX, i64::from(attempt_index)),
            ]);
        }
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

// IMPORTANT: The `tower_service::Service` impl below and the `layered::Service` impl above
// contain logic-equivalent orchestration code. Any change to the `call` body MUST be mirrored
// in the `execute` body, and vice versa. See crate-level AGENTS.md.
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
                let mut input = req;
                let max_hedged = shared.max_hedged_attempts;
                let total_attempts = max_hedged.saturating_add(1);

                if max_hedged == 0 {
                    let mut svc = inner;
                    return svc.call(input).await;
                }

                let mut futs = FuturesUnordered::new();

                let args = TryCloneArgs {
                    attempt: Attempt::new(0, total_attempts == 1),
                };
                if let Some(cloned) = shared.try_clone.call(&mut input, args) {
                    let mut svc = inner.clone();
                    futs.push(svc.call(cloned));
                } else {
                    let mut svc = inner;
                    return svc.call(input).await;
                }

                if shared.hedging_mode.is_immediate() {
                    for i in 1..total_attempts {
                        let args = TryCloneArgs {
                            attempt: Attempt::new(i, i == total_attempts.saturating_sub(1)),
                        };
                        shared.invoke_on_hedge(i.saturating_sub(1));
                        shared.emit_telemetry(i);
                        if let Some(cloned) = shared.try_clone.call(&mut input, args) {
                            let mut svc = inner.clone();
                            futs.push(svc.call(cloned));
                        }
                    }
                    return shared.drain_for_first_acceptable(&mut futs).await;
                }

                shared
                    .run_delay_loop(&mut futs, &mut input, max_hedged, |cloned| {
                        let mut svc = inner.clone();
                        svc.call(cloned)
                    })
                    .await
            }),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(not(miri))]
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
    fn layer_ensure_defaults() {
        let context = ResilienceContext::<String, String>::new(Clock::new_frozen()).name("test_pipeline");
        let layer: HedgingLayer<String, String, NotSet, NotSet> = Hedging::layer("test_hedging", &context);
        let layer = layer.recovery_with(|_, _| RecoveryInfo::never()).try_clone();

        let hedging = layer.layer(Execute::new(|v: String| async move { v }));

        assert_eq!(hedging.shared.telemetry.pipeline_name.to_string(), "test_pipeline");
        assert_eq!(hedging.shared.telemetry.strategy_name.to_string(), "test_hedging");
        assert_eq!(hedging.shared.max_hedged_attempts, 1);
        assert!(!hedging.shared.hedging_mode.is_immediate());
        assert!(hedging.shared.on_hedge.is_none());
        assert!(hedging.shared.enable_if.call(&"str".to_string()));
    }

    #[tokio::test]
    async fn hedging_emits_metrics() {
        let tester = MetricTester::new();
        let context = ResilienceContext::<String, String>::new(ClockControl::default().auto_advance_timers(true).to_clock())
            .name("test_pipeline")
            .use_metrics(tester.meter_provider());

        let service = Hedging::layer("test_hedging", &context)
            .try_clone()
            .recovery_with(|_input, _args| RecoveryInfo::retry())
            .max_hedged_attempts(1)
            .hedging_mode(HedgingMode::immediate())
            .layer(Execute::new(|v: String| async move { v }));

        let _result = service.execute("test".to_string()).await;

        tester.assert_attributes(
            &[
                KeyValue::new("resilience.pipeline.name", "test_pipeline"),
                KeyValue::new("resilience.strategy.name", "test_hedging"),
                KeyValue::new("resilience.event.name", "hedge"),
                KeyValue::new("resilience.attempt.index", 1i64),
            ],
            Some(4),
        );
    }

    #[tokio::test]
    async fn hedging_emits_log() {
        use tracing_subscriber::util::SubscriberInitExt;

        use crate::testing::LogCapture;

        let log_capture = LogCapture::new();
        let _guard = log_capture.subscriber().set_default();

        let clock = ClockControl::default().auto_advance_timers(true).to_clock();
        let context = ResilienceContext::<String, String>::new(clock).name("log_test_pipeline").use_logs();

        let service = Hedging::layer("log_test_hedging", &context)
            .try_clone()
            .recovery_with(|_, _| RecoveryInfo::retry())
            .max_hedged_attempts(1)
            .hedging_mode(HedgingMode::immediate())
            .layer(Execute::new(|v: String| async move { v }));

        let _ = service.execute("test".to_string()).await;

        log_capture.assert_contains("seatbelt::hedging");
        log_capture.assert_contains("log_test_pipeline");
        log_capture.assert_contains("log_test_hedging");
        log_capture.assert_contains("resilience.attempt.index");
    }

    #[test]
    fn hedging_future_debug_contains_struct_name() {
        let future = HedgingFuture::<String> {
            inner: Box::pin(async { "test".to_string() }),
        };
        let debug_output = format!("{future:?}");
        assert!(debug_output.contains("HedgingFuture"));
    }

    #[tokio::test]
    async fn poll_ready_propagates_inner_error() {
        let context = ResilienceContext::<String, Result<String, String>>::new(Clock::new_frozen()).name("test");
        let layer = Hedging::layer("test_hedging", &context)
            .recovery_with(|_, _| RecoveryInfo::never())
            .try_clone();

        let mut service = layer.layer(FailReadyService);

        poll_fn(|cx| tower_service::Service::poll_ready(&mut service, cx))
            .await
            .unwrap_err();
    }
}
