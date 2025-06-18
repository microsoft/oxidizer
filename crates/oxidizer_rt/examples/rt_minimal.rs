// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use oxidizer_rt::{BasicThreadState, main};

#[main]
async fn main(cx: BasicThreadState) {
    println!("Hello, world!");

    cx.local_scheduler()
        .spawn(async move || {
            println!("Hello again!");
        })
        .await;
}