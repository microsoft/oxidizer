// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Calls Tokio-dependent async code directly from an Arty task.

use std::time::Duration;

use arty::task::Builtins;
use arty_tokio::TokioHandle;

async fn call_tokio_based_library() -> u64 {
    tokio::time::sleep(Duration::from_millis(10)).await;
    42
}

#[arty::main]
async fn main(cx: Builtins) -> Result<(), ohno::AppError> {
    arty_tokio::init(&cx, TokioHandle::default()).await?;

    let value = call_tokio_based_library().await;
    println!("Tokio-based library returned {value}");
    Ok(())
}
