// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Explicit runtime ownership, worker limits, and worker-affine child tasks.

use std::time::Duration;

use arty::runtime::{ProcessorCount, Runtime};

fn main() -> Result<(), arty::runtime::Error> {
    let runtime = Runtime::builder().processor_count(ProcessorCount::at_most(2)).build()?;
    let answer = runtime
        .task_scheduler()
        .spawn(async |cx| {
            cx.clock().delay(Duration::from_millis(1)).await;
            // This scheduler keeps the child on the parent's worker.
            cx.scheduler()
                .spawn(async |_| 42)
                .await
                .expect("the child task completes before its parent returns")
        })
        .wait()
        .expect("the task completes before runtime shutdown");
    println!("{answer}");
    Ok(())
}
