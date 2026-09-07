// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Injects a sample I/O driver after a fixed two-thread runtime has started.

mod runtime;
mod sample_driver;
mod system_tasks;

use std::error::Error;

use runtime::Runtime;
use sample_driver::{SampleContext, created_driver_count};

fn main() -> Result<(), Box<dyn Error>> {
    let runtime = Runtime::start()?;
    println!("runtime started with {} workers", Runtime::WORKER_COUNT);

    let contexts = runtime.get_context::<SampleContext>();
    let same_contexts = runtime.get_context::<SampleContext>();

    assert_eq!(contexts, same_contexts);
    assert_eq!(created_driver_count(), Runtime::WORKER_COUNT);
    for context in &contexts {
        println!("sample I/O driver installed on {:?}", context.driver_thread());
    }

    runtime.shutdown()?;
    Ok(())
}
