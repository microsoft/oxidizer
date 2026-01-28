// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "retry")]
#![allow(dead_code, reason = "This is a test module")]
#![allow(missing_docs, reason = "This is a test module")]

//! Integration tests for retry middleware using only public API.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use layered::{Execute, Service, Stack};
use seatbelt::retry::{Backoff, OnRetryArgs, RecoveryArgs, Retry};
use seatbelt::{RecoveryInfo, ResilienceContext};
use tick::{Clock, ClockControl};

#[tokio::test]
async fn retry_disabled_no_inner_calls() {
    let clock = Clock::new_frozen();
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = Arc::clone(&counter);

    let context: ResilienceContext<String, String> = ResilienceContext::new(&clock).name("test_pipeline");
    let stack = (
        Retry::layer("test_retry", &context)
            .clone_input_with(move |input: &mut String, _args| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                Some(input.clone())
            })
            .recovery_with(|_: &String, _| RecoveryInfo::retry())
            .disable(),
        Execute::new(move |v: String| async move { v }),
    );

    let service = stack.into_service();
    let result = service.execute("test".to_string()).await;

    assert_eq!(result, "test");
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn uncloneable_recovery_called() {
    let clock = Clock::new_frozen();
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = Arc::clone(&counter);

    let context: ResilienceContext<String, String> = ResilienceContext::new(&clock).name("test_pipeline");
    let stack = (
        Retry::layer("test_retry", &context)
            .clone_input_with(move |_input: &mut String, _args| None)
            .recovery_with(move |_input: &String, _args| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                RecoveryInfo::retry()
            }),
        Execute::new(move |v: String| async move { v }),
    );

    let service = stack.into_service();
    let result = service.execute("test".to_string()).await;

    assert_eq!(result, "test");
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn no_recovery_ensure_no_additional_retries() {
    let clock = Clock::new_frozen();
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = Arc::clone(&counter);

    let context: ResilienceContext<String, String> = ResilienceContext::new(&clock).name("test_pipeline");
    let stack = (
        Retry::layer("test_retry", &context)
            .clone_input_with(move |input: &mut String, _args| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                Some(input.clone())
            })
            .recovery_with(move |_input: &String, _args| RecoveryInfo::never()),
        Execute::new(move |v: String| async move { v }),
    );

    let service = stack.into_service();
    let result = service.execute("test".to_string()).await;

    assert_eq!(result, "test");
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retry_recovery_ensure_retries_exhausted() {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = Arc::clone(&counter);

    let context: ResilienceContext<String, String> = ResilienceContext::new(&clock).name("test_pipeline");
    let stack = (
        Retry::layer("test_retry", &context)
            .clone_input_with(move |input: &mut String, _args| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                Some(input.clone())
            })
            .recovery_with(move |_input: &String, _args| RecoveryInfo::retry())
            .max_retry_attempts(4),
        Execute::new(move |v: String| async move { v }),
    );

    let service = stack.into_service();
    let result = service.execute("test".to_string()).await;

    assert_eq!(result, "test");
    assert_eq!(counter.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn retry_recovery_ensure_correct_delays() {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let delays = Arc::new(Mutex::new(vec![]));
    let delays_clone = Arc::clone(&delays);

    let context: ResilienceContext<String, String> = ResilienceContext::new(&clock).name("test_pipeline");
    let stack = (
        Retry::layer("test_retry", &context)
            .clone_input_with(move |input: &mut String, _args| Some(input.clone()))
            .use_jitter(false)
            .backoff(Backoff::Linear)
            .recovery_with(move |_input: &String, _args| RecoveryInfo::retry())
            .max_retry_attempts(4)
            .on_retry(move |_output: &String, args: OnRetryArgs| {
                delays_clone.lock().unwrap().push(args.retry_delay());
            }),
        Execute::new(move |v: String| async move { v }),
    );

    let service = stack.into_service();
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
    use seatbelt::retry::Attempt;

    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let attempts = Arc::new(Mutex::new(vec![]));
    let attempts_clone = Arc::clone(&attempts);

    let attempts_for_clone = Arc::new(Mutex::new(vec![]));
    let attempts_for_clone_clone = Arc::clone(&attempts_for_clone);

    let context: ResilienceContext<String, String> = ResilienceContext::new(&clock).name("test_pipeline");
    let stack = (
        Retry::layer("test_retry", &context)
            .clone_input_with(move |input: &mut String, args| {
                attempts_for_clone_clone.lock().unwrap().push(args.attempt());
                Some(input.clone())
            })
            .recovery_with(move |_input: &String, _args| RecoveryInfo::retry())
            .max_retry_attempts(4)
            .on_retry(move |_output: &String, args: OnRetryArgs| {
                attempts_clone.lock().unwrap().push(args.attempt());
            }),
        Execute::new(move |v: String| async move { v }),
    );

    let service = stack.into_service();
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
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let call_count = Arc::new(AtomicU32::new(0));
    let call_count_clone = Arc::clone(&call_count);
    let restore_count = Arc::new(AtomicU32::new(0));
    let restore_count_clone = Arc::clone(&restore_count);

    let context: ResilienceContext<String, String> = ResilienceContext::new(&clock).name("test_pipeline");
    let stack = (
        Retry::layer("test_retry", &context)
            .clone_input_with(|_input: &mut String, _args| None) // Don't clone - force restore path
            .restore_input(move |output: &mut String, _args| {
                restore_count_clone.fetch_add(1, Ordering::SeqCst);
                output.contains("error:").then(|| {
                    let input = output.replace("error:", "");
                    *output = "restored".to_string();
                    input
                })
            })
            .recovery_with(|output: &String, _args| {
                if output.contains("error:") {
                    RecoveryInfo::retry()
                } else {
                    RecoveryInfo::never()
                }
            })
            .max_retry_attempts(2),
        Execute::new(move |input: String| {
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
        }),
    );

    let service = stack.into_service();
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

    let context: ResilienceContext<String, String> = ResilienceContext::new(&clock).name("test_pipeline");
    let stack = (
        Retry::layer("test_retry", &context)
            .clone_input_with(move |input: &mut String, _args| {
                call_count_clone.fetch_add(1, Ordering::SeqCst);
                Some(input.clone())
            })
            .recovery_with(|_output: &String, _args| RecoveryInfo::unavailable()),
        Execute::new(move |v: String| async move { v }),
    );

    let service = stack.into_service();
    let result = service.execute("test".to_string()).await;

    // Should not retry when outage handling is disabled
    assert_eq!(result, "test");
    assert_eq!(call_count.load(Ordering::SeqCst), 1); // Only original call, no retries
}

#[tokio::test]
async fn outage_handling_enabled_with_retries() {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let call_count = Arc::new(AtomicU32::new(0));
    let call_count_clone = Arc::clone(&call_count);

    let context: ResilienceContext<String, String> = ResilienceContext::new(&clock).name("test_pipeline");
    let stack = (
        Retry::layer("test_retry", &context)
            .clone_input_with(move |input: &mut String, _args| {
                call_count_clone.fetch_add(1, Ordering::SeqCst);
                Some(input.clone())
            })
            .recovery_with(|_output: &String, args: RecoveryArgs| {
                // First attempt returns outage, subsequent attempts succeed
                if args.attempt().index() == 0 {
                    RecoveryInfo::unavailable()
                } else {
                    RecoveryInfo::never()
                }
            })
            .handle_unavailable(true) // Enable outage handling
            .max_retry_attempts(2),
        Execute::new(move |input: String| async move { format!("processed_{input}") }),
    );

    let service = stack.into_service();
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

    let context: ResilienceContext<String, String> = ResilienceContext::new(&clock).name("test_pipeline");
    let stack = (
        Retry::layer("test_retry", &context)
            .clone_input_with(move |input: &mut String, _args| Some(input.clone()))
            .recovery_with(|_output: &String, args: RecoveryArgs| {
                if args.attempt().index() == 0 {
                    RecoveryInfo::unavailable().delay(Duration::from_secs(10)) // 10 second recovery hint
                } else {
                    RecoveryInfo::never()
                }
            })
            .handle_unavailable(true)
            .max_retry_attempts(1)
            .on_retry(move |_output: &String, args: OnRetryArgs| {
                delays_clone.lock().unwrap().push(args.retry_delay());
            }),
        Execute::new(move |v: String| async move { v }),
    );

    let service = stack.into_service();
    let _result = service.execute("test".to_string()).await;

    // Should use the recovery hint as the delay
    assert_eq!(delays.lock().unwrap().to_vec(), vec![Duration::from_secs(10)]);
}

#[tokio::test]
async fn clone_service_works_independently() {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let call_count = Arc::new(AtomicU32::new(0));
    let call_count_clone = Arc::clone(&call_count);

    let context: ResilienceContext<String, String> = ResilienceContext::new(&clock).name("test_pipeline");
    let stack = (
        Retry::layer("test_retry", &context)
            .clone_input_with(|input: &mut String, _args| Some(input.clone()))
            .recovery_with(|_output: &String, _args| RecoveryInfo::retry())
            .max_retry_attempts(2),
        Execute::new(move |input: String| {
            let count = call_count_clone.fetch_add(1, Ordering::SeqCst);
            async move {
                if count < 2 {
                    format!("attempt_{count}:{input}")
                } else {
                    format!("success:{input}")
                }
            }
        }),
    );

    let service = stack.into_service();
    let cloned_service = service.clone();

    // Both services should work independently
    let result1 = service.execute("original".to_string()).await;
    let result2 = cloned_service.execute("cloned".to_string()).await;

    assert_eq!(result1, "success:original");
    assert_eq!(result2, "success:cloned");
    // Each service ran through retry cycle: 3 attempts each = 6 total
    assert_eq!(call_count.load(Ordering::SeqCst), 6);
}
