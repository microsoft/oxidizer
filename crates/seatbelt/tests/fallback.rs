// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(dead_code, reason = "This is a test module")]
#![allow(missing_docs, reason = "This is a test module")]
#![cfg(feature = "fallback")]

//! Integration tests for fallback middleware using only public API.

use std::future::poll_fn;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use layered::{Execute, Service, Stack};
use rstest::rstest;
use seatbelt::ResilienceContext;
use seatbelt::fallback::Fallback;
use tick::Clock;
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
async fn no_fallback_when_output_valid(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);
    let before_called = Arc::new(AtomicBool::new(false));
    let before_called_clone = Arc::clone(&before_called);
    let after_called = Arc::new(AtomicBool::new(false));
    let after_called_clone = Arc::clone(&after_called);

    let stack = (
        Fallback::layer("test_fallback", &context)
            .should_fallback(|output: &Result<String, String>| output.is_err())
            .fallback(|_output| Ok::<_, String>("fallback_value".to_string()))
            .before_fallback(move |_output, _args| {
                before_called.store(true, Ordering::SeqCst);
            })
            .after_fallback(move |_output, _args| {
                after_called.store(true, Ordering::SeqCst);
            }),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test input".to_string(), use_tower).await;

    assert_eq!(output, Ok("test input".to_string()));
    assert!(
        !before_called_clone.load(Ordering::SeqCst),
        "before_fallback must not be called when output is valid"
    );
    assert!(
        !after_called_clone.load(Ordering::SeqCst),
        "after_fallback must not be called when output is valid"
    );
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn fallback_invoked_when_output_invalid(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);
    let before_called = Arc::new(AtomicBool::new(false));
    let before_called_clone = Arc::clone(&before_called);
    let after_called = Arc::new(AtomicBool::new(false));
    let after_called_clone = Arc::clone(&after_called);

    let stack = (
        Fallback::layer("test_fallback", &context)
            .should_fallback(|output: &Result<String, String>| output.is_err())
            .fallback(|_output| Ok::<_, String>("replaced".to_string()))
            .before_fallback(move |output, _args| {
                assert!(output.is_err(), "before_fallback should see the original error");
                before_called.store(true, Ordering::SeqCst);
            })
            .after_fallback(move |output, _args| {
                assert_eq!("replaced", output.as_ref().unwrap().as_str());
                after_called.store(true, Ordering::SeqCst);
            }),
        Execute::new(|_input: String| async move { Err::<String, String>("service_error".to_string()) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test input".to_string(), use_tower).await;

    assert_eq!(output, Ok("replaced".to_string()));
    assert!(before_called_clone.load(Ordering::SeqCst));
    assert!(after_called_clone.load(Ordering::SeqCst));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn fallback_async_invoked(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let stack = (
        Fallback::layer("test_fallback", &context)
            .should_fallback(|output: &Result<String, String>| output.is_err())
            .fallback_async(|_output| async { Ok::<_, String>("async_replacement".to_string()) }),
        Execute::new(|_input: String| async move { Err::<String, String>("fail".to_string()) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test input".to_string(), use_tower).await;

    assert_eq!(output, Ok("async_replacement".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn no_fallback_if_disabled(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let stack = (
        Fallback::layer("test_fallback", &context)
            .should_fallback(|output: &Result<String, String>| output.is_err())
            .fallback(|_output| Ok::<_, String>("should_not_appear".to_string()))
            .disable(),
        Execute::new(|_input: String| async move { Err::<String, String>("service_error".to_string()) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test input".to_string(), use_tower).await;

    // Fallback is disabled, so the original error passes through
    assert_eq!(output, Err("service_error".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn enable_if_respected(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let stack = (
        Fallback::layer("test_fallback", &context)
            .should_fallback(|output: &Result<String, String>| output.is_err())
            .fallback(|_output| Ok::<_, String>("replaced".to_string()))
            .enable_if(|input: &String| input != "bypass"),
        Execute::new(|_input: String| async move { Err::<String, String>("fail".to_string()) }),
    );

    let mut service = stack.into_service();

    // Enabled for this input
    let output1 = execute_service(&mut service, "normal".to_string(), use_tower).await;
    assert_eq!(output1, Ok("replaced".to_string()));

    // Disabled for "bypass"
    let output2 = execute_service(&mut service, "bypass".to_string(), use_tower).await;
    assert_eq!(output2, Err("fail".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn clone_service_works_independently(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test_pipeline");

    let stack = (
        Fallback::layer("test_fallback", &context)
            .should_fallback(|output: &Result<String, String>| output.is_err())
            .fallback(|_output| Ok::<_, String>("replaced".to_string())),
        Execute::new(|input: String| async move { Ok::<_, String>(format!("processed:{input}")) }),
    );

    let mut service = stack.into_service();
    let mut cloned_service = service.clone();

    let result1 = execute_service(&mut service, "original".to_string(), use_tower).await;
    let result2 = execute_service(&mut cloned_service, "cloned".to_string(), use_tower).await;

    assert_eq!(result1, Ok("processed:original".to_string()));
    assert_eq!(result2, Ok("processed:cloned".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn fallback_receives_original_output(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let stack = (
        Fallback::layer("test_fallback", &context)
            .should_fallback(|output: &Result<String, String>| output.is_err())
            .fallback(|output: Result<String, String>| {
                let original_err = output.unwrap_err();
                Ok::<_, String>(format!("recovered from: {original_err}"))
            }),
        Execute::new(|_input: String| async move { Err::<String, String>("specific_error".to_string()) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(output, Ok("recovered from: specific_error".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn fallback_output_returns_fixed_value(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let stack = (
        Fallback::layer("test_fallback", &context)
            .should_fallback(|output: &Result<String, String>| output.is_err())
            .fallback_output(Ok::<_, String>("fixed_default".to_string())),
        Execute::new(|_input: String| async move { Err::<String, String>("fail".to_string()) }),
    );

    let mut service = stack.into_service();

    // Each call returns a clone of the fixed value
    let output1 = execute_service(&mut service, "first".to_string(), use_tower).await;
    let output2 = execute_service(&mut service, "second".to_string(), use_tower).await;

    assert_eq!(output1, Ok("fixed_default".to_string()));
    assert_eq!(output2, Ok("fixed_default".to_string()));
}
