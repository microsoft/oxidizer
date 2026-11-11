// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Test-only runtime helpers.

use std::task::Waker;

use performables::arc::Arc;
use thread_aware::Thread;

use crate::runtime::config::RuntimeName;
use crate::runtime::dispatch::{DispatcherClient, DispatcherCore, WorkerEndpoint};
use crate::runtime::shutdown::Shutdown;
use crate::runtime::worker::asynchronous::{CommandSender, command_channel};
use crate::runtime::worker::blocking::{Pool, Worker};

impl DispatcherClient {
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn for_test(thread: &Thread) -> Self {
        let (commands, _command_rx) = command_channel();
        let shutdown = Shutdown::default();
        let endpoint = worker_endpoint(
            commands,
            Waker::noop().clone(),
            thread.clone(),
            Worker::new(Pool::new(RuntimeName::default(), None, None), shutdown.clone()),
        );
        Self::new(DispatcherCore::new(nonempty::NonEmpty::new(endpoint), shutdown))
    }
}

pub(crate) fn worker_endpoint(commands: CommandSender, waker: Waker, thread: Thread, blocking_worker: Arc<Worker>) -> WorkerEndpoint {
    WorkerEndpoint {
        commands,
        waker,
        thread,
        blocking_worker,
    }
}
