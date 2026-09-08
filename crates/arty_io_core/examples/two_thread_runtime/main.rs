// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Injects a sample I/O driver after a fixed two-thread runtime has started.

mod echo_driver;
mod runtime;
mod sample_driver;
mod system_tasks;

use std::error::Error;

use echo_driver::{
    EchoContext, EchoIoError, created_driver_count as echo_created_driver_count, shutdown_driver_count as echo_shutdown_driver_count,
};
use runtime::Runtime;
use sample_driver::{SampleContext, SampleIoError, created_driver_count, shutdown_driver_count};

fn main() -> Result<(), Box<dyn Error>> {
    let runtime = Runtime::start()?;
    println!("runtime started with {} workers", Runtime::WORKER_COUNT);

    let context = runtime.get_context::<SampleContext>();
    assert_eq!(created_driver_count(), Runtime::WORKER_COUNT);

    let same_context = runtime.get_context::<SampleContext>();
    assert_eq!(context, same_context);
    assert_eq!(created_driver_count(), Runtime::WORKER_COUNT);
    println!("sample I/O context uses driver on {:?}", context.driver_thread());

    assert_eq!(context.perform_io(41), Ok(42));
    assert_eq!(context.operation_count(), 1);

    let echo = runtime.get_context::<EchoContext>();
    assert_eq!(echo_created_driver_count(), Runtime::WORKER_COUNT);
    assert_eq!(echo, runtime.get_context::<EchoContext>());
    assert_eq!(echo_created_driver_count(), Runtime::WORKER_COUNT);
    assert_eq!(echo.perform_io("arty"), Ok("ARTY".to_owned()));

    runtime.shutdown()?;
    assert_eq!(shutdown_driver_count(), Runtime::WORKER_COUNT);
    assert_eq!(echo_shutdown_driver_count(), Runtime::WORKER_COUNT);
    println!("runtime shutdown complete");

    assert_eq!(context.perform_io(41), Err(SampleIoError));
    assert_eq!(context.operation_count(), 1);
    assert_eq!(echo.perform_io("arty"), Err(EchoIoError));
    Ok(())
}
