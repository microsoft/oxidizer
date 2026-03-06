// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Hedging middleware example demonstrating how a slow primary request is hedged
//! with a faster secondary request that completes first.

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
    // Set up tracing subscriber for logs to console
    tracing_subscriber::registry().with(tracing_subscriber::fmt::layer()).init();

    let clock = Clock::new_tokio();
    let context = ResilienceContext::new(&clock).use_logs();

    let op_clock = clock.clone();

    // Configure hedging: if the original request hasn't completed after 200ms,
    // launch a hedging request. The first successful response wins.
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

/// Simulates a service where the first two calls are slow (500ms) and subsequent
/// calls (the second hedging attempt onwards) are fast (50ms). The fast hedging
/// attempt completes before the slow ones, demonstrating how hedging reduces tail latency.
async fn slow_then_fast_operation(input: String, clock: &Clock) -> String {
    let call = CALL_COUNT.fetch_add(1, Ordering::Relaxed);

    if call < 2 {
        // Original request and first hedge: simulate a slow response
        println!("[service] attempt {call}: slow path (500ms)");
        clock.delay(Duration::from_millis(500)).await;
        format!("{input} - slow response")
    } else {
        // Second hedging request: simulate a fast response
        println!("[service] attempt {call}: fast path (50ms)");
        clock.delay(Duration::from_millis(50)).await;
        format!("{input} - fast response")
    }
}
