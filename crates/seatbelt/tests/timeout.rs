// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(dead_code, reason = "This is a test module")]
#![allow(missing_docs, reason = "This is a test module")]
#![cfg(feature = "timeout")]

//! Integration tests for timeout middleware using only public API.

use std::future::poll_fn;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use layered::{Execute, Service, Stack};
use rstest::rstest;
use seatbelt::ResilienceContext;
use seatbelt::timeout::Timeout;
use tick::{Clock, ClockControl};
use tower_service::Service as TowerService;

/// Helper to execute a service either via layered::Service or tower_service::Service.
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
async fn no_timeout(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let stack = (
        Timeout::layer("test_timeout", &context)
            .timeout_output(|args| Ok::<_, String>(format!("timed out after {}ms", args.timeout().as_millis())))
            .timeout(Duration::from_secs(5)),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test input".to_string(), use_tower).await;

    assert_eq!(output, Ok("test input".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn timeout(#[case] use_tower: bool) {
    let clock = ClockControl::default()
        .auto_advance(Duration::from_millis(200))
        .auto_advance_limit(Duration::from_millis(500))
        .to_clock();
    let context = ResilienceContext::new(clock.clone());
    let called = Arc::new(AtomicBool::new(false));
    let called_clone = Arc::clone(&called);

    let stack = (
        Timeout::layer("test_timeout", &context)
            .timeout_output(|args| Ok::<_, String>(format!("timed out after {}ms", args.timeout().as_millis())))
            .timeout(Duration::from_millis(200))
            .on_timeout(move |out, args| {
                assert_eq!("timed out after 200ms", out.as_ref().unwrap().as_str());
                assert_eq!(200, args.timeout().as_millis());
                called.store(true, Ordering::SeqCst);
            }),
        Execute::new(move |input| {
            let clock = clock.clone();
            async move {
                clock.delay(Duration::from_secs(1)).await;
                Ok::<_, String>(input)
            }
        }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test input".to_string(), use_tower).await;

    assert_eq!(output, Ok("timed out after 200ms".to_string()));
    assert!(called_clone.load(Ordering::SeqCst));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn timeout_override_ensure_respected(#[case] use_tower: bool) {
    let clock = ClockControl::default()
        .auto_advance(Duration::from_millis(200))
        .auto_advance_limit(Duration::from_millis(5000))
        .to_clock();

    let stack = (
        Timeout::layer("test_timeout", &ResilienceContext::new(clock.clone()))
            .timeout_output(|args| Ok::<_, String>(format!("timed out after {}ms", args.timeout().as_millis())))
            .timeout(Duration::from_millis(200))
            .timeout_override(|input: &String, _args| {
                if input == "ignore" {
                    return None;
                }

                Some(Duration::from_millis(150))
            }),
        Execute::new(move |input| {
            let clock = clock.clone();
            async move {
                clock.delay(Duration::from_secs(10)).await;
                Ok::<_, String>(input)
            }
        }),
    );

    let mut service = stack.into_service();

    let output1 = execute_service(&mut service, "test input".to_string(), use_tower).await;
    assert_eq!(output1, Ok("timed out after 150ms".to_string()));

    let output2 = execute_service(&mut service, "ignore".to_string(), use_tower).await;
    assert_eq!(output2, Ok("timed out after 200ms".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn no_timeout_if_disabled(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let stack = (
        Timeout::layer("test_timeout", &ResilienceContext::new(&clock))
            .timeout_output(|_args| Ok::<_, String>("timed out".to_string()))
            .timeout(Duration::from_millis(200))
            .disable(),
        Execute::new({
            let clock = clock.clone();
            move |input| {
                let clock = clock.clone();
                async move {
                    clock.delay(Duration::from_secs(1)).await;
                    Ok::<_, String>(input)
                }
            }
        }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test input".to_string(), use_tower).await;

    assert_eq!(output, Ok("test input".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn clone_service_works_independently(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test_pipeline");

    let stack = (
        Timeout::layer("test_timeout", &context)
            .timeout_output(|args| Ok::<_, String>(format!("timed out after {}ms", args.timeout().as_millis())))
            .timeout(Duration::from_secs(5)),
        Execute::new(|input: String| async move { Ok::<_, String>(format!("processed:{input}")) }),
    );

    let mut service = stack.into_service();
    let mut cloned_service = service.clone();

    // Both services should work independently
    let result1 = execute_service(&mut service, "original".to_string(), use_tower).await;
    let result2 = execute_service(&mut cloned_service, "cloned".to_string(), use_tower).await;

    assert_eq!(result1, Ok("processed:original".to_string()));
    assert_eq!(result2, Ok("processed:cloned".to_string()));
}
