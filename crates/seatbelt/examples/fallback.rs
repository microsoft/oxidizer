// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Fallback middleware example that replaces invalid service output with a safe default.
//!
//! This example simulates an unreliable operation that occasionally returns empty
//! responses. The fallback middleware detects these invalid outputs and substitutes
//! a predefined default value.

use layered::{Execute, Service, Stack};
use seatbelt::ResilienceContext;
use seatbelt::fallback::Fallback;
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let context = ResilienceContext::new(&clock);

    // Define stack with fallback layer
    let stack = (
        Fallback::layer("my_fallback", &context)
            // Required: predicate that decides when fallback is needed
            .should_fallback(|output: &String| output.is_empty())
            // Required: the replacement output
            .fallback_output("fallback_value".to_string()),
        Execute::new(execute_operation),
    );

    // Create the service from the stack
    let service = stack.into_service();

    for i in 0..10 {
        let result = service.execute(format!("request-{i}")).await;
        println!("{i}: result = '{result}'");
    }
}

// 30% chance of returning an empty string (invalid output)
async fn execute_operation(input: String) -> String {
    if fastrand::i16(0..10) < 3 {
        String::default()
    } else {
        format!("processed:{input}")
    }
}
