// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Frozen-clock middleware contracts, shared by layered and tower services.

#![cfg(all(
    feature = "retry",
    feature = "timeout",
    feature = "hedging",
    feature = "chaos-latency",
    feature = "tower-service"
))]

use std::future::{pending, poll_fn};
use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use futures::executor::block_on;
use layered::{Execute, Service, Stack};
use rstest::rstest;
use seatbelt::chaos::latency::Latency;
use seatbelt::hedging::{Hedging, HedgingConfig};
use seatbelt::retry::{Backoff, Retry};
use seatbelt::timeout::Timeout;
use seatbelt::{RecoveryInfo, ResilienceContext};
use tick::ClockControl;
use tower_service::Service as TowerService;

testing_aids::init_tracing!();

#[global_allocator]
#[cfg(not(miri))]
static ALLOCATOR: alloc_tracker::Allocator<std::alloc::System> = alloc_tracker::Allocator::system();

#[cfg(feature = "breaker")]
#[rstest]
#[case::layered(false)]
#[case::tower(true)]
fn gamma_survivor_breaker_counts_only_live_abandonment_and_bypasses_disabled_inputs(#[case] tower: bool) {
    use seatbelt::breaker::{AbandonedPolicy, Breaker};

    let clock = ClockControl::new().to_clock();
    let context = ResilienceContext::new(&clock);
    let opened = Arc::new(AtomicUsize::new(0));
    let captured = Arc::clone(&opened);
    let recovered = Arc::new(AtomicUsize::new(0));
    let recovery = Arc::clone(&recovered);
    let stack = (
        Breaker::layer("abandon", &context)
            .enable_if(|input: &u32| *input != 0)
            .recovery_with(move |_: &Result<u32, String>, _| {
                recovery.fetch_add(1, Ordering::Relaxed);
                RecoveryInfo::never()
            })
            .rejected_input(|_, _| Err("rejected".to_owned()))
            .abandoned_policy(AbandonedPolicy::as_failures())
            .min_throughput(1)
            .failure_threshold(0.1)
            .on_opened(move |output: Option<&Result<u32, String>>, _| {
                assert!(output.is_none());
                captured.fetch_add(1, Ordering::Relaxed);
            }),
        Execute::new(|input| async move {
            if input == 2 {
                pending::<()>().await;
            }
            Ok::<_, String>(input)
        }),
    );
    let mut service = stack.into_service();
    assert_eq!(block_on(execute(&mut service, 0, tower)), Ok(0));
    assert_eq!(recovered.load(Ordering::Relaxed), 0);
    assert_eq!(block_on(execute(&mut service, 1, tower)), Ok(1));
    assert_eq!(opened.load(Ordering::Relaxed), 0);
    assert_eq!(recovered.load(Ordering::Relaxed), 1);
    {
        let mut future = pin!(execute(&mut service, 2, tower));
        assert_eq!(future.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Pending);
    }
    assert_eq!(opened.load(Ordering::Relaxed), 1);
    assert_eq!(block_on(execute(&mut service, 3, tower)), Err("rejected".to_owned()));
    assert_eq!(recovered.load(Ordering::Relaxed), 1);
}

#[cfg(feature = "breaker")]
#[rstest]
#[case::layered(false)]
#[case::tower(true)]
fn gamma_survivor_breaker_unwinding_inner_does_not_record_abandonment_or_open_circuit(#[case] tower: bool) {
    use seatbelt::breaker::{AbandonedPolicy, Breaker};

    let clock = ClockControl::new().to_clock();
    let context = ResilienceContext::new(&clock);
    let opened = Arc::new(AtomicUsize::new(0));
    let captured = Arc::clone(&opened);
    let stack = (
        Breaker::layer("unwind", &context)
            .recovery_with(|_: &Result<u32, String>, _| RecoveryInfo::never())
            .rejected_input(|_, _| Err("rejected".to_owned()))
            .abandoned_policy(AbandonedPolicy::as_failures())
            .min_throughput(1)
            .on_opened(move |_, _| {
                captured.fetch_add(1, Ordering::Relaxed);
            }),
        Execute::new(|input| async move {
            assert_ne!(input, 1, "controlled inner failure");
            Ok::<_, String>(input)
        }),
    );
    let mut service = stack.into_service();
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| block_on(execute(&mut service, 1, tower)))).unwrap_err();
    assert_eq!(opened.load(Ordering::Relaxed), 0);
    assert_eq!(block_on(execute(&mut service, 0, tower)), Ok(0));
}

