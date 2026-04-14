// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(dead_code, reason = "This is a test module")]
#![allow(missing_docs, reason = "This is a test module")]
#![cfg(all(
    feature = "timeout",
    feature = "retry",
    feature = "breaker",
    feature = "hedging",
    feature = "fallback",
    feature = "chaos-injection",
    feature = "chaos-latency",
))]
#![cfg(not(miri))]

//! Integration test verifying that all resilience middleware can be stacked
//! together with non-static reference input and output types (`&str`).

use std::time::Duration;

use layered::{Execute, Intercept, Service, Stack};
use seatbelt::breaker::Breaker;
use seatbelt::chaos::injection::Injection;
use seatbelt::chaos::latency::Latency;
use seatbelt::fallback::Fallback;
use seatbelt::hedging::Hedging;
use seatbelt::retry::Retry;
use seatbelt::timeout::Timeout;
use seatbelt::{RecoveryInfo, ResilienceContext};
use tick::ClockControl;

#[tokio::test]
async fn all_middleware_stacked_with_str_references() {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let context: ResilienceContext<&str, &str> = ResilienceContext::new(&clock);

    let stack = (
        Fallback::layer("test_fallback", &context)
            .should_fallback(|output: &&str| output.is_empty())
            .fallback(|_output, _args| "fallback_value"),
        Intercept::<&str, &str, _>::layer()
            .on_input(|input: &&str| {
                assert!(!input.is_empty());
            })
            .on_output(|output: &&str| {
                assert!(!output.is_empty());
            }),
        Retry::layer("test_retry", &context)
            .clone_input()
            .recovery_with(|_output: &&str, _| RecoveryInfo::never()),
        Hedging::layer("test_hedging", &context)
            .clone_input()
            .recovery_with(|_output: &&str, _| RecoveryInfo::never()),
        Breaker::layer("test_breaker", &context)
            .recovery_with(|_output: &&str, _| RecoveryInfo::never())
            .rejected_input(|_input: &str, _| "circuit is open"),
        Timeout::layer("test_timeout", &context)
            .timeout_output(|_args| "timed out")
            .timeout(Duration::from_secs(5)),
        Injection::layer("test_injection", &context)
            .rate(0.0)
            .output_with(|_input: &str, _args| "injected"),
        Latency::layer("test_latency", &context)
            .rate(0.0)
            .latency(Duration::from_millis(10)),
        Execute::new(|input: &str| async move { input }),
    );

    let service = stack.into_service();
    let output = service.execute("hello").await;

    assert_eq!(output, "hello");
}
