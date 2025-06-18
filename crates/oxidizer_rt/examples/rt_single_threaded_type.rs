// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Showcases how single-threaded types can be shared between tasks.

use std::rc::Rc;

use oxidizer_rt::{BasicThreadState, main};

#[main]
async fn main(cx: BasicThreadState) {
    // Rc is a single-threaded type, so cannot be shared between threads.
    let my_pin_code = Rc::new(1234);

    println!("Main task sees: {my_pin_code}");

    let shared_pin_code = Rc::clone(&my_pin_code);

    // Using the local scheduler ensures that the spawned task is on the same thread.
    cx.local_scheduler()
        .spawn(async move || {
            println!("Spawned task sees: {shared_pin_code}");
        })
        .await;
}