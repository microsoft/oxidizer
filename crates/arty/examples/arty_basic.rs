// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A macro entry point, a worker-driven delay, and a simple message.

use std::time::Duration;

use arty::runtime::Builtins;

#[arty::main]
async fn main(cx: Builtins) {
    cx.clock().delay(Duration::from_millis(1)).await;
    println!("Hello from Arty!");
}