#[cfg(all(feature = "serde", feature = "breaker"))]
#[test]
fn gamma_survivor_abandoned_policy_wrong_threshold_type_is_a_deserialization_error() {
    serde_json::from_str::<seatbelt::breaker::AbandonedPolicy>(r#"{"rate_threshold":"not-a-number"}"#).unwrap_err();
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
fn gamma_survivor_zero_base_exponential_jitter_stays_zero_past_float_overflow(#[case] tower: bool) {
    let control = ClockControl::new().auto_advance_timers(true);
    let clock = control.to_clock();
    let started = clock.instant();
    let context = ResilienceContext::new(&clock);
    let calls = Arc::new(AtomicUsize::new(0));
    let captured = Arc::clone(&calls);
    let stack = (
        Retry::layer("zero_overflow", &context)
            .clone_input()
            .recovery_with(|_: &Result<u32, String>, _| RecoveryInfo::retry())
            .base_delay(Duration::ZERO)
            .backoff(Backoff::Exponential)
            .use_jitter(true)
            .max_retry_attempts(1100),
        Execute::new(move |input| {
            captured.fetch_add(1, Ordering::Relaxed);
            async move { Ok::<_, String>(input) }
        }),
    );
    assert_eq!(block_on(execute(&mut stack.into_service(), 7, tower)), Ok(7));
    assert_eq!(calls.load(Ordering::Relaxed), 1101);
    assert_eq!(clock.instant(), started);
}

async fn execute<S>(service: &mut S, input: u32, tower: bool) -> Result<u32, String>
where
    S: Service<u32, Out = Result<u32, String>> + TowerService<u32, Response = u32, Error = String>,
    S::Future: Send,
{
    if tower {
        poll_fn(|cx| service.poll_ready(cx)).await?;
        service.call(input).await
    } else {
        service.execute(input).await
    }
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
fn gamma_survivor_retry_previous_recovery_and_restore_only_without_clone(#[case] tower: bool) {
    let clock = ClockControl::new().auto_advance_timers(true).to_clock();
    let history = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&history);
    let restores = Arc::new(AtomicUsize::new(0));
    let restored = Arc::clone(&restores);
    let context = ResilienceContext::new(&clock);
    let stack = (
        Retry::layer("history", &context)
            .clone_input_with(move |input: &mut u32, args| {
                captured
                    .lock()
                    .unwrap()
                    .push((args.attempt().index(), args.previous_recovery().map(RecoveryInfo::get_delay)));
                Some(*input)
            })
            .restore_input(move |_: &mut Result<u32, String>, _| {
                restored.fetch_add(1, Ordering::Relaxed);
                Some(99)
            })
            .recovery_with(|_: &Result<u32, String>, args| {
                RecoveryInfo::retry().delay(Duration::from_millis(u64::from(args.attempt().index()) + 3))
            })
            .max_retry_attempts(2),
        Execute::new(|input| async move { Ok::<_, String>(input) }),
    );
    assert_eq!(block_on(execute(&mut stack.into_service(), 7, tower)), Ok(7));
    assert_eq!(restores.load(Ordering::Relaxed), 0);
    assert_eq!(
        *history.lock().unwrap(),
        [
            (0, None),
            (1, Some(Some(Duration::from_millis(3)))),
            (2, Some(Some(Duration::from_millis(4)))),
        ]
    );
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
fn gamma_survivor_retry_disabled_and_nonrecoverable_stop_without_extra_calls(#[case] tower: bool) {
    for (enabled, recovery, unavailable, expected_calls) in [
        (false, RecoveryInfo::retry(), true, 1),
        (true, RecoveryInfo::never(), true, 1),
        (true, RecoveryInfo::unavailable(), false, 1),
        (true, RecoveryInfo::unavailable(), true, 3),
    ] {
        let clock = ClockControl::new().to_clock();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&calls);
        let context = ResilienceContext::new(&clock);
        let stack = (
            Retry::layer("stop", &context)
                .clone_input()
                .enable_if(move |_: &u32| enabled)
                .recovery_with(move |_: &Result<u32, String>, _| recovery.clone())
                .handle_unavailable(unavailable)
                .base_delay(Duration::ZERO)
                .max_retry_attempts(2),
            Execute::new(move |input| {
                count.fetch_add(1, Ordering::Relaxed);
                async move { Ok::<_, String>(input) }
            }),
        );
        assert_eq!(block_on(execute(&mut stack.into_service(), 7, tower)), Ok(7));
        assert_eq!(calls.load(Ordering::Relaxed), expected_calls);
    }
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
fn gamma_survivor_backoff_zero_and_capped_progression_are_exact(#[case] tower: bool) {
    for (backoff, base, expected) in [
        (Backoff::Exponential, Duration::ZERO, [0, 0, 0, 0]),
        (Backoff::Linear, Duration::from_millis(3), [3, 6, 7, 7]),
        (Backoff::Exponential, Duration::from_millis(3), [3, 6, 7, 7]),
    ] {
        let clock = ClockControl::new().auto_advance_timers(true).to_clock();
        let delays = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&delays);
        let context = ResilienceContext::new(&clock);
        let stack = (
            Retry::layer("backoff", &context)
                .clone_input()
                .recovery_with(|_: &Result<u32, String>, _| RecoveryInfo::retry())
                .base_delay(base)
                .backoff(backoff)
                .max_delay(Duration::from_millis(7))
                .use_jitter(false)
                .max_retry_attempts(4)
                .on_retry(move |_, args| captured.lock().unwrap().push(args.retry_delay().as_millis())),
            Execute::new(|input| async move { Ok::<_, String>(input) }),
        );
        assert_eq!(block_on(execute(&mut stack.into_service(), 7, tower)), Ok(7));
        assert_eq!(*delays.lock().unwrap(), expected);
    }
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
fn gamma_survivor_timeout_predicate_controls_deadline_and_drops_pending_inner(#[case] tower: bool) {
    struct OnDrop(Arc<AtomicUsize>);
    impl Drop for OnDrop {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    for enabled in [false, true] {
        let control = ClockControl::new();
        let clock = control.to_clock();
        let drops = Arc::new(AtomicUsize::new(0));
        let captured = Arc::clone(&drops);
        let context = ResilienceContext::new(&clock);
        let stack = (
            Timeout::layer("deadline", &context)
                .timeout_output(|_| Err::<u32, String>(String::from("deadline")))
                .timeout(Duration::from_millis(5))
                .enable_if(move |_: &u32| enabled),
            Execute::new(move |_| {
                let guard = OnDrop(Arc::clone(&captured));
                async move {
                    let _guard = guard;
                    pending::<Result<u32, String>>().await
                }
            }),
        );
        let mut service = stack.into_service();
        {
            let mut future = pin!(execute(&mut service, 7, tower));
            let mut cx = Context::from_waker(Waker::noop());
            assert!(future.as_mut().poll(&mut cx).is_pending());
            control.advance(Duration::from_millis(4));
            assert!(future.as_mut().poll(&mut cx).is_pending());
            assert_eq!(drops.load(Ordering::Relaxed), 0);
            control.advance(Duration::from_millis(1));
            if enabled {
                assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(Err(String::from("deadline"))));
                assert_eq!(drops.load(Ordering::Relaxed), 1);
            } else {
                assert!(future.as_mut().poll(&mut cx).is_pending());
                assert_eq!(drops.load(Ordering::Relaxed), 0);
            }
        }
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
fn gamma_survivor_latency_bypass_and_zero_rate_do_not_advance_clock(#[case] tower: bool) {
    for (enabled, rate, expected) in [(false, 1.0, 0), (true, 0.0, 0), (true, 1.0, 7)] {
        let clock = ClockControl::new().auto_advance_timers(true).to_clock();
        let context = ResilienceContext::new(&clock);
        let stack = (
            Latency::layer("latency", &context)
                .enable_if(move |_: &u32| enabled)
                .rate(rate)
                .latency(Duration::from_millis(7)),
            Execute::new(|input| async move { Ok::<_, String>(input) }),
        );
        let start = clock.instant();
        assert_eq!(block_on(execute(&mut stack.into_service(), 7, tower)), Ok(7));
        assert_eq!(clock.instant().duration_since(start), Duration::from_millis(expected));
    }
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
fn gamma_survivor_delayed_hedges_report_distinct_attempts_and_configured_delays(#[case] tower: bool) {
    let control = ClockControl::new();
    let clock = control.to_clock();
    let executions = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&executions);
    let recovery_attempts = Arc::new(Mutex::new(Vec::new()));
    let recovered = Arc::clone(&recovery_attempts);
    let inner_clock = clock.clone();
    let context = ResilienceContext::new(&clock);
    let stack = (
        Hedging::layer("hedges", &context)
            .config(&HedgingConfig::default())
            .clone_input()
            .hedging_delay_with(|_: &u32, args| Duration::from_millis(u64::from(args.attempt().index()) * 3))
            .on_execute(move |input: &mut u32, args| {
                *input = args.attempt().index();
                captured.lock().unwrap().push((args.attempt().index(), args.delay()));
            })
            .recovery_with(move |_: &Result<u32, String>, args| {
                recovered.lock().unwrap().push(args.attempt().index());
                RecoveryInfo::never()
            })
            .max_hedged_attempts(2),
        Execute::new(move |input| {
            let clock = inner_clock.clone();
            async move {
                if input < 2 {
                    pending::<()>().await;
                } else {
                    clock.delay(Duration::from_millis(1)).await;
                }
                Ok::<_, String>(input)
            }
        }),
    );
    let mut service = stack.into_service();
    let mut future = pin!(execute(&mut service, 0, tower));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(future.as_mut().poll(&mut cx).is_pending());
    control.advance(Duration::from_millis(3));
    assert!(future.as_mut().poll(&mut cx).is_pending());
    control.advance(Duration::from_millis(6));
    assert!(future.as_mut().poll(&mut cx).is_pending());
    control.advance(Duration::from_millis(1));
    assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(2)));
    assert_eq!(
        *executions.lock().unwrap(),
        [(0, Duration::ZERO), (1, Duration::from_millis(3)), (2, Duration::from_millis(6)),]
    );
    assert_eq!(*recovery_attempts.lock().unwrap(), [2]);
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
fn gamma_survivor_ready_recoverable_hedges_launch_next_attempt_without_waiting(#[case] tower: bool) {
    for handle_unavailable in [false, true] {
        let clock = ClockControl::new().auto_advance_timers(true).to_clock();
        let start = clock.instant();
        let context = ResilienceContext::new(&clock);
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&attempts);
        let stack = (
            Hedging::layer("immediate", &context)
                .clone_input()
                .handle_unavailable(handle_unavailable)
                .hedging_delay(Duration::from_secs(30))
                .on_execute(|input: &mut u32, args| *input = args.attempt().index())
                .recovery_with(move |output: &Result<u32, String>, args| {
                    captured.lock().unwrap().push(args.attempt().index());
                    if output == &Ok(0) {
                        RecoveryInfo::unavailable()
                    } else {
                        RecoveryInfo::never()
                    }
                })
                .max_hedged_attempts(2),
            Execute::new(|input| async move { Ok::<_, String>(input) }),
        );
        assert_eq!(
            block_on(execute(&mut stack.into_service(), 0, tower)),
            Ok(u32::from(handle_unavailable))
        );
        assert_eq!(*attempts.lock().unwrap(), if handle_unavailable { vec![0, 1] } else { vec![0] });
        assert_eq!(clock.instant(), start);
    }
}

#[cfg(all(feature = "logs", feature = "breaker", feature = "fallback", feature = "chaos-injection"))]
#[rstest]
#[case::layered(false)]
#[case::tower(true)]
fn gamma_survivor_all_middleware_logs_are_opt_in_and_hedge_recovery_is_not_abandoned(#[case] tower: bool) {
    use seatbelt::breaker::{Breaker, HalfOpenMode};
    use seatbelt::chaos::injection::Injection;
    use seatbelt::fallback::Fallback;
    use testing_aids::tracing_logs::Capture;
    use tracing_subscriber::util::SubscriberInitExt;

    for logs in [false, true] {
        let capture = Capture::new();
        let _guard = capture.subscriber().set_default();
        let control = ClockControl::new().auto_advance_timers(true);
        let clock = control.to_clock();
        let context = ResilienceContext::new(&clock).name("gamma_pipeline");
        let context = if logs { context.use_logs() } else { context };
        let fallback = (
            Fallback::layer("gamma_fallback", &context)
                .should_fallback(|_: &Result<u32, String>| true)
                .fallback(|_, _| Ok::<_, String>(11)),
            Execute::new(|input| async move { Ok::<_, String>(input) }),
        );
        assert_eq!(block_on(execute(&mut fallback.into_service(), 7, tower)), Ok(11));
        let injection = (
            Injection::layer("gamma_injection", &context)
                .rate(1.0)
                .output_with(|_, _| Ok::<_, String>(13)),
            Execute::new(|_| async move { panic!("full-rate injection must bypass the inner service") }),
        );
        assert_eq!(block_on(execute(&mut injection.into_service(), 7, tower)), Ok(13));
        let timeout = (
            Timeout::layer("gamma_timeout", &context)
                .timeout(Duration::from_millis(1))
                .timeout_output(|_| Err("deadline".to_owned())),
            Execute::new(|_: u32| pending::<Result<u32, String>>()),
        );
        assert_eq!(block_on(execute(&mut timeout.into_service(), 7, tower)), Err("deadline".to_owned()));
        let hedging = (
            Hedging::layer("gamma_hedging", &context)
                .clone_input()
                .on_execute(|input: &mut u32, args| *input = args.attempt().index())
                .recovery_with(|out: &Result<u32, String>, _| {
                    if out == &Ok(0) {
                        RecoveryInfo::retry()
                    } else {
                        RecoveryInfo::never()
                    }
                })
                .max_hedged_attempts(1),
            Execute::new(|input| async move { Ok::<_, String>(input) }),
        );
        assert_eq!(block_on(execute(&mut hedging.into_service(), 0, tower)), Ok(1));
        let breaker = (
            Breaker::layer("gamma_breaker", &context)
                .recovery_with(|out: &Result<u32, String>, _| {
                    if out == &Ok(0) {
                        RecoveryInfo::retry()
                    } else {
                        RecoveryInfo::never()
                    }
                })
                .rejected_input(|_, _| Err("rejected".to_owned()))
                .min_throughput(1)
                .break_duration(Duration::from_secs(1))
                .half_open_mode(HalfOpenMode::quick()),
            Execute::new(|input| async move { Ok::<_, String>(input) }),
        );
        let mut breaker = breaker.into_service();
        assert_eq!(block_on(execute(&mut breaker, 0, tower)), Ok(0));
        assert_eq!(block_on(execute(&mut breaker, 1, tower)), Err("rejected".to_owned()));
        control.advance(Duration::from_secs(1));
        assert_eq!(block_on(execute(&mut breaker, 1, tower)), Ok(1));
        let output = capture.output();
        if logs {
            for name in ["gamma_fallback", "gamma_injection", "gamma_timeout", "gamma_hedging"] {
                assert_eq!(output.lines().filter(|line| line.contains(name)).count(), 1, "{output}");
            }
            assert_eq!(output.lines().filter(|line| line.contains("gamma_breaker")).count(), 4, "{output}");
            let hedge = output.lines().find(|line| line.contains("gamma_hedging")).unwrap();
            assert!(hedge.contains("retry"), "{hedge}");
            assert!(!hedge.contains("abandoned"), "{hedge}");
        } else {
            assert_eq!(output, "");
        }
    }
}

#[cfg(feature = "logs")]
#[rstest]
#[case::layered(false)]
#[case::tower(true)]
fn gamma_survivor_retry_and_latency_logs_respect_context_opt_in(#[case] tower: bool) {
    use testing_aids::tracing_logs::Capture;
    use tracing_subscriber::util::SubscriberInitExt;

    for logs in [false, true] {
        let capture = Capture::new();
        let _guard = capture.subscriber().set_default();
        let clock = ClockControl::new().auto_advance_timers(true).to_clock();
        let context = ResilienceContext::new(&clock).name("gamma_pipeline");
        let context = if logs { context.use_logs() } else { context };
        let retry = (
            Retry::layer("gamma_retry", &context)
                .clone_input()
                .recovery_with(|_: &Result<u32, String>, _| RecoveryInfo::retry())
                .base_delay(Duration::ZERO)
                .max_retry_attempts(1),
            Execute::new(|input| async move { Ok::<_, String>(input) }),
        );
        assert_eq!(block_on(execute(&mut retry.into_service(), 7, tower)), Ok(7));
        let latency = (
            Latency::layer("gamma_latency", &context)
                .rate(1.0)
                .latency(Duration::from_millis(7)),
            Execute::new(|input| async move { Ok::<_, String>(input) }),
        );
        assert_eq!(block_on(execute(&mut latency.into_service(), 7, tower)), Ok(7));
        let output = capture.output();
        if logs {
            assert_eq!(output.lines().filter(|line| line.contains("gamma_retry")).count(), 2);
            assert_eq!(output.lines().filter(|line| line.contains("gamma_latency")).count(), 1);
            assert!(output.contains("pipeline.name=gamma_pipeline"));
            assert!(output.contains("latency.ms=7"));
        } else {
            assert_eq!(output, "");
        }
    }
}

#[cfg(feature = "metrics")]
type MetricEvents = Vec<(u64, Vec<opentelemetry::KeyValue>)>;

#[cfg(feature = "metrics")]
#[derive(Clone, Default)]
struct MetricProvider(Arc<Mutex<MetricEvents>>);

#[cfg(feature = "metrics")]
impl opentelemetry::metrics::MeterProvider for MetricProvider {
    fn meter_with_scope(&self, scope: opentelemetry::InstrumentationScope) -> opentelemetry::metrics::Meter {
        assert_eq!(scope.name(), "seatbelt");
        opentelemetry::metrics::Meter::new(Arc::new(self.clone()))
    }
}

#[cfg(feature = "metrics")]
impl opentelemetry::metrics::InstrumentProvider for MetricProvider {
    fn u64_counter(
        &self,
        builder: opentelemetry::metrics::InstrumentBuilder<'_, opentelemetry::metrics::Counter<u64>>,
    ) -> opentelemetry::metrics::Counter<u64> {
        assert_eq!(builder.name, "resilience.event");
        assert_eq!(
            builder.description.as_deref(),
            Some("Emitted upon the occurrence of a resilience event.")
        );
        assert_eq!(builder.unit.as_deref(), Some("u64"));
        opentelemetry::metrics::Counter::new(Arc::new(self.clone()))
    }
}

#[cfg(feature = "metrics")]
#[cfg(test)]
impl opentelemetry::metrics::SyncInstrument<u64> for MetricProvider {
    fn measure(&self, measurement: u64, attributes: &[opentelemetry::KeyValue]) {
        self.0.lock().unwrap().push((measurement, attributes.to_vec()));
    }
}

#[cfg(feature = "metrics")]
#[rstest]
#[case::layered(false)]
#[case::tower(true)]
fn gamma_survivor_metrics_register_exact_description_and_record_one_per_latency_event(#[case] tower: bool) {
    let provider = MetricProvider::default();
    let clock = ClockControl::new().auto_advance_timers(true).to_clock();
    let context = ResilienceContext::new(&clock).name("gamma_pipeline").use_metrics(&provider);
    let latency = (
        Latency::layer("gamma_latency", &context)
            .rate(1.0)
            .latency(Duration::from_millis(7)),
        Execute::new(|input| async move { Ok::<_, String>(input) }),
    );
    assert_eq!(block_on(execute(&mut latency.into_service(), 7, tower)), Ok(7));
    let events = provider.0.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, 1);
    assert!(
        events[0]
            .1
            .contains(&opentelemetry::KeyValue::new("resilience.pipeline.name", "gamma_pipeline"))
    );
    assert!(
        events[0]
            .1
            .contains(&opentelemetry::KeyValue::new("resilience.strategy.name", "gamma_latency"))
    );
}

#[cfg(all(feature = "metrics", feature = "breaker", feature = "fallback", feature = "chaos-injection"))]
#[cfg(test)]
fn measure_metric_event<T>(operation: impl FnOnce() -> T) -> (T, u64) {
    let session = alloc_tracker::Session::new().no_stdout().no_file();
    let measurement = session.operation("gamma_metric_event");
    let result = {
        let _span = measurement.measure_thread().iterations(1);
        operation()
    };
    let allocations = session.to_report().operations().next().unwrap().1.total_allocations_count();
    (result, allocations)
}

#[cfg(all(feature = "metrics", feature = "breaker", feature = "fallback", feature = "chaos-injection"))]
#[cfg(test)]
fn metric_event_contracts(
    context: &ResilienceContext<u32, Result<u32, String>>,
    control: &ClockControl,
    tower: bool,
    owned_names: bool,
) -> Vec<u64> {
    use std::borrow::Cow;

    use seatbelt::breaker::{Breaker, HalfOpenMode};
    use seatbelt::chaos::injection::Injection;
    use seatbelt::fallback::Fallback;

    let name = |name: &'static str| {
        if owned_names {
            Cow::Owned(name.to_owned())
        } else {
            Cow::Borrowed(name)
        }
    };
    let mut allocations = Vec::new();
    macro_rules! run {
        ($service:expr, $input:expr, $expected:expr) => {{
            let service = $service;
            let (output, count) = measure_metric_event(|| block_on(execute(service, $input, tower)));
            assert_eq!(output, $expected);
            allocations.push(count);
        }};
    }
    let fallback = (
        Fallback::layer(name("fallback"), context)
            .should_fallback(|_: &Result<u32, String>| true)
            .fallback(|_, _| Ok::<_, String>(11)),
        Execute::new(|input| async move { Ok::<_, String>(input) }),
    );
    run!(&mut fallback.into_service(), 7, Ok(11));
    let injection = (
        Injection::layer(name("injection"), context)
            .rate(1.0)
            .output_with(|_, _| Ok::<_, String>(13)),
        Execute::new(|_| async move { panic!("full-rate injection must bypass the inner service") }),
    );
    run!(&mut injection.into_service(), 7, Ok(13));
    let timeout = (
        Timeout::layer(name("timeout"), context)
            .timeout(Duration::from_millis(1))
            .timeout_output(|_| Err("deadline".to_owned())),
        Execute::new(|_: u32| pending::<Result<u32, String>>()),
    );
    run!(&mut timeout.into_service(), 7, Err("deadline".to_owned()));
    let hedging = (
        Hedging::layer(name("hedging"), context)
            .clone_input()
            .on_execute(|input: &mut u32, args| *input = args.attempt().index())
            .recovery_with(|out: &Result<u32, String>, _| {
                if out == &Ok(0) {
                    RecoveryInfo::retry()
                } else {
                    RecoveryInfo::never()
                }
            })
            .max_hedged_attempts(1),
        Execute::new(|input| async move { Ok::<_, String>(input) }),
    );
    run!(&mut hedging.into_service(), 0, Ok(1));
    let breaker = (
        Breaker::layer(name("breaker"), context)
            .recovery_with(|out: &Result<u32, String>, _| {
                if out == &Ok(0) {
                    RecoveryInfo::retry()
                } else {
                    RecoveryInfo::never()
                }
            })
            .rejected_input(|_, _| Err("rejected".to_owned()))
            .min_throughput(1)
            .break_duration(Duration::from_secs(1))
            .half_open_mode(HalfOpenMode::quick()),
        Execute::new(|input| async move { Ok::<_, String>(input) }),
    );
    let mut breaker = breaker.into_service();
    run!(&mut breaker, 0, Ok(0));
    run!(&mut breaker, 1, Err("rejected".to_owned()));
    control.advance(Duration::from_secs(1));
    run!(&mut breaker, 1, Ok(1));
    let retry = (
        Retry::layer(name("retry"), context)
            .clone_input()
            .recovery_with(|_: &Result<u32, String>, _| RecoveryInfo::retry())
            .base_delay(Duration::ZERO)
            .max_retry_attempts(1),
        Execute::new(|input| async move { Ok::<_, String>(input) }),
    );
    run!(&mut retry.into_service(), 7, Ok(7));
    let latency = (
        Latency::layer(name("latency"), context).rate(1.0).latency(Duration::from_millis(7)),
        Execute::new(|input| async move { Ok::<_, String>(input) }),
    );
    run!(&mut latency.into_service(), 7, Ok(7));
    allocations
}

#[cfg(all(feature = "metrics", feature = "breaker", feature = "fallback", feature = "chaos-injection"))]
#[rstest]
#[case::layered(false)]
#[case::tower(true)]
fn gamma_survivor_metrics_preserve_ordered_attribute_prefix_for_every_event_kind(#[case] tower: bool) {
    let provider = MetricProvider::default();
    let control = ClockControl::new().auto_advance_timers(true);
    let context = ResilienceContext::new(control.to_clock()).name("pipeline").use_metrics(&provider);
    metric_event_contracts(&context, &control, tower, false);
    let events = provider.0.lock().unwrap();
    for (measurement, attributes) in &*events {
        assert_eq!(*measurement, 1);
        assert_eq!(
            attributes
                .iter()
                .take(3)
                .map(|attribute| attribute.key.as_str())
                .collect::<Vec<_>>(),
            ["resilience.pipeline.name", "resilience.strategy.name", "resilience.event.name"]
        );
        assert_eq!(attributes[0], opentelemetry::KeyValue::new("resilience.pipeline.name", "pipeline"));
    }
    for name in ["fallback", "injection", "timeout", "hedging", "breaker", "retry", "latency"] {
        let count = events
            .iter()
            .filter(|(_, attributes)| attributes[1] == opentelemetry::KeyValue::new("resilience.strategy.name", name))
            .count();
        if name == "breaker" {
            assert_eq!(count, 4);
        } else {
            assert!(count >= 1, "the {name} event must reach the ordered-attribute oracle");
        }
    }
}

#[cfg(all(feature = "metrics", feature = "breaker", feature = "fallback", feature = "chaos-injection"))]
#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[cfg_attr(miri, ignore = "native tests cover allocation counts; middleware behavior retains Miri coverage")]
fn gamma_survivor_disabled_metrics_do_not_allocate_clones_of_owned_telemetry_names(#[case] tower: bool) {
    let warm_control = ClockControl::new().auto_advance_timers(true);
    let warm_context = ResilienceContext::new(warm_control.to_clock()).name("pipeline");
    metric_event_contracts(&warm_context, &warm_control, tower, false);
    let mut counts = Vec::new();
    for owned in [false, true] {
        let control = ClockControl::new().auto_advance_timers(true);
        let name = if owned {
            std::borrow::Cow::Owned("pipeline".to_owned())
        } else {
            std::borrow::Cow::Borrowed("pipeline")
        };
        let context = ResilienceContext::new(control.to_clock()).name(name);
        counts.push(metric_event_contracts(&context, &control, tower, owned));
    }
    assert_eq!(counts[0].len(), 9);
    let owned_name_overhead = counts[1]
        .iter()
        .zip(&counts[0])
        .map(|(owned, borrowed)| {
            assert!(owned >= borrowed, "allocation counts: {counts:?}");
            owned - borrowed
        })
        .collect::<Vec<_>>();
    // Two hedging guards retain both labels independently of metrics configuration.
    assert_eq!(owned_name_overhead, [0, 0, 0, 4, 0, 0, 0, 0, 0]);
}
