// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example uses a changing injection rate to simulate an extended outage.
//!
//! It uses [`rate_with`][seatbelt::chaos::injection::InjectionLayer::rate_with] to
//! change the injection rate over time. During the outage, it injects every request
//! at rate 1.0. Outside the outage, the service operates normally at rate 0.0.

use std::sync::Arc;
use std::time::Duration;

use layered::{Execute, Service, Stack};
use seatbelt::ResilienceContext;
use seatbelt::chaos::injection::Injection;
use tick::Clock;

const OUTAGE_DURATION: Duration = Duration::from_secs(3);

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let context = ResilienceContext::new(&clock);

    // Create one stopwatch so the callback can measure the outage duration.
    let stopwatch = Arc::new(clock.stopwatch());

    let stack = (
        Injection::layer("outage_injection", &context)
            // For the first 3 seconds, make every request fail.
            // After 3 seconds, let the service operate normally.
            .rate_with(move |_input, _args| {
                if stopwatch.elapsed() < OUTAGE_DURATION {
                    1.0 // full outage
                } else {
                    0.0 // healthy
                }
            })
            .output_with(|input: String, _args| format!("OUTAGE_ERROR for '{input}'")),
        Execute::new(execute_operation),
    );

    let service = stack.into_service();

    for i in 0..6 {
        let result = service.execute(format!("request-{i}")).await;
        println!("  {result}");
        clock.delay(Duration::from_secs(1)).await;
    }
}

async fn execute_operation(input: String) -> String {
    format!("processed:{input}")
}
