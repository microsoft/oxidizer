// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Example of implementing a custom spawner using std::thread.

use wing::Spawner;
use std::thread;

/// A spawner that uses std::thread to execute futures in a blocking manner.
///
/// This demonstrates how to integrate with non-async runtimes.
#[derive(Clone, Copy)]
struct ThreadSpawner;

impl Spawner for ThreadSpawner {
    fn spawn<T>(&self, work: impl Future<Output = T> + Send + 'static) -> T
    where
        T: Send + 'static,
    {
        let (sender, receiver) = std::sync::mpsc::channel();

        thread::spawn(move || {
            // Block on the future using a simple executor
            let result = futures::executor::block_on(work);
            let _ = sender.send(result);
        });

        receiver.recv().expect("thread panicked or disconnected")
    }
}

#[tokio::main]
async fn main() {
    let spawner = ThreadSpawner;

    println!("Spawning task on std::thread...");
    let result = spawner.spawn(async {
        println!("Task running on thread!");
        std::thread::sleep(std::time::Duration::from_millis(100));
        42
    });

    println!("Result: {result}");
}
