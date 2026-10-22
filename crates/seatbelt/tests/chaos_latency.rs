// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(dead_code, reason = "This is a test module")]
#![allow(missing_docs, reason = "This is a test module")]
#![cfg(feature = "chaos-latency")]
#![cfg(not(miri))]

//! Integration tests for chaos latency middleware using only public API.

use std::future::poll_fn;
use std::time::Duration;

use layered::{Execute, Service, Stack};
use rstest::rstest;
use seatbelt::ResilienceContext;
use seatbelt::chaos::latency::{Latency, LatencyConfig};
use tick::ClockControl;
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
async fn no_latency_when_rate_zero(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let context = ResilienceContext::new(&clock);

    let stack = (
        Latency::layer("test_latency", &context).rate(0.0).latency(Duration::from_secs(10)),
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
async fn always_latency_when_rate_one(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let context = ResilienceContext::new(&clock);

    let stopwatch = clock.stopwatch();

    let stack = (
        Latency::layer("test_latency", &context)
            .rate(1.0)
            .latency(Duration::from_millis(100)),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test input".to_string(), use_tower).await;

    assert_eq!(output, Ok("test input".to_string()));
    // auto_advance_timers advances exactly to the timer deadline.
    assert!(stopwatch.elapsed() >= Duration::from_millis(100));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn latency_with_dynamic_duration(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let context = ResilienceContext::new(&clock);

    let stack = (
        Latency::layer("test_latency", &context)
            .rate(1.0)
            .latency_with(|input: &String, _args| {
                if input.starts_with("slow") {
                    Duration::from_millis(500)
                } else {
                    Duration::from_millis(50)
                }
            }),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();

    let output = execute_service(&mut service, "slow_request".to_string(), use_tower).await;
    assert_eq!(output, Ok("slow_request".to_string()));

    let output = execute_service(&mut service, "fast_request".to_string(), use_tower).await;
    assert_eq!(output, Ok("fast_request".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn no_latency_if_disabled(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let context = ResilienceContext::new(&clock);

    let stack = (
        Latency::layer("test_latency", &context)
            .rate(1.0)
            .latency(Duration::from_secs(10))
            .enable(false),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test input".to_string(), use_tower).await;

    // Injection is disabled, so the original output passes through.
    assert_eq!(output, Ok("test input".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn enable_if_respected(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let context = ResilienceContext::new(&clock);

    let stack = (
        Latency::layer("test_latency", &context)
            .rate(1.0)
            .latency(Duration::from_millis(100))
            .enable_if(|input: &String| input != "bypass"),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();

    // Enabled for this input — latency injected
    let output1 = execute_service(&mut service, "normal".to_string(), use_tower).await;
    assert_eq!(output1, Ok("normal".to_string()));

    // Disabled for "bypass" — no latency
    let output2 = execute_service(&mut service, "bypass".to_string(), use_tower).await;
    assert_eq!(output2, Ok("bypass".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn clone_service_works_independently(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock).name("test_pipeline");

    let stack = (
        Latency::layer("test_latency", &context)
            .rate(1.0)
            .latency(Duration::from_millis(50)),
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
async fn config_applies_rate_and_latency(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let context = ResilienceContext::new(&clock);

    let mut config = LatencyConfig::default();
    config.enabled = true;
    config.rate = 1.0;
    config.latency = Duration::from_millis(50);

    let stopwatch = clock.stopwatch();

    let stack = (
        Latency::layer("test_latency", &context).config(&config),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(output, Ok("test".to_string()));
    assert!(stopwatch.elapsed() >= Duration::from_millis(50));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn config_disabled_passes_through(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let context = ResilienceContext::new(&clock);

    let mut config = LatencyConfig::default();
    config.enabled = false;
    config.rate = 1.0;
    config.latency = Duration::from_secs(10);

    let stack = (
        Latency::layer("test_latency", &context).config(&config),
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
async fn config_with_max_latency_creates_range(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let context = ResilienceContext::new(&clock);

    let mut config = LatencyConfig::default();
    config.enabled = true;
    config.rate = 1.0;
    config.latency = Duration::from_millis(100);
    config.max_latency = Some(Duration::from_millis(500));

    let stopwatch = clock.stopwatch();

    let stack = (
        Latency::layer("test_latency", &context).config(&config),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(output, Ok("test".to_string()));
    // Should have injected latency in [100ms, 500ms)
    let elapsed = stopwatch.elapsed();
    assert!(elapsed >= Duration::from_millis(100));
    assert!(elapsed < Duration::from_millis(500));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn rate_with_dynamic_rate(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let context = ResilienceContext::new(&clock);

    let stack = (
        Latency::layer("test_latency", &context)
            .rate_with(|input: &String, _args| if input.starts_with("slow") { 1.0 } else { 0.0 })
            .latency(Duration::from_millis(100)),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();

    // "slow_me" starts with "slow" -> rate=1.0 -> always inject
    let output = execute_service(&mut service, "slow_me".to_string(), use_tower).await;
    assert_eq!(output, Ok("slow_me".to_string()));

    // "normal" does not start with "slow" -> rate=0.0 -> never inject
    let output = execute_service(&mut service, "normal".to_string(), use_tower).await;
    assert_eq!(output, Ok("normal".to_string()));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn rate_with_clamps_above_one(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let context = ResilienceContext::new(&clock);

    let stack = (
        Latency::layer("test_latency", &context)
            .rate_with(|_input: &String, _args| 999.0) // out of range, should clamp to 1.0
            .latency(Duration::from_millis(50)),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();
    let stopwatch = clock.stopwatch();
    let output = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(output, Ok("test".to_string()));
    assert!(stopwatch.elapsed() >= Duration::from_millis(50));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn latency_range_produces_delay_within_bounds(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let context = ResilienceContext::new(&clock);

    let stack = (
        Latency::layer("test_latency", &context)
            .rate(1.0)
            .latency_range(Duration::from_millis(100)..Duration::from_millis(500)),
        Execute::new(|input: String| async move { Ok::<_, String>(input) }),
    );

    let mut service = stack.into_service();

    let stopwatch = clock.stopwatch();
    let output = execute_service(&mut service, "test".to_string(), use_tower).await;

    assert_eq!(output, Ok("test".to_string()));
    let elapsed = stopwatch.elapsed();
    assert!(elapsed >= Duration::from_millis(100));
    assert!(elapsed < Duration::from_millis(500));
}

#[rstest]
#[case::layered(false)]
#[case::tower(true)]
#[tokio::test]
async fn inner_service_output_preserved(#[case] use_tower: bool) {
    let clock = ClockControl::default().auto_advance_timers(true).to_clock();
    let context = ResilienceContext::new(&clock);

    let stack = (
        Latency::layer("test_latency", &context)
            .rate(1.0)
            .latency(Duration::from_millis(10)),
        Execute::new(|input: String| async move { Ok::<_, String>(format!("processed:{input}")) }),
    );

    let mut service = stack.into_service();
    let output = execute_service(&mut service, "hello".to_string(), use_tower).await;

    // Unlike injection, latency preserves the inner service output.
    assert_eq!(output, Ok("processed:hello".to_string()));
}
