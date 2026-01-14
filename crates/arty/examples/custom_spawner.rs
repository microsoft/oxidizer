// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Example of using a custom spawner with std::thread.

use arty::Spawner;

fn main() {
    let spawner = Spawner::new_custom(|fut| {
        std::thread::spawn(move || futures::executor::block_on(fut));
    });

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
