// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Spawning tasks with a custom spawner.

use std::thread::{sleep, spawn};
use std::time::Duration;

use anyspawn::{BoxedFuture, CustomSpawnerBuilder};
use futures::executor::block_on;

#[tokio::main]
async fn main() {
    // Create a spawner that runs futures on background threads
    let spawner = CustomSpawnerBuilder::custom("threadpool", |fut: BoxedFuture| {
        spawn(move || block_on(fut));
    })
    .build();

    // Fire-and-forget: spawn a task without waiting for its result
    let () = spawner
        .spawn(async {
            println!("Background task completed!");
        })
        .await;

    // Retrieve a result by awaiting the JoinHandle
    let handle = spawner.spawn(async { 1 + 1 });
    let value = handle.await;
    println!("Got result: {value}");

    // Wait for background task
    sleep(Duration::from_millis(50));
}
