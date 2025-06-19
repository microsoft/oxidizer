// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use anyhow::Result;
use oxidizer_rt::{BasicThreadState, main};

#[main]
async fn main(cx: BasicThreadState) -> Result<()> {
    println!("Hello, world!");

    cx.scheduler()
        .spawn(async move |_| {
            println!("Hello again!");
        })
        .await;

    Ok(())
}