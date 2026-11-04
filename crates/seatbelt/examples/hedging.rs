// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example starts hedge requests when the original request is slow.
//! The second hedge is faster and completes first.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use layered::{Execute, Service, Stack};
use seatbelt::hedging::Hedging;
use seatbelt::{RecoveryInfo, ResilienceContext};
use tick::Clock;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

static CALL_COUNT: AtomicU32 = AtomicU32::new(0);

#[tokio::main]
async fn main() {
    // Send tracing logs to the console.
    tracing_subscriber::registry().with(tracing_subscriber::fmt::layer()).init();

    let clock = Clock::new_tokio();
    let context = ResilienceContext::new(&clock).use_logs();

    let op_clock = clock.clone();

    // If the original request does not finish in 200 milliseconds, start a hedge.
    // Return the first successful response.
    let stack = (
        Hedging::layer("my_hedging", &context)
            .clone_input()
            .max_hedged_attempts(4)
            .recovery_with(|_output: &String, _args| RecoveryInfo::never())
            .hedging_delay(Duration::from_millis(200))
            .on_execute(|_input, args| {
                println!(
                    "[execute] launching attempt {} (last: {})",
                    args.attempt().index(),
                    args.attempt().is_last()
                );
            }),
        Execute::new(move |input: String| {
            let clock = op_clock.clone();
            async move { slow_then_fast_operation(input, &clock).await }
        }),
    );

    let service = stack.into_service();

    println!("[main] sending request...");
    let start = std::time::Instant::now();

    let output = service.execute("hello".to_string()).await;
    println!("[main] result: {output} (took {:?})", start.elapsed());
}

/// Simulates two slow calls of 500 milliseconds.
///
/// Later calls take 50 milliseconds. The fast hedge finishes before the slow calls
/// and reduces tail latency.
async fn slow_then_fast_operation(input: String, clock: &Clock) -> String {
    let call = CALL_COUNT.fetch_add(1, Ordering::Relaxed);

    if call < 2 {
        // Make the original request and first hedge slow.
        println!("[service] attempt {call}: slow path (500ms)");
        clock.delay(Duration::from_millis(500)).await;
        format!("{input} - slow response")
    } else {
        // Make the second hedge fast.
        println!("[service] attempt {call}: fast path (50ms)");
        clock.delay(Duration::from_millis(50)).await;
        format!("{input} - fast response")
    }
}
