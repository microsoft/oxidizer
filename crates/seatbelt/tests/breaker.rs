// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(dead_code, reason = "This is a test module")]
#![allow(missing_docs, reason = "This is a test module")]
#![cfg(feature = "breaker")]
#![cfg(not(miri))]

//! Integration tests for circuit breaker middleware using only public API.

use std::future::poll_fn;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use layered::{Execute, Service, Stack};
use rstest::rstest;
use seatbelt::breaker::{Breaker, BreakerId, HalfOpenMode, OnClosedArgs, RejectedInputArgs};
use seatbelt::{RecoveryInfo, ResilienceContext};
use tick::{Clock, ClockControl};
use tower_service::Service as TowerService;

const DEFAULT_BREAK_DURATION: Duration = Duration::from_secs(5);

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
async fn breaker_disabled_no_inner_calls(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test_pipeline");

    let stack = (
        Breaker::layer("test_breaker", &context)
            .recovery_with(|output: &Result<String, String>, _| {
                if output.as_ref().is_ok_and(|s| s.contains("error")) {
                    RecoveryInfo::retry()
                } else {
                    RecoveryInfo::never()
                }
            })
            .rejected_input(|_: String, _| Ok("circuit is open".to_string()))
            .disable(),
        Execute::new(move |v: String| async move { Ok::<_, String>(v) }),
    );

    let mut service = stack.into_service();
    let result = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(result, Ok("test".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn passthrough_behavior(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test_pipeline");

    let stack = (
        Breaker::layer("test_breaker", &context)
            .recovery_with(|output: &Result<String, String>, _| {
                if output.as_ref().is_ok_and(|s| s.contains("error")) {
                    RecoveryInfo::retry()
                } else {
                    RecoveryInfo::never()
                }
            })
            .rejected_input(|_: String, _| Ok("circuit is open".to_string())),
        Execute::new(move |v: String| async move { Ok::<_, String>(v) }),
    );

    let mut service = stack.into_service();
    let result = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(result, Ok("test".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn execute_end_to_end_with_callbacks(#[case] use_tower: bool) {
    let probing_called = Arc::new(AtomicBool::new(false));
    let opened_called = Arc::new(AtomicBool::new(false));
    let closed_called = Arc::new(AtomicBool::new(false));

    let probing_called_clone = Arc::clone(&probing_called);
    let opened_called_clone = Arc::clone(&opened_called);
    let closed_called_clone = Arc::clone(&closed_called);

    let clock_control = ClockControl::new();
    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(clock_control.to_clock()).name("test_pipeline");

    // Create a service that transforms input and can trigger different circuit states
    let stack = (
        Breaker::layer("test_breaker", &context)
            .recovery_with(|output: &Result<String, String>, _| {
                if output.as_ref().is_ok_and(|s| s.contains("error")) {
                    RecoveryInfo::retry()
                } else {
                    RecoveryInfo::never()
                }
            })
            .rejected_input(|_: String, _| Ok("circuit is open".to_string()))
            .min_throughput(5)
            .half_open_mode(HalfOpenMode::quick())
            .on_probing(move |input: &mut String, _| {
                assert_eq!(input, "probe_input");
                probing_called.store(true, Ordering::SeqCst);
            })
            .on_opened(move |output: Option<&Result<String, String>>, _| {
                assert_eq!(output.unwrap().as_ref().unwrap(), "error_output");
                opened_called.store(true, Ordering::SeqCst);
            })
            .on_closed(move |output: &Result<String, String>, args: OnClosedArgs| {
                assert_eq!(output.as_ref().unwrap(), "probe_output");
                assert!(args.open_duration() > Duration::ZERO);
                closed_called.store(true, Ordering::SeqCst);
            }),
        Execute::new(move |input: String| async move {
            // Transform input to simulate different scenarios
            Ok::<_, String>(match input.as_str() {
                "probe_input" => "probe_output".to_string(),
                "success_input" => "success_output".to_string(),
                "error_input" => "error_output".to_string(),
                _ => input,
            })
        }),
    );

    let mut service = stack.into_service();

    // break the circuit first by simulating failures
    for _ in 0..5 {
        let result = execute_service(&mut service, "error_input".to_string(), use_tower).await;
        assert_eq!(result, Ok("error_output".to_string()));
    }

    // rejected input
    let result = execute_service(&mut service, "success_input".to_string(), use_tower).await;
    assert_eq!(result, Ok("circuit is open".to_string()));
    assert!(opened_called_clone.load(Ordering::SeqCst));
    assert!(!closed_called_clone.load(Ordering::SeqCst));

    // send probe and close the circuit
    clock_control.advance(DEFAULT_BREAK_DURATION);
    let result = execute_service(&mut service, "probe_input".to_string(), use_tower).await;
    assert_eq!(result, Ok("probe_output".to_string()));
    assert!(probing_called_clone.load(Ordering::SeqCst));
    assert!(closed_called_clone.load(Ordering::SeqCst));

    // normal execution should pass through
    let result = execute_service(&mut service, "success_input".to_string(), use_tower).await;
    assert_eq!(result, Ok("success_output".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn abandoned_executions_open_circuit(#[case] use_tower: bool) {
    use futures::FutureExt;

    let opened_called = Arc::new(AtomicBool::new(false));
    let opened_output_was_none = Arc::new(AtomicBool::new(false));
    let opened_called_clone = Arc::clone(&opened_called);
    let opened_output_was_none_clone = Arc::clone(&opened_output_was_none);

    let clock = Clock::new_frozen();
    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test_pipeline");

    let stack = (
        Breaker::layer("test_breaker", &context)
            .recovery_with(|_: &Result<String, String>, _| RecoveryInfo::never())
            .rejected_input(|_: String, _| Ok("circuit is open".to_string()))
            .min_throughput(3)
            .on_opened(move |output: Option<&Result<String, String>>, _| {
                opened_called.store(true, Ordering::SeqCst);
                // An abandoned execution produces no output.
                opened_output_was_none.store(output.is_none(), Ordering::SeqCst);
            }),
        // The inner service never completes, so every execution is abandoned when its future is dropped.
        Execute::new(|_: String| async move { std::future::pending::<Result<String, String>>().await }),
    );

    let mut service = stack.into_service();

    // Abandon several executions by polling each future once (so the circuit breaker accepts it) and
    // then dropping it while the inner service is still pending.
    for _ in 0..3 {
        if use_tower {
            poll_fn(|cx| service.poll_ready(cx)).await.unwrap();
            let future = TowerService::call(&mut service, "input".to_string());
            assert!(future.now_or_never().is_none(), "execution should not complete");
        } else {
            let future = service.execute("input".to_string());
            assert!(future.now_or_never().is_none(), "execution should not complete");
        }
    }

    // With no successful executions, the abandoned ones are considered and open the circuit.
    assert!(opened_called_clone.load(Ordering::SeqCst));
    assert!(opened_output_was_none_clone.load(Ordering::SeqCst));

    // Subsequent executions are rejected because the circuit is now open.
    let result = execute_service(&mut service, "input".to_string(), use_tower).await;
    assert_eq!(result, Ok("circuit is open".to_string()));
}

/// Regression test for the tower path: `enter` runs synchronously inside `call`, so a future that
/// is dropped *before it is ever polled* must still be recorded as abandoned. Otherwise an accepted
/// execution would leak without ever affecting the circuit breaker state.
#[tokio::test]
async fn tower_abandons_future_dropped_before_first_poll() {
    let opened_called = Arc::new(AtomicBool::new(false));
    let opened_called_clone = Arc::clone(&opened_called);

    let clock = Clock::new_frozen();
    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test_pipeline");

    let stack = (
        Breaker::layer("test_breaker", &context)
            .recovery_with(|_: &Result<String, String>, _| RecoveryInfo::never())
            .rejected_input(|_: String, _| Ok("circuit is open".to_string()))
            .min_throughput(3)
            .on_opened(move |_output: Option<&Result<String, String>>, _| {
                opened_called.store(true, Ordering::SeqCst);
            }),
        Execute::new(|_: String| async move { std::future::pending::<Result<String, String>>().await }),
    );

    let mut service = stack.into_service();

    // Build each future via `call` and drop it immediately without polling.
    for _ in 0..3 {
        poll_fn(|cx| TowerService::poll_ready(&mut service, cx)).await.unwrap();
        let future = TowerService::call(&mut service, "input".to_string());
        drop(future);
    }

    // Even though none of the futures were ever polled, the abandonments were recorded and the
    // circuit is now open.
    assert!(opened_called_clone.load(Ordering::SeqCst));

    let result = execute_service(&mut service, "input".to_string(), true).await;
    assert_eq!(result, Ok("circuit is open".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn abandoned_executions_ignored_when_successes_present(#[case] use_tower: bool) {
    use futures::FutureExt;

    let opened_called = Arc::new(AtomicBool::new(false));
    let opened_called_clone = Arc::clone(&opened_called);

    let clock = Clock::new_frozen();
    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test_pipeline");

    let stack = (
        Breaker::layer("test_breaker", &context)
            .recovery_with(|_: &Result<String, String>, _| RecoveryInfo::never())
            .rejected_input(|_: String, _| Ok("circuit is open".to_string()))
            .min_throughput(3)
            .on_opened(move |_, _| opened_called.store(true, Ordering::SeqCst)),
        Execute::new(|input: String| async move {
            if input == "complete" {
                Ok::<_, String>("ok".to_string())
            } else {
                std::future::pending::<Result<String, String>>().await
            }
        }),
    );

    let mut service = stack.into_service();

    // Record a successful execution so abandoned executions are subsequently ignored.
    let result = execute_service(&mut service, "complete".to_string(), use_tower).await;
    assert_eq!(result, Ok("ok".to_string()));

    // Abandon many executions; because a success was recorded, they must not open the circuit.
    for _ in 0..10 {
        if use_tower {
            poll_fn(|cx| service.poll_ready(cx)).await.unwrap();
            let future = TowerService::call(&mut service, "abandon".to_string());
            assert!(future.now_or_never().is_none());
        } else {
            let future = service.execute("abandon".to_string());
            assert!(future.now_or_never().is_none());
        }
    }

    assert!(!opened_called_clone.load(Ordering::SeqCst));

    // The circuit is still closed, so a completing execution passes through.
    let result = execute_service(&mut service, "complete".to_string(), use_tower).await;
    assert_eq!(result, Ok("ok".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn different_partitions_ensure_isolated(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test_pipeline");

    let stack = (
        Breaker::layer("test_breaker", &context)
            .breaker_id(|input: &String| BreakerId::from(input.clone()))
            .min_throughput(3)
            .recovery_with(|_: &Result<String, String>, _| RecoveryInfo::retry())
            .rejected_input(|_: String, args: RejectedInputArgs| Ok(format!("circuit is open, breaker: {}", args.breaker_id()))),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();

    // break the circuit for partition "A"
    for _ in 0..3 {
        let result = execute_service(&mut service, "A".to_string(), use_tower).await;
        assert_eq!(result, Ok("A".to_string()));
    }

    let result = execute_service(&mut service, "A".to_string(), use_tower).await;
    assert_eq!(result, Ok("circuit is open, breaker: A".to_string()));

    // Execute on partition "B" should pass through
    let result = execute_service(&mut service, "B".to_string(), use_tower).await;
    assert_eq!(result, Ok("B".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn clone_service_shares_circuit_state(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test_pipeline");

    let stack = (
        Breaker::layer("test_breaker", &context)
            .min_throughput(3)
            .recovery_with(|output: &Result<String, String>, _| {
                if output.as_ref().is_ok_and(|s| s.contains("error")) {
                    RecoveryInfo::retry()
                } else {
                    RecoveryInfo::never()
                }
            })
            .rejected_input(|_: String, _| Ok("circuit is open".to_string())),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();
    let mut cloned_service = service.clone();

    // Trip the circuit using the original service
    for _ in 0..3 {
        let _ = execute_service(&mut service, "error".to_string(), use_tower).await;
    }

    // Both services should see the circuit as open (shared state)
    let result1 = execute_service(&mut service, "test".to_string(), use_tower).await;
    let result2 = execute_service(&mut cloned_service, "test".to_string(), use_tower).await;

    assert_eq!(result1, Ok("circuit is open".to_string()));
    assert_eq!(result2, Ok("circuit is open".to_string()));
}
