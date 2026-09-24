// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Lazily initializes two I/O drivers in a fixed single-thread runtime.

mod drivers;
mod runtime;

use arty_io_core::ShutdownError;
use drivers::{EchoContext, SampleContext};
use runtime::Runtime;

fn main() -> Result<(), ShutdownError> {
    let runtime = Runtime::start();
    println!("runtime started");

    let _sample = runtime.get_context::<SampleContext>();
    let _echo = runtime.get_context::<EchoContext>();

    runtime.shutdown()?;
    println!("runtime shutdown complete");
    Ok(())
}
