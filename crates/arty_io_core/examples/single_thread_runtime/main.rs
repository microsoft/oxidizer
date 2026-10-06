// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates lazy driver registration and waiting roles on one runtime worker.
//!
//! The sample drivers do not perform I/O and return no-op wakers.
//! Cycle failures stop the worker after shutting down its drivers. Shutdown joins the
//! worker and returns the original cycle error, reporting any cleanup error separately.

mod drivers;
mod runtime;

use std::error::Error;

use drivers::{EchoContext, SampleContext};
use runtime::Runtime;

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let runtime = Runtime::start();
    println!("runtime started");

    let registration = runtime
        .get_context::<SampleContext>()
        .and_then(|sample| runtime.get_context::<EchoContext>().map(|echo| (sample, echo)));

    let shutdown = runtime.shutdown();
    let (_sample, _echo) = registration?;
    shutdown?;
    println!("runtime shutdown complete");
    Ok(())
}
