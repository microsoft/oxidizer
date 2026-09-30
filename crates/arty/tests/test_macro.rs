// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime test attributes preserve asynchronous bodies and return values.

#![cfg(feature = "rt")]
#![cfg(feature = "macros")]

testing_aids::init_tracing!();

use arty::runtime::{Builtins, test};

#[test]
async fn simple_main(cx: Builtins) {
    println!("Hello, world!");
    cx.scheduler()
        .spawn(async move |_| {
            println!("Hello again!");
        })
        .await;
}

#[test]
async fn simple_main_returning(cx: Builtins) -> Result<(), Box<dyn std::error::Error + Send + 'static>> {
    println!("Hello, world!");
    cx.scheduler()
        .spawn(async move |_| {
            println!("Hello again!");
        })
        .await;
    Ok(())
}
