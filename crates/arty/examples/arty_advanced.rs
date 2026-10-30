// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Creating a runtime, limiting workers, and keeping child tasks on their parent's worker.

use std::time::Duration;

use arty::runtime::{Runtime, WorkersPolicy};

fn main() -> Result<(), ohno::AppError> {
    let runtime = Runtime::builder().workers(WorkersPolicy::at_most(2)).build()?;
    let answer = runtime.scheduler().block_on(async |cx| {
        cx.clock().delay(Duration::from_millis(1)).await;
        // This scheduler runs the child on the same worker as its parent.
        cx.scheduler().spawn(async |_| 42).await
    })??;
    println!("{answer}");
    Ok(())
}
