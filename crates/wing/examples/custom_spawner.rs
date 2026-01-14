// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Example of implementing a custom spawner using std::thread.

use std::thread;
use wing::Spawner;

/// A spawner that uses std::thread to execute futures.
///
/// This demonstrates how to integrate with non-async runtimes.
#[derive(Clone, Copy)]
struct ThreadSpawner;

impl Spawner for ThreadSpawner {
    fn spawn<T>(&self, work: T)
    where
        T: Future<Output = ()> + Send + 'static,
    {
        thread::spawn(move || {
            futures::executor::block_on(work);
        });
    }
}

fn main() {
    let spawner = ThreadSpawner;
    let (sender, receiver) = std::sync::mpsc::channel();

    println!("Spawning task on std::thread...");
    spawner.spawn(async move {
        println!("Task running on thread!");
        std::thread::sleep(std::time::Duration::from_millis(100));
        sender.send(42).unwrap();
    });

    let result = receiver.recv().expect("thread panicked or disconnected");
    println!("Result: {result}");
}
