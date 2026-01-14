// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Spawning tasks with Tokio.

use std::time::Duration;

use arty::Spawner;
use tick::Clock;

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let spawner = Spawner::tokio();

    // Fire-and-forget: spawn a task without waiting for its result
    spawner.spawn({
        let clock = clock.clone();
        async move {
            clock.delay(Duration::from_millis(10)).await;
            println!("Background task completed!");
        }
    });

    // Retrieve a result using run
    let value = spawner.run(async { 1 + 1 }).await.unwrap();
    println!("Got result: {value}");

    // Wait for background task
    clock.delay(Duration::from_millis(50)).await;
}
