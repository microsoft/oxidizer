// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Basic tokio usage example.

use arty::Spawner;

#[tokio::main]
async fn main() {
    let spawner = Spawner::Tokio;
    let (tx, rx) = tokio::sync::oneshot::channel();

    // Spawn a task that sends its result through a channel
    println!("Spawning task...");
    spawner.spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        tx.send(42).expect("Failed to send result");
    });

    let result = rx.await.expect("Task panicked or sender dropped");
    println!("Task returned: {result}");
}
