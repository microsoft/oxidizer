// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates lazy driver registration and peer discovery on one runtime worker.
//!
//! The sample drivers do not perform I/O; their work tracker is a no-op.

#[path = "../../tests/support/coordinator.rs"]
mod coordinator;
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
