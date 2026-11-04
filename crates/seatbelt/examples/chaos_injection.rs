// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example injects faults at a configured rate.
//!
//! It intercepts 30% of requests and returns an error.
//! The inner service does not run for these requests.
//! This tests how the service responds to failures.

use layered::{Execute, Service, Stack};
use seatbelt::ResilienceContext;
use seatbelt::chaos::injection::Injection;
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let context = ResilienceContext::new(&clock);

    // Define a stack with an injection layer.
    let stack = (
        Injection::layer("my_injection", &context)
            // Set the injection rate to 30%.
            .rate(0.3)
            // Create the output to inject from the consumed input.
            .output_with(|input: String, _args| format!("INJECTED_FAULT for '{input}'")),
        Execute::new(execute_operation),
    );

    // Create the service from the stack.
    let service = stack.into_service();

    for i in 0..10 {
        let result = service.execute(format!("request-{i}")).await;
        println!("{i}: result = '{result}'");
    }
}

async fn execute_operation(input: String) -> String {
    format!("processed:{input}")
}
