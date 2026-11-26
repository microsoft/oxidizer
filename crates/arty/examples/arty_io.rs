// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Initialize one I/O driver on every runtime worker.

use std::task::Waker;

use arty::io::Io;
use arty::task::{Context, JoinError};
use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, DriverRole, IoContext};
use thread_aware::ThreadAware;

#[derive(Clone, ThreadAware)]
struct ExampleContext;

/// An application-facing endpoint that keeps the Arty context internal.
struct ExampleEndpoint {
    context: ExampleContext,
}

impl ExampleEndpoint {
    /// Builds the endpoint from the runtime's I/O access.
    fn new(io: &Io) -> Self {
        Self {
            context: io.context::<ExampleContext>(&()),
        }
    }

    /// Submits work through the endpoint without exposing its context.
    fn submit(&self, request: &str) {
        println!("submitted {request}");
        _ = &self.context;
    }
}

impl IoContext for ExampleContext {
    type ContextOptions = ();
    type Driver = ExampleDriver;

    fn new((): Self::ContextOptions) -> Self {
        Self
    }

    fn driver(&mut self, options: DriverOptions) -> Result<Self::Driver, DriverError> {
        if options.role() != DriverRole::Primary {
            return Err(DriverError::from_message("example driver requires the primary role"));
        }
        println!("initialized driver on {:?}", options.thread().id());
        Ok(ExampleDriver)
    }
}

struct ExampleDriver;

impl Driver for ExampleDriver {
    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }

    fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
        Ok(())
    }

    fn shutdown(self) -> Result<(), DriverError> {
        Ok(())
    }
}

#[arty::main(workers = 2)]
async fn main(cx: Context) -> Result<(), JoinError> {
    let tasks: Vec<_> = cx
        .control()
        .workers()
        .map(|worker| {
            cx.scheduler().spawn_at(&worker, cx.clone(), |worker| async move {
                ExampleEndpoint::new(worker.io()).submit("request");
            })
        })
        .collect();

    for task in tasks {
        task.await?;
    }
    Ok(())
}
