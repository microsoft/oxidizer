// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A macro entry point, a worker-driven delay, and a simple message.

use std::time::Duration;

use arty::task::Context;

#[arty::main]
async fn main(cx: Context) {
    cx.clock().delay(Duration::from_millis(1)).await;
    println!("Hello from Arty!");
}
