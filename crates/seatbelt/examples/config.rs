// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example loads resilience pipeline configuration from a JSON file.
//!
//! It deserializes [`RetryConfig`], [`BreakerConfig`], and [`TimeoutConfig`].
//! It applies them to a retry → circuit breaker → timeout pipeline with `.config()`.

use layered::{Execute, Service, Stack};
use seatbelt::breaker::{Breaker, BreakerConfig};
use seatbelt::retry::{Retry, RetryConfig};
use seatbelt::timeout::{Timeout, TimeoutConfig};
use seatbelt::{RecoveryInfo, ResilienceContext};
use serde::Deserialize;
use tick::Clock;

/// Configuration that matches the top level of the JSON file.
#[derive(Debug, Deserialize)]
struct PipelineConfig {
    retry: RetryConfig,
    breaker: BreakerConfig,
    timeout: TimeoutConfig,
}

#[tokio::main]
async fn main() {
    // Load the pipeline configuration from a JSON file.
    let config: PipelineConfig =
        serde_json::from_str(include_str!("config.json")).expect("config.json should be valid JSON matching PipelineConfig");

    println!("loaded pipeline configuration:\n{config:#?}\n");

    let clock = Clock::new_tokio();
    let context = ResilienceContext::new(&clock);

    // Build the retry → circuit breaker → timeout → operation pipeline.
    //
    // The `.config()` method applies every field from each deserialized value.
    let stack = (
        Retry::layer("my_retry", &context)
            .config(&config.retry)
            .clone_input()
            .recovery_with(|output: &String, _args| match output.as_str() {
                "error" | "circuit is open" | "timed out" => RecoveryInfo::retry(),
                _ => RecoveryInfo::never(),
            }),
        Breaker::layer("my_breaker", &context)
            .config(&config.breaker)
            .recovery_with(|output: &String, _args| match output.as_str() {
                "error" => RecoveryInfo::retry(),
                _ => RecoveryInfo::never(),
            })
            .rejected_input(|_input, _args| "circuit is open".to_string()),
        Timeout::layer("my_timeout", &context)
            .config(&config.timeout)
            .timeout_output(|_args| "timed out".to_string()),
        Execute::new(execute_operation),
    );

    let service = stack.into_service();

    let output = service.execute("hello".to_string()).await;
    println!("output: {output}");
}

async fn execute_operation(input: String) -> String {
    if fastrand::u8(0..10) < 5 { "error".to_string() } else { input }
}
