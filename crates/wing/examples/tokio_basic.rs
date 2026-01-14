// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Basic tokio usage example.

use wing::tokio::TokioSpawner;
use wing::Spawner;

#[tokio::main]
async fn main() {
    let spawner = TokioSpawner;
    let (tx, rx) = tokio::sync::oneshot::channel();

    // Spawn a task that sends its result through a channel
    println!("Spawning task...");
    spawner.spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        tx.send(42).unwrap();
    });

    let result = rx.await.unwrap();
    println!("Task returned: {result}");
}
