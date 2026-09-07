// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Injects a sample I/O driver after a fixed two-thread runtime has started.

mod runtime;
mod sample_driver;
mod system_tasks;

use std::error::Error;

use runtime::Runtime;
use sample_driver::{SampleContext, created_driver_count, shutdown_driver_count};

fn main() -> Result<(), Box<dyn Error>> {
    let runtime = Runtime::start()?;
    println!("runtime started with {} workers", Runtime::WORKER_COUNT);

    {
        let context = runtime.get_context::<SampleContext>();
        assert_eq!(created_driver_count(), Runtime::WORKER_COUNT);

        let same_context = runtime.get_context::<SampleContext>();
        assert_eq!(context, same_context);
        assert_eq!(created_driver_count(), Runtime::WORKER_COUNT);
        println!("sample I/O context uses driver on {:?}", context.driver_thread());
    }

    runtime.shutdown()?;
    assert_eq!(shutdown_driver_count(), Runtime::WORKER_COUNT);
    Ok(())
}
