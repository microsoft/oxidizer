// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::{Duration, Instant};

use futures::StreamExt;
use oxidizer_rt::{BasicThreadState, Placement, Runtime, RuntimeBuilder, TaskMeta};
use oxidizer_time::{ClockControl, Delay, PeriodicTimer};

async fn async_main(scenario: &str, cx: BasicThreadState) {
    println!("-----------------------------");
    println!("Scenario: '{scenario}'");
    println!();

    let now = Instant::now();

    // Delays

    cx.scheduler()
        .spawn(async move |cx| {
            println!("Delaying for 2s...");
            Delay::with_clock(cx.clock(), Duration::from_secs(1)).await;
            println!("Delaying for 2s...done");
        })
        .await;

    cx.scheduler()
        .spawn_with_meta(
            TaskMeta::with_placement(Placement::Background),
            async move |cx| {
                println!("Firing periodic timer for 3 times...");

                let periodic_timer = PeriodicTimer::with_clock(cx.clock(), Duration::from_secs(1));
                periodic_timer
                    .take(3)
                    .for_each(async |()| {
                        println!("Periodic timer fired");
                    })
                    .await;

                println!("Firing periodic timer for 3 times...done");
            },
        )
        .await;

    println!(
        "Scenario '{}' took {}ms",
        scenario,
        now.elapsed().as_millis()
    );
    println!();
}

fn main() {
    // Real time
    Runtime::new()
        .expect("Failed to create runtime")
        .run(|cx| async_main("Real Time:", cx));

    // Controlled time
    RuntimeBuilder::new()
        .with_clock(ClockControl::default().auto_advance_timers(true))
        .build()
        .expect("Failed to create runtime")
        .run(|cx| async_main("Controlled Time:", cx));
}