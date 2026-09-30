// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Explicit runtime ownership, detached submission, and worker-affine continuations.

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
            cx.scheduler().spawn(async |_| 42).await
        })
        .wait();
    println!("{answer}");
    Ok(())
}
