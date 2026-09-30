// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Explicit runtime ownership with a worker limit and a worker-affine child.

use std::num::NonZeroUsize;
use std::time::Duration;

use arty::runtime::{ProcessorCount, Runtime};

fn main() -> Result<(), arty::runtime::Error> {
    let runtime = Runtime::builder()
        .processor_count(ProcessorCount::at_most(NonZeroUsize::new(2).expect("two is nonzero")))
        .build()?;
    let answer = runtime
        .task_scheduler()
        .spawn(async |cx| {
            cx.clock().delay(Duration::from_millis(1)).await;
            // This scheduler keeps the child on the parent's worker.
            cx.scheduler().spawn(async |_| 42).await
        })
        .wait();
    println!("{answer}");
    Ok(())
}
