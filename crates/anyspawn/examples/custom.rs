// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Spawning tasks with a custom spawner.

use std::thread::sleep;
use std::time::Duration;

use anyspawn::{BoxedFuture, SpawnCustom, Spawner};
use thread_aware::ThreadAware;
use thread_aware::affinity::{MemoryAffinity, PinnedAffinity};

/// A simple spawner that runs futures on background threads.
#[derive(Clone)]
struct ThreadPoolSpawner;

impl ThreadAware for ThreadPoolSpawner {
    fn relocated(&mut self, _: MemoryAffinity, _: PinnedAffinity) {}
}

impl SpawnCustom for ThreadPoolSpawner {
    fn spawn(&self, task: BoxedFuture) {
        std::thread::spawn(move || futures::executor::block_on(task));
    }

    fn spawn_anywhere(&self, task: BoxedFuture) {
        self.spawn(task);
    }
}

#[tokio::main]
async fn main() {
    let spawner = Spawner::new_custom("threadpool", ThreadPoolSpawner);

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
