// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "breaker")]

//! Integration tests for circuit breaker middleware using only public API.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use layered::{Execute, Service, Stack};
use seatbelt::breaker::{Breaker, BreakerId, HalfOpenMode, OnClosedArgs, RejectedInputArgs};
use seatbelt::{RecoveryInfo, ResilienceContext};
use tick::{Clock, ClockControl};

const DEFAULT_BREAK_DURATION: Duration = Duration::from_secs(5);

#[tokio::test]
async fn breaker_disabled_no_inner_calls() {
    let clock = Clock::new_frozen();
    let context: ResilienceContext<String, String> = ResilienceContext::new(&clock).name("test_pipeline");

    let stack = (
        Breaker::layer("test_breaker", &context)
            .recovery_with(|output: &String, _| {
                if output.contains("error") {
                    RecoveryInfo::retry()
                } else {
                    RecoveryInfo::never()
                }
            })
            .rejected_input(|_: String, _| "circuit is open".to_string())
            .disable(),
        Execute::new(move |v: String| async move { v }),
    );

    let service = stack.into_service();
    let result = service.execute("test".to_string()).await;

    assert_eq!(result, "test");
}

#[tokio::test]
async fn passthrough_behavior() {
    let clock = Clock::new_frozen();
    let context: ResilienceContext<String, String> = ResilienceContext::new(&clock).name("test_pipeline");

    let stack = (
        Breaker::layer("test_breaker", &context)
            .recovery_with(|output: &String, _| {
                if output.contains("error") {
                    RecoveryInfo::retry()
                } else {
                    RecoveryInfo::never()
                }
            })
            .rejected_input(|_: String, _| "circuit is open".to_string()),
        Execute::new(move |v: String| async move { v }),
    );

    let service = stack.into_service();
    let result = service.execute("test".to_string()).await;

    assert_eq!(result, "test");
}

#[tokio::test]
async fn execute_end_to_end_with_callbacks() {
    let probing_called = Arc::new(AtomicBool::new(false));
    let opened_called = Arc::new(AtomicBool::new(false));
    let closed_called = Arc::new(AtomicBool::new(false));

    let probing_called_clone = Arc::clone(&probing_called);
    let opened_called_clone = Arc::clone(&opened_called);
    let closed_called_clone = Arc::clone(&closed_called);

    let clock_control = ClockControl::new();
    let context: ResilienceContext<String, String> = ResilienceContext::new(clock_control.to_clock()).name("test_pipeline");

    // Create a service that transforms input and can trigger different circuit states
    let stack = (
        Breaker::layer("test_breaker", &context)
            .recovery_with(|output: &String, _| {
                if output.contains("error") {
                    RecoveryInfo::retry()
                } else {
                    RecoveryInfo::never()
                }
            })
            .rejected_input(|_: String, _| "circuit is open".to_string())
            .min_throughput(5)
            .half_open_mode(HalfOpenMode::quick())
            .on_probing(move |input: &mut String, _| {
                assert_eq!(input, "probe_input");
                probing_called.store(true, Ordering::SeqCst);
            })
            .on_opened(move |output: &String, _| {
                assert_eq!(output, "error_output");
                opened_called.store(true, Ordering::SeqCst);
            })
            .on_closed(move |output: &String, args: OnClosedArgs| {
                assert_eq!(output, "probe_output");
                assert!(args.open_duration() > Duration::ZERO);
                closed_called.store(true, Ordering::SeqCst);
            }),
        Execute::new(move |input: String| async move {
            // Transform input to simulate different scenarios
            match input.as_str() {
                "probe_input" => "probe_output".to_string(),
                "success_input" => "success_output".to_string(),
                "error_input" => "error_output".to_string(),
                _ => input,
            }
        }),
    );

    let service = stack.into_service();

    // break the circuit first by simulating failures
    for _ in 0..5 {
        let result = service.execute("error_input".to_string()).await;
        assert_eq!(result, "error_output");
    }

    // rejected input
    let result = service.execute("success_input".to_string()).await;
    assert_eq!(result, "circuit is open");
    assert!(opened_called_clone.load(Ordering::SeqCst));
    assert!(!closed_called_clone.load(Ordering::SeqCst));

    // send probe and close the circuit
    clock_control.advance(DEFAULT_BREAK_DURATION);
    let result = service.execute("probe_input".to_string()).await;
    assert_eq!(result, "probe_output");
    assert!(probing_called_clone.load(Ordering::SeqCst));
    assert!(closed_called_clone.load(Ordering::SeqCst));

    // normal execution should pass through
    let result = service.execute("success_input".to_string()).await;
    assert_eq!(result, "success_output");
}

#[tokio::test]
async fn different_partitions_ensure_isolated() {
    let clock = Clock::new_frozen();
    let context: ResilienceContext<String, String> = ResilienceContext::new(&clock).name("test_pipeline");

    let stack = (
        Breaker::layer("test_breaker", &context)
            .breaker_id(|input: &String| BreakerId::from(input.clone()))
            .min_throughput(3)
            .recovery_with(|_: &String, _| RecoveryInfo::retry())
            .rejected_input(|_: String, args: RejectedInputArgs| format!("circuit is open, breaker: {}", args.breaker_id())),
        Execute::new(|input: String| async move { input }),
    );

    let service = stack.into_service();

    // break the circuit for partition "A"
    for _ in 0..3 {
        let result = service.execute("A".to_string()).await;
        assert_eq!(result, "A");
    }

    let result = service.execute("A".to_string()).await;
    assert_eq!(result, "circuit is open, breaker: A");

    // Execute on partition "B" should pass through
    let result = service.execute("B".to_string()).await;
    assert_eq!(result, "B");
}

#[tokio::test]
async fn clone_service_shares_circuit_state() {
    let clock = Clock::new_frozen();
    let context: ResilienceContext<String, String> = ResilienceContext::new(&clock).name("test_pipeline");

    let stack = (
        Breaker::layer("test_breaker", &context)
            .min_throughput(3)
            .recovery_with(|output: &String, _| {
                if output.contains("error") {
                    RecoveryInfo::retry()
                } else {
                    RecoveryInfo::never()
                }
            })
            .rejected_input(|_: String, _| "circuit is open".to_string()),
        Execute::new(|input: String| async move { input }),
    );

    let service = stack.into_service();
    let cloned_service = service.clone();

    // Trip the circuit using the original service
    for _ in 0..3 {
        let _ = service.execute("error".to_string()).await;
    }

    // Both services should see the circuit as open (shared state)
    let result1 = service.execute("test".to_string()).await;
    let result2 = cloned_service.execute("test".to_string()).await;

    assert_eq!(result1, "circuit is open");
    assert_eq!(result2, "circuit is open");
}
