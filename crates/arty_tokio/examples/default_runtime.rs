// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Starts the default Tokio runtime and submits a Tokio task from Arty.

use arty::task::Builtins;
use arty_tokio::TokioHandle;

#[arty::main]
async fn main(cx: Builtins) -> Result<(), ohno::AppError> {
    arty_tokio::init(&cx, TokioHandle::default()).await?;

    let value = tokio::spawn(async { 42 }).await?;
    println!("Tokio returned {value}");
    Ok(())
}
