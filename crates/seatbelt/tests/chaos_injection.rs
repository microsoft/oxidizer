// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(dead_code, reason = "This is a test module")]
#![allow(missing_docs, reason = "This is a test module")]
#![cfg(feature = "chaos-injection")]
#![cfg(not(miri))]

//! Integration tests for chaos injection middleware using only public API.

use std::future::poll_fn;

use layered::{Execute, Service, Stack};
use rstest::rstest;
use seatbelt::ResilienceContext;
use seatbelt::chaos::injection::{Injection, InjectionConfig};
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
async fn no_injection_when_rate_zero(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let stack = (
        Injection::layer("test_injection", &context)
            .rate(0.0)
            .output_with(|_input, _args| Ok::<_, String>("should_not_appear".to_string())),
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
async fn always_injection_when_rate_one(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let stack = (
        Injection::layer("test_injection", &context)
            .rate(1.0)
            .output_with(|_input, _args| Ok::<_, String>("injected".to_string())),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test input".to_string(), use_tower).await;

    assert_eq!(output, Ok("injected".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn injection_with_fixed_output(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let stack = (
        Injection::layer("test_injection", &context)
            .rate(1.0)
            .output(Ok::<_, String>("fixed_value".to_string())),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(output, Ok("fixed_value".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn no_injection_if_disabled(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let stack = (
        Injection::layer("test_injection", &context)
            .rate(1.0)
            .output_with(|_input, _args| Ok::<_, String>("should_not_appear".to_string()))
            .disable(),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test input".to_string(), use_tower).await;

    // Injection is disabled, so the original output passes through
    assert_eq!(output, Ok("test input".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn enable_if_respected(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let stack = (
        Injection::layer("test_injection", &context)
            .rate(1.0)
            .output_with(|_input, _args| Ok::<_, String>("injected".to_string()))
            .enable_if(|input: &String| input != "bypass"),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();

    // Enabled for this input
    let output1 = execute_service(&mut service, "normal".to_string(), use_tower).await;
    assert_eq!(output1, Ok("injected".to_string()));

    // Disabled for "bypass"
    let output2 = execute_service(&mut service, "bypass".to_string(), use_tower).await;
    assert_eq!(output2, Ok("bypass".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn clone_service_works_independently(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test_pipeline");

    let stack = (
        Injection::layer("test_injection", &context)
            .rate(1.0)
            .output_with(|_input, _args| Ok::<_, String>("injected".to_string())),
        Execute::new(|input: String| async move { Ok::<_, String>(format!("processed:{input}")) }),
    );

    let mut service = stack.into_service();
    let mut cloned_service = service.clone();

    let result1 = execute_service(&mut service, "original".to_string(), use_tower).await;
    let result2 = execute_service(&mut cloned_service, "cloned".to_string(), use_tower).await;

    assert_eq!(result1, Ok("injected".to_string()));
    assert_eq!(result2, Ok("injected".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn config_applies_rate_and_enabled(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let mut config = InjectionConfig::default();
    config.enabled = true;
    config.rate = 1.0;

    let stack = (
        Injection::layer("test_injection", &context)
            .config(&config)
            .output_with(|_input, _args| Ok::<_, String>("config_injected".to_string())),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(output, Ok("config_injected".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn config_disabled_passes_through(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let mut config = InjectionConfig::default();
    config.enabled = false;
    config.rate = 1.0;

    let stack = (
        Injection::layer("test_injection", &context)
            .config(&config)
            .output_with(|_input, _args| Ok::<_, String>("should_not_appear".to_string())),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(output, Ok("test".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn injection_can_inject_errors(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let stack = (
        Injection::layer("test_injection", &context)
            .rate(1.0)
            .output_with(|_input, _args| Err::<String, String>("chaos_error".to_string())),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(output, Err("chaos_error".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn injection_output_error_with(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let stack = (
        Injection::layer("test_injection", &context)
            .rate(1.0)
            .output_error_with(|_input, _args| "chaos_error_with".to_string()),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(output, Err("chaos_error_with".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn injection_output_error_fixed(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let stack = (
        Injection::layer("test_injection", &context)
            .rate(1.0)
            .output_error("fixed_chaos_error".to_string()),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(output, Err("fixed_chaos_error".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn rate_with_dynamic_rate(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let stack = (
        Injection::layer("test_injection", &context)
            .rate_with(|input: &String, _args| if input.starts_with("inject") { 1.0 } else { 0.0 })
            .output_with(|_input, _args| Ok::<_, String>("injected".to_string())),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();

    // "inject_me" starts with "inject" → rate=1.0 → always inject
    let output = execute_service(&mut service, "inject_me".to_string(), use_tower).await;
    assert_eq!(output, Ok("injected".to_string()));

    // "normal" does not start with "inject" → rate=0.0 → never inject
    let output = execute_service(&mut service, "normal".to_string(), use_tower).await;
    assert_eq!(output, Ok("normal".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn rate_with_clamps_above_one(#[case] use_tower: bool) {
    let clock = Clock::new_frozen();
    let context = ResilienceContext::new(clock);

    let stack = (
        Injection::layer("test_injection", &context)
            .rate_with(|_input: &String, _args| 999.0) // out of range, should clamp to 1.0
            .output_with(|_input, _args| Ok::<_, String>("injected".to_string())),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(output, Ok("injected".to_string()));
}

#[tokio::test]
async fn str_references() {
    let clock = Clock::new_frozen();
    let context: ResilienceContext<&str, &str> = ResilienceContext::new(&clock);

    let stack = (
        Injection::layer("test_injection", &context)
            .rate(0.0)
            .output_with(|_input: &str, _args| "injected"),
        Execute::new(|input: &str| async move { input }),
    );

    let input = "hello".to_string();
    let service = stack.into_service();
    let output = service.execute(input.as_str()).await;

    assert_eq!(output, "hello");
}
