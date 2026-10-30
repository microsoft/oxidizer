// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Uses an application-owned Tokio runtime from an Arty task.

use arty::task::Context;
use tokio::runtime::Builder;

#[arty::main]
async fn main(cx: Context) -> Result<(), ohno::AppError> {
    let tokio = Builder::new_multi_thread().enable_all().build()?;
    arty_tokio::init(&cx, tokio.handle()).await?;

    let value = tokio::spawn(async { 42 }).await?;
    println!("Tokio returned {value}");
    Ok(())
}
