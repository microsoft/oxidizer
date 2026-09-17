// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Working coordination reference with two owner workers and one shared system-work thread.
//!
//! This is an IN-MEMORY NATIVE-ADAPTER SIMULATION, not production IOCP or `io_uring`. Each owner
//! runs a single collector alongside independent record-delivery and private-queue drivers.
//! Submitting publishes native-shaped activity; only collection followed by driver service
//! completes operations. There are no per-driver threads or per-operation producer threads.
//!
//! The external caller uses blocking result handles for a compact demonstration; owner workers
//! never use those handles. This is a completion coordinator, not an application-future executor.
//! Thread-aware coordinates describe placement, not OS affinity. Moving a context never rebinds
//! an operation. Contexts may outlive runtime shutdown and remain usable, closed handles.
//!
//! Registration is lazy and transactional across both workers. The coordinator retains unique
//! source wakers, budget continuations and deadlines, collects even while runnable, and arms and
//! rechecks sources and control work before entering the one latched domain wait. Shutdown starts
//! all drains before driving any, keeps the collector and shared offload facility alive, and
//! reports failure or an overall timeout without invalidating callback-owned storage.
//! Retained owner threads and accepted cleanup keep that same offload thread available after a
//! controller timeout. Inert task handles and retained consumer contexts do not prolong draining.

#![forbid(unsafe_code)]

mod coordinator;
mod echo_driver;
mod native;
mod runtime;
mod sample_driver;
mod system_tasks;

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod contract_tests;

use std::error::Error;

use echo_driver::{EchoContext, EchoIoError};
use runtime::Runtime;
use sample_driver::{SampleContext, SampleIoError};

fn main() -> Result<(), Box<dyn Error>> {
    let runtime = Runtime::start()?;
    let sample = runtime.get_context::<SampleContext>()?;
    let echo = runtime.get_context::<EchoContext>()?;
    assert_eq!(sample, runtime.get_context::<SampleContext>()?);
    assert_eq!(echo, runtime.get_context::<EchoContext>()?);

    for worker in 0..Runtime::WORKER_COUNT {
        let sample = runtime.get_context_on::<SampleContext>(worker)?;
        let echo = runtime.get_context_on::<EchoContext>(worker)?;
        assert_eq!(sample.driver_thread(), echo.driver_thread());
        let number = sample.submit(41)?;
        let text = echo.submit("arty")?;
        assert_eq!(number.wait()?, 42);
        assert_eq!(text.wait()?, "ARTY");
        assert_eq!(sample.operation_count(), 1);
        println!("both completion models progressed on owner {:?}", sample.driver_thread());
    }

    runtime.shutdown()?;
    assert!(matches!(sample.submit(0), Err(SampleIoError::Closed)));
    assert!(matches!(echo.submit("closed"), Err(EchoIoError::Closed)));
    println!("all drivers drained; retained contexts are closed");
    Ok(())
}
