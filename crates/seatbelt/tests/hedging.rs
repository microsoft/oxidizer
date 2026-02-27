// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(dead_code, reason = "This is a test module")]
#![allow(missing_docs, reason = "This is a test module")]
#![cfg(feature = "hedging")]

//! Integration tests for hedging middleware using only public API.

use std::future::poll_fn;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use layered::{Execute, Service, Stack};
use rstest::rstest;
use seatbelt::hedging::{Hedging, HedgingMode, OnHedgeArgs};
use seatbelt::{RecoveryInfo, ResilienceContext};
use tick::{Clock, ClockControl};
use tower_service::Service as TowerService;

/// Helper to execute a service either via `layered::Service` or `tower_service::Service`.
async fn execute_service<S, In, Out, Err>(service: &mut S, input: In, use_tower: bool) -> Result<Out, Err>
where
    S: Service<In, Out = Result<Out, Err>> + TowerService<In, Response = Out, Error = Err>,
    S::Future: Send,
    In: Send + 'static,
    Out: Send + 'static,
    Err: Send + 'static,
{
    if use_tower {
        poll_fn(|cx| service.poll_ready(cx)).await?;
        service.call(input).await
    } else {
        service.execute(input).await
    }
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn hedging_disabled_passes_through(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = Arc::clone(&counter);

    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test");
    let stack = (
        Hedging::layer("test_hedging", &context)
            .clone_input()
            .recovery_with(|_: &Result<String, String>, _| RecoveryInfo::retry())
            .disable(),
        Execute::new(move |v: String| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            async move { Ok::<_, String>(v) }
        }),
    );

    let mut service = stack.into_service();
    let result = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(result, Ok("test".to_string()));
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn immediate_mode_all_run_concurrently(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = Arc::clone(&counter);

    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test");
    let stack = (
        Hedging::layer("test_hedging", &context)
            .clone_input()
            .recovery_with(|result: &Result<String, String>, _| match result {
                Ok(_) => RecoveryInfo::never(),
                Err(_) => RecoveryInfo::retry(),
            })
            .hedging_mode(HedgingMode::immediate())
            .max_hedged_attempts(2),
        Execute::new(move |v: String| {
            let count = counter_clone.fetch_add(1, Ordering::SeqCst);
            async move {
                if count < 2 {
                    Err::<String, String>(format!("error_{count}"))
                } else {
                    Ok(format!("success:{v}"))
                }
            }
        }),
    );

    let mut service = stack.into_service();
    let result = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(result, Ok("success:test".to_string()));
    // All 3 attempts (1 original + 2 hedges) should have been launched
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn immediate_mode_returns_first_success(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = Arc::clone(&counter);

    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test");
    let stack = (
        Hedging::layer("test_hedging", &context)
            .clone_input()
            .recovery_with(|result: &Result<String, String>, _| match result {
                Ok(_) => RecoveryInfo::never(),
                Err(_) => RecoveryInfo::retry(),
            })
            .hedging_mode(HedgingMode::immediate())
            .max_hedged_attempts(2),
        Execute::new(move |v: String| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            async move { Ok::<_, String>(format!("ok:{v}")) }
        }),
    );

    let mut service = stack.into_service();
    let result = execute_service(&mut service, "test".to_string(), use_tower).await;

    // First success should be returned
    assert_eq!(result, Ok("ok:test".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn delay_mode_launches_hedge_after_timeout(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = Arc::clone(&counter);

    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test");
    let stack = (
        Hedging::layer("test_hedging", &context)
            .clone_input()
            .recovery_with(|result: &Result<String, String>, _| match result {
                Ok(_) => RecoveryInfo::never(),
                Err(_) => RecoveryInfo::retry(),
            })
            .hedging_mode(HedgingMode::delay(Duration::from_secs(1)))
            .max_hedged_attempts(1),
        Execute::new(move |v: String| {
            let count = counter_clone.fetch_add(1, Ordering::SeqCst);
            async move {
                if count == 0 {
                    // First attempt fails
                    Err::<String, String>("transient".to_string())
                } else {
                    Ok(format!("hedged:{v}"))
                }
            }
        }),
    );

    let mut service = stack.into_service();
    let result = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(result, Ok("hedged:test".to_string()));
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn dynamic_mode_computes_delay(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = Arc::clone(&counter);

    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test");
    let stack = (
        Hedging::layer("test_hedging", &context)
            .clone_input()
            .recovery_with(|result: &Result<String, String>, _| match result {
                Ok(_) => RecoveryInfo::never(),
                Err(_) => RecoveryInfo::retry(),
            })
            .hedging_mode(HedgingMode::dynamic(|args| {
                Duration::from_millis(100 * u64::from(args.attempt().index()))
            }))
            .max_hedged_attempts(2),
        Execute::new(move |v: String| {
            let count = counter_clone.fetch_add(1, Ordering::SeqCst);
            async move {
                if count < 2 {
                    Err::<String, String>(format!("fail_{count}"))
                } else {
                    Ok(format!("ok:{v}"))
                }
            }
        }),
    );

    let mut service = stack.into_service();
    let result = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(result, Ok("ok:test".to_string()));
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn on_hedge_callback_invoked(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let hedge_calls = Arc::new(AtomicU32::new(0));
    let hedge_calls_clone = Arc::clone(&hedge_calls);

    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test");
    let stack = (
        Hedging::layer("test_hedging", &context)
            .clone_input()
            .recovery_with(|_: &Result<String, String>, _| RecoveryInfo::retry())
            .hedging_mode(HedgingMode::immediate())
            .max_hedged_attempts(2)
            .on_hedge(move |_: OnHedgeArgs| {
                hedge_calls_clone.fetch_add(1, Ordering::SeqCst);
            }),
        Execute::new(|v: String| async move { Ok::<_, String>(v) }),
    );

    let mut service = stack.into_service();
    let _result = execute_service(&mut service, "test".to_string(), use_tower).await;

    // on_hedge should be called once for each hedge (2 hedges)
    assert_eq!(hedge_calls.load(Ordering::SeqCst), 2);
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn no_hedges_configured_passes_through(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = Arc::clone(&counter);

    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test");
    let stack = (
        Hedging::layer("test_hedging", &context)
            .clone_input()
            .recovery_with(|_: &Result<String, String>, _| RecoveryInfo::retry())
            .max_hedged_attempts(0),
        Execute::new(move |v: String| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            async move { Ok::<_, String>(v) }
        }),
    );

    let mut service = stack.into_service();
    let result = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(result, Ok("test".to_string()));
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn all_fail_returns_last_result(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();

    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test");
    let stack = (
        Hedging::layer("test_hedging", &context)
            .clone_input()
            .recovery_with(|_: &Result<String, String>, _| RecoveryInfo::retry())
            .hedging_mode(HedgingMode::immediate())
            .max_hedged_attempts(2),
        Execute::new(|_v: String| async move { Err::<String, String>("always_fail".to_string()) }),
    );

    let mut service = stack.into_service();
    let result = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(result, Err("always_fail".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn clone_service_works_independently(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let call_count = Arc::new(AtomicU32::new(0));
    let call_count_clone = Arc::clone(&call_count);

    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test");
    let stack = (
        Hedging::layer("test_hedging", &context)
            .clone_input()
            .recovery_with(|result: &Result<String, String>, _| match result {
                Ok(_) => RecoveryInfo::never(),
                Err(_) => RecoveryInfo::retry(),
            })
            .hedging_mode(HedgingMode::immediate())
            .max_hedged_attempts(1),
        Execute::new(move |v: String| {
            call_count_clone.fetch_add(1, Ordering::SeqCst);
            async move { Ok::<_, String>(format!("ok:{v}")) }
        }),
    );

    let mut service = stack.into_service();
    let mut cloned_service = service.clone();

    let result1 = execute_service(&mut service, "original".to_string(), use_tower).await;
    let result2 = execute_service(&mut cloned_service, "cloned".to_string(), use_tower).await;

    assert_eq!(result1, Ok("ok:original".to_string()));
    assert_eq!(result2, Ok("ok:cloned".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn enable_if_skips_hedging(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = Arc::clone(&counter);

    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test");
    let stack = (
        Hedging::layer("test_hedging", &context)
            .clone_input()
            .recovery_with(|_: &Result<String, String>, _| RecoveryInfo::retry())
            .hedging_mode(HedgingMode::immediate())
            .max_hedged_attempts(2)
            .enable_if(|input| input.contains("hedge")),
        Execute::new(move |v: String| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            async move { Ok::<_, String>(v) }
        }),
    );

    let mut service = stack.into_service();

    // This should NOT be hedged (no "hedge" in input)
    let result = execute_service(&mut service, "normal".to_string(), use_tower).await;
    assert_eq!(result, Ok("normal".to_string()));
    assert_eq!(counter.load(Ordering::SeqCst), 1); // Only 1 call, no hedges
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn clone_returning_none_skips_hedge(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = Arc::clone(&counter);

    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test");
    let stack = (
        Hedging::layer("test_hedging", &context)
            .clone_input_with(|input, args| {
                // Only clone for the original request (attempt 0), refuse all hedges.
                (args.attempt().index() == 0).then(|| input.clone())
            })
            .recovery_with(|result: &Result<String, String>, _| match result {
                Ok(_) => RecoveryInfo::never(),
                Err(_) => RecoveryInfo::retry(),
            })
            .hedging_mode(HedgingMode::delay(Duration::from_millis(10)))
            .max_hedged_attempts(2),
        Execute::new(move |v: String| {
            let count = counter_clone.fetch_add(1, Ordering::SeqCst);
            async move {
                if count == 0 {
                    Err::<String, String>("transient".to_string())
                } else {
                    Ok(format!("hedged:{v}"))
                }
            }
        }),
    );

    let mut service = stack.into_service();
    let result = execute_service(&mut service, "test".to_string(), use_tower).await;

    // Clone refused all hedges, so only the original (recoverable) result returns.
    assert_eq!(result, Err("transient".to_string()));
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn handle_unavailable_continues_hedging(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = Arc::clone(&counter);

    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test");
    let stack = (
        Hedging::layer("test_hedging", &context)
            .clone_input()
            .recovery_with(|result: &Result<String, String>, _| match result {
                Ok(_) => RecoveryInfo::never(),
                Err(e) if e == "unavailable" => RecoveryInfo::unavailable(),
                Err(_) => RecoveryInfo::retry(),
            })
            .hedging_mode(HedgingMode::immediate())
            .max_hedged_attempts(2)
            .handle_unavailable(true),
        Execute::new(move |v: String| {
            let count = counter_clone.fetch_add(1, Ordering::SeqCst);
            async move {
                if count == 0 {
                    Err::<String, String>("unavailable".to_string())
                } else {
                    Ok(format!("ok:{v}"))
                }
            }
        }),
    );

    let mut service = stack.into_service();
    let result = execute_service(&mut service, "test".to_string(), use_tower).await;

    // With handle_unavailable(true), the "unavailable" error is treated as recoverable,
    // so hedging continues and a successful result is returned.
    assert_eq!(result, Ok("ok:test".to_string()));
}
