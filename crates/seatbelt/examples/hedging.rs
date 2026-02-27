// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Hedging middleware example demonstrating how a slow primary request is hedged
//! with a faster secondary request that completes first.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use layered::{Execute, Service, Stack};
use seatbelt::hedging::Hedging;
use seatbelt::hedging::HedgingMode;
use seatbelt::{RecoveryInfo, ResilienceContext};
use tick::Clock;

static CALL_COUNT: AtomicU32 = AtomicU32::new(0);

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let context = ResilienceContext::new(&clock);

    // Configure hedging: if the original request hasn't completed after 200ms,
    // launch a hedge request. The first successful response wins.
    let stack = (
        Hedging::layer("my_hedge", &context)
            .clone_input()
            .recovery_with(|_output: &String, _args| RecoveryInfo::never())
            .hedging_mode(HedgingMode::delay(Duration::from_millis(200)))
            .on_hedge(|args| {
                println!(
                    "[hedge] launching attempt {} (last: {})",
                    args.attempt().index(),
                    args.attempt().is_last()
                );
            }),
        Execute::new(slow_then_fast_operation),
    );

    let service = stack.into_service();

    println!("[main] sending request...");
    let start = std::time::Instant::now();

    let output = service.execute("hello".to_string()).await;
    println!("[main] result: {output} (took {:?})", start.elapsed());
}

/// Simulates a service where the first call is slow (500ms) and the second
/// call (the hedge) is fast (50ms). The hedge completes before the original,
/// demonstrating how hedging reduces tail latency.
async fn slow_then_fast_operation(input: String) -> String {
    let call = CALL_COUNT.fetch_add(1, Ordering::Relaxed);

    if call == 0 {
        // Original request: simulate a slow response
        println!("[service] attempt 0: slow path (500ms)");
        tokio::time::sleep(Duration::from_millis(500)).await;
        format!("{input} - slow response")
    } else {
        // Hedge request: simulate a fast response
        println!("[service] attempt {call}: fast path (50ms)");
        tokio::time::sleep(Duration::from_millis(50)).await;
        format!("{input} - fast response")
    }
}
