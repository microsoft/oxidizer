// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Spawning tasks with Tokio.

use std::time::Duration;

use anyspawn::Spawner;
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let spawner = Spawner::tokio();

    // Fire-and-forget: spawn a task without waiting for its result
    let () = spawner
        .spawn({
            let clock = clock.clone();
            async move {
                clock.delay(Duration::from_millis(10)).await;
                println!("Background task completed!");
            }
        })
        .await;

    // Retrieve a result by awaiting the JoinHandle
    let value = spawner.spawn(async { 1 + 1 }).await;
    println!("Got result: {value}");

    // Wait for background task
    clock.delay(Duration::from_millis(50)).await;
}
