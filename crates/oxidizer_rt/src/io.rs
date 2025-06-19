// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::pin;
use std::rc::Weak;
use std::thread;
use std::time::Duration;

use futures::StreamExt;
use oxidizer_io::{Runtime, SystemTaskCategory};

use crate::{LocalTaskMeta, LocalTaskScheduler, SpawnQueue, ThreadWaker};

//TODO: As a performance optimization, we should not compile ThreadWaker on windows release builds.
/// This facade is used for waking up any associated waiters.
///
/// This is used to abstract over the different waker types used in the runtime.
/// The `ThreadWaker` is used on platforms not supported by `oxidizer_io` and for testing.
/// The `IoWaker` is used on platforms supported by `oxidizer_io`.
#[derive(Debug, Clone)]
pub enum WakerFacade {
    ThreadWaker(ThreadWaker),
    IoWaker(oxidizer_io::Waker),
}

impl From<ThreadWaker> for WakerFacade {
    fn from(waker: ThreadWaker) -> Self {
        Self::ThreadWaker(waker)
    }
}

impl From<oxidizer_io::Waker> for WakerFacade {
    fn from(waker: oxidizer_io::Waker) -> Self {
        Self::IoWaker(waker)
    }
}

impl WakerFacade {
    #[cfg_attr(test, mutants::skip)]
    pub fn notify(&self) {
        match self {
            Self::ThreadWaker(waker) => waker.notify(),
            Self::IoWaker(waker) => waker.wake(),
        }
    }
}

/// This facade is used for waiting on any associated wakers.
///
/// This is used to abstract over the different waker types used in the runtime.
/// The `ThreadWaker` is used on platforms not supported by `oxidizer_io` and for testing.
/// The `SharedIoWaker` is used on platforms supported by `oxidizer_io`.
//TODO: As a performance optimization, we should not compile ThreadWaker on windows release builds.
#[derive(Debug)]
pub enum WakerWaiterFacade {
    ThreadWaker(ThreadWaker),
    SharedIoWaker(oxidizer_io::Driver),
}

impl WakerWaiterFacade {
    #[cfg_attr(test, mutants::skip)]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "truncation would only happen with crazy-high values in which case we would have bigger problems already"
    )]
    pub fn wait(&mut self, timeout: Duration) {
        match self {
            Self::ThreadWaker(waker) => waker.wait(timeout),
            Self::SharedIoWaker(driver) => {
                driver.process_completions(timeout.as_millis() as _);
            }
        }
    }

    #[cfg_attr(test, mutants::skip)] // This just passes through to the driver, it's tested there.
    pub fn is_inert(&self) -> bool {
        match self {
            Self::ThreadWaker(_) => true,
            Self::SharedIoWaker(driver) => driver.is_inert(),
        }
    }
}

impl From<ThreadWaker> for WakerWaiterFacade {
    fn from(waker: ThreadWaker) -> Self {
        Self::ThreadWaker(waker)
    }
}

impl From<oxidizer_io::Driver> for WakerWaiterFacade {
    fn from(driver: oxidizer_io::Driver) -> Self {
        Self::SharedIoWaker(driver)
    }
}

/// IO Dispatch serves as a implementation of `oxidizer_io::Runtime`.
/// It is used to dispatch IO tasks to the system worker and local tasks to the local task scheduler.
///
/// This is seperated from the dispatcher because of dependency cycles during initialization.
/// The IO system doesn't need the full dispatcher so we seperate it out.
#[derive(Debug)]
pub struct IoDispatch {
    spawn_async_tx: async_channel::Sender<oxidizer_io::AsyncTask>,
}

impl IoDispatch {
    #[expect(
        clippy::allow_attributes,
        reason = "conditional - only fails lint in some build configurations"
    )]
    #[allow(dead_code, reason = "platform-specific (at least for now)")]
    pub fn new(spawn_local: Weak<SpawnQueue>) -> Self {
        let local_scheduler = LocalTaskScheduler::new(Weak::clone(&spawn_local));

        let (spawn_async_tx, spawn_async_rx) = async_channel::unbounded::<oxidizer_io::AsyncTask>();

        // We start a shoveler task here to take tasks from the channel and put them into the
        // local scheduler. This allows the scheduler to be thread-local while the originators
        // of the tasks may be anywhere, as required by the `Runtime` trait API contract.
        local_scheduler.spawn(async move || {
            let local_scheduler = LocalTaskScheduler::new(spawn_local);

            let mut spawn_async_rx = pin!(spawn_async_rx);

            while let Some(body) = spawn_async_rx.next().await {
                let meta = LocalTaskMeta::builder().name("IO Async Task").build();

                let body = Box::into_pin(body);
                local_scheduler.spawn_with_meta(meta, move || body);
            }
        });

        Self { spawn_async_tx }
    }
}

impl Runtime for IoDispatch {
    #[cfg_attr(test, mutants::skip)]
    fn enqueue_system_task(
        &self,
        _category: SystemTaskCategory,
        body: Box<dyn FnOnce() + Send + 'static>,
    ) {
        // Hilarious workaround until we have ability to spawn proper
        // system tasks in a thread-safe manner.
        thread::spawn(move || {
            body();
        });
    }

    #[cfg_attr(test, mutants::skip)]
    fn enqueue_task(&self, body: oxidizer_io::AsyncTask) {
        self.spawn_async_tx
            .send_blocking(body)
            .expect("this should never happen - the shoveler only stops when we close the channel");
    }
}