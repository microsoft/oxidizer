// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "timeout")]

//! Integration tests for timeout middleware using only public API.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use layered::{Execute, Service, Stack};
use seatbelt::ResilienceContext;
use seatbelt::timeout::Timeout;
use tick::{Clock, ClockControl};

#[tokio::test]
async fn no_timeout() {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let stack = (
        Timeout::layer("test_timeout", &context)
            .timeout_output(|args| format!("timed out after {}ms", args.timeout().as_millis()))
            .timeout(Duration::from_secs(5)),
        Execute::new(|input: String| async move { input }),
    );

    let service = stack.into_service();
    let output = service.execute("test input".to_string()).await;

    assert_eq!(output, "test input".to_string());
}

#[tokio::test]
async fn timeout() {
    let clock = ClockControl::default()
        .auto_advance(Duration::from_millis(200))
        .auto_advance_limit(Duration::from_millis(500))
        .to_clock();
    let context = ResilienceContext::new(clock.clone());
    let called = Arc::new(AtomicBool::new(false));
    let called_clone = Arc::clone(&called);

    let stack = (
        Timeout::layer("test_timeout", &context)
            .timeout_output(|args| format!("timed out after {}ms", args.timeout().as_millis()))
            .timeout(Duration::from_millis(200))
            .on_timeout(move |out, args| {
                assert_eq!("timed out after 200ms", out.as_str());
                assert_eq!(200, args.timeout().as_millis());
                called.store(true, Ordering::SeqCst);
            }),
        Execute::new(move |input| {
            let clock = clock.clone();
            async move {
                clock.delay(Duration::from_secs(1)).await;
                input
            }
        }),
    );

    let service = stack.into_service();
    let output = service.execute("test input".to_string()).await;

    assert_eq!(output, "timed out after 200ms");
    assert!(called_clone.load(Ordering::SeqCst));
}

#[tokio::test]
async fn timeout_override_ensure_respected() {
    let clock = ClockControl::default()
        .auto_advance(Duration::from_millis(200))
        .auto_advance_limit(Duration::from_millis(5000))
        .to_clock();

    let stack = (
        Timeout::layer("test_timeout", &ResilienceContext::new(clock.clone()))
            .timeout_output(|args| format!("timed out after {}ms", args.timeout().as_millis()))
            .timeout(Duration::from_millis(200))
            .timeout_override(|input, _args| {
                if input == "ignore" {
                    return None;
                }

                Some(Duration::from_millis(150))
            }),
        Execute::new(move |input| {
            let clock = clock.clone();
            async move {
                clock.delay(Duration::from_secs(10)).await;
                input
            }
        }),
    );

    let service = stack.into_service();

    assert_eq!(service.execute("test input".to_string()).await, "timed out after 150ms");
    assert_eq!(service.execute("ignore".to_string()).await, "timed out after 200ms");
}

#[tokio::test]
async fn no_timeout_if_disabled() {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let stack = (
        Timeout::layer("test_timeout", &ResilienceContext::new(&clock))
            .timeout_output(|_args| "timed out".to_string())
            .timeout(Duration::from_millis(200))
            .disable(),
        Execute::new({
            let clock = clock.clone();
            move |input| {
                let clock = clock.clone();
                async move {
                    clock.delay(Duration::from_secs(1)).await;
                    input
                }
            }
        }),
    );

    let service = stack.into_service();
    let output = service.execute("test input".to_string()).await;

    assert_eq!(output, "test input");
}

#[tokio::test]
async fn clone_service_works_independently() {
    let clock = Clock::new_frozen();
    let context: ResilienceContext<String, String> = ResilienceContext::new(&clock).name("test_pipeline");

    let stack = (
        Timeout::layer("test_timeout", &context)
            .timeout_output(|args| format!("timed out after {}ms", args.timeout().as_millis()))
            .timeout(Duration::from_secs(5)),
        Execute::new(|input: String| async move { format!("processed:{input}") }),
    );

    let service = stack.into_service();
    let cloned_service = service.clone();

    // Both services should work independently
    let result1 = service.execute("original".to_string()).await;
    let result2 = cloned_service.execute("cloned".to_string()).await;

    assert_eq!(result1, "processed:original");
    assert_eq!(result2, "processed:cloned");
}
