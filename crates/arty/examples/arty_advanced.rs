// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Explicit runtime ownership, worker limits, and worker-affine child tasks.

use std::time::Duration;

use arty::runtime::{ProcessorCount, Runtime};

fn main() -> Result<(), ohno::AppError> {
    let runtime = Runtime::builder().processor_count(ProcessorCount::at_most(2)).build()?;
    let answer = runtime
        .scheduler()
        .spawn_anywhere(async |cx| {
            cx.clock().delay(Duration::from_millis(1)).await;
            // This scheduler keeps the child on the parent's worker.
            cx.scheduler().spawn(async |_| 42).await
        })
        .wait()??;
    println!("{answer}");
    Ok(())
}
