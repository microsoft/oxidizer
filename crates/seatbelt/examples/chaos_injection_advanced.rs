// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Advanced chaos injection example that simulates an extended outage using dynamic injection rates.
//!
//! This example uses [`rate_with`][seatbelt::chaos::injection::InjectionLayer::rate_with] to
//! vary the injection probability over time: during a simulated outage window every request
//! is injected (rate 1.0), while outside the window the service operates normally (rate 0.0).

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

    // Create the stopwatch once so the rate callback can track how much time
    // has passed since the outage started.
    let stopwatch = Arc::new(clock.stopwatch());

    let stack = (
        Injection::layer("outage_injection", &context)
            // Dynamically compute the injection rate based on elapsed time:
            // for the first 3 seconds every request fails; afterwards the
            // service operates normally.
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
