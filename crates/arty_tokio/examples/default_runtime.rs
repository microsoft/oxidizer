// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Uses the shared Tokio runtime from an Arty application.

use arty::task::Builtins;
use arty_tokio::TokioHandle;

#[arty::main]
async fn main(cx: Builtins) -> Result<(), ohno::AppError> {
    arty_tokio::init(&cx, TokioHandle::default()).await?;

    let value = tokio::spawn(async { 42 }).await?;
    println!("Tokio returned {value}");
    Ok(())
}
