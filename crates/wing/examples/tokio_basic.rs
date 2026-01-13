// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Basic tokio usage example.

use wing::tokio::TokioSpawner;
use wing::Spawner;

#[tokio::main]
async fn main() {
    let spawner = TokioSpawner;

    // Spawn and wait for result
    println!("Spawning task...");
    let result = spawner.spawn(async {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        42
    });

    println!("Task returned: {result}");
}
