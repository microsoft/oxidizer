// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use crate::rt::runtime::context::Builtins;
use crate::rt::task::execution::BoxedRemoteFutureFactory;

pub(in crate::rt::runtime) enum AsyncWorkerCommand<TS = Builtins> {
    /// Schedules a new task for execution on this worker, providing the factory function that will
    /// be used to create the future that becomes the body of the task. The factory registers the
    /// future it creates with the worker's task set, which lets the executor store the future
    /// inline instead of behind an extra box.
    ///
    /// Note that these remotely enqueued tasks do not have an output type - it is the
    /// responsibility of the task itself to deliver any outputs to some waiting thread. In
    /// practice, this means there will be two layers of futures: an outer layer responsible for
    /// delivering the output, and an inner layer with the actual user code to execute.
    ///
    /// The task may end up never getting executed if the runtime is shut down before it gets to it.
    EnqueueTask { future_factory: BoxedRemoteFutureFactory<TS> },

    /// Initiates the shutdown process. The worker will stop accepting new tasks and will discard
    /// any tasks that are enqueued after this command is received (e.g. because other threads do
    /// not yet know about the shutdown).
    ///
    /// It is fine to send this command multiple times - duplicates will be ignored.
    Shutdown,
}

impl<TS> fmt::Debug for AsyncWorkerCommand<TS> {
    #[cfg_attr(test, mutants::skip)] // We have no contract to test here - can return anything.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EnqueueTask { .. } => write!(f, "EnqueueTask"),
            Self::Shutdown => write!(f, "Shutdown"),
        }
    }
}
