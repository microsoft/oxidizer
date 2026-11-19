// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Spawning tasks with a Tokio runtime.

use anyspawn::Spawner;

#[tokio::main]
async fn main() -> Result<(), anyspawn::JoinError> {
    let spawner = Spawner::new_tokio(None);

    // Wait for the task and propagate a runtime failure.
    let () = spawner
        .spawn({
            async move {
                println!("Background task completed!");
            }
        })
        .await?;

    // Retrieve a result by awaiting the JoinHandle
    let value = spawner.spawn(async { 1 + 1 }).await?;
    println!("Got result: {value}");
    Ok(())
}
