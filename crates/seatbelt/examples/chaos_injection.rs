// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Chaos injection middleware example that injects faults with a configurable probability.
//!
//! This example simulates a service where 30% of requests are intercepted and
//! replaced with an injected error output, demonstrating how chaos injection can
//! be used to test resilience under failure conditions.

use layered::{Execute, Service, Stack};
use seatbelt::ResilienceContext;
use seatbelt::chaos::injection::Injection;
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let context = ResilienceContext::new(&clock);

    // Define stack with injection layer
    let stack = (
        Injection::layer("my_injection", &context)
            // Required: probability of injection (30%)
            .rate(0.3)
            // Required: the output to inject (receives the consumed input)
            .output_with(|input: String, _args| format!("INJECTED_FAULT for '{input}'")),
        Execute::new(execute_operation),
    );

    // Create the service from the stack
    let service = stack.into_service();

    for i in 0..10 {
        let result = service.execute(format!("request-{i}")).await;
        println!("{i}: result = '{result}'");
    }
}

async fn execute_operation(input: String) -> String {
    format!("processed:{input}")
}
