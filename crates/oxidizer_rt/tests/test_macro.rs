// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(not(miri))] // The runtime talks to the real OS, which Miri cannot do.
#![cfg(feature = "macros")]

use oxidizer_rt::{BasicThreadState, test};

#[test]
async fn simple_main(cx: BasicThreadState) {
    println!("Hello, world!");
    cx.scheduler()
        .spawn(async move |_| {
            println!("Hello again!");
        })
        .await;
}

#[test]
async fn simple_main_returning(
    cx: BasicThreadState,
) -> Result<(), Box<dyn std::error::Error + Send + 'static>> {
    println!("Hello, world!");
    cx.scheduler()
        .spawn(async move |_| {
            println!("Hello again!");
        })
        .await;
    Ok(())
}