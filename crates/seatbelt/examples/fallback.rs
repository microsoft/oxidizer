// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example replaces invalid service output with a safe default.
//!
//! The operation sometimes returns an empty response.
//! The fallback middleware detects it and returns a predefined value.

use layered::{Execute, Service, Stack};
use seatbelt::ResilienceContext;
use seatbelt::fallback::Fallback;
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let context = ResilienceContext::new(&clock);

    // Define a stack with a fallback layer.
    let stack = (
        Fallback::layer("my_fallback", &context)
            // Decide when to use the fallback.
            .should_fallback(|output: &String| output.is_empty())
            // Provide the replacement output.
            .fallback_output("fallback_value".to_string()),
        Execute::new(execute_operation),
    );

    // Create the service from the stack.
    let service = stack.into_service();

    for i in 0..10 {
        let result = service.execute(format!("request-{i}")).await;
        println!("{i}: result = '{result}'");
    }
}

// Return an invalid empty string for 30% of calls.
async fn execute_operation(input: String) -> String {
    if fastrand::i16(0..10) < 3 {
        String::default()
    } else {
        format!("processed:{input}")
    }
}
