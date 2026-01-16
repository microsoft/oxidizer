// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Spawning tasks with a custom spawner.

use std::time::Duration;

use arty::Spawner;

fn main() {
    // Create a spawner that runs futures on background threads
    let spawner = Spawner::custom(|fut| {
        std::thread::spawn(move || futures::executor::block_on(fut));
    });

    // Fire-and-forget: spawn a task without waiting for its result
    let _ = spawner.spawn(async {
        std::thread::sleep(Duration::from_millis(10));
        println!("Background task completed!");
    });

    // Retrieve a result by awaiting the JoinHandle
    let handle = spawner.spawn(async { 1 + 1 });
    let value = futures::executor::block_on(handle);
    println!("Got result: {value}");

    // Wait for background task
    std::thread::sleep(Duration::from_millis(50));
}
