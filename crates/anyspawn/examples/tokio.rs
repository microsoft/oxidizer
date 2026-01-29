// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Spawning tasks with Tokio.

use anyspawn::Spawner;

#[tokio::main]
async fn main() {
    let spawner = Spawner::new_tokio();

    // Fire-and-forget: spawn a task without waiting for its result
    let () = spawner
        .spawn({
            async move {
                println!("Background task completed!");
            }
        })
        .await;

    // Retrieve a result by awaiting the JoinHandle
    let value = spawner.spawn(async { 1 + 1 }).await;
    println!("Got result: {value}");
}
