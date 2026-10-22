// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Initialize one I/O driver on every runtime worker.

use std::task::Waker;

use arty::task::{Builtins, JoinError};
use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, DriverRole, IoContext};
use thread_aware::ThreadAware;

#[derive(Clone, ThreadAware)]
struct ExampleContext;

impl IoContext for ExampleContext {
    type ContextOptions = ();
    type Driver = ExampleDriver;

    fn context((): Self::ContextOptions) -> Self {
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
async fn main(builtins: Builtins) -> Result<(), JoinError> {
    let tasks: Vec<_> = builtins
        .operations()
        .workers()
        .into_iter()
        .map(|worker| {
            builtins.scheduler().spawn_at(worker, builtins.clone(), |worker| async move {
                let _ = worker.io().context::<ExampleContext>(&());
            })
        })
        .collect();

    for task in tasks {
        task.await?;
    }
    Ok(())
}
