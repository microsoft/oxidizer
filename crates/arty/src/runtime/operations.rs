// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::runtime::Runtime;
use crate::runtime::dispatch::DispatcherClient;
use crate::task::Builtins;

/// A handle for requesting runtime shutdown.
///
/// Create this handle with `RuntimeOperations::from(&runtime)` or
/// `RuntimeOperations::from(&builtins)`.
/// It can be cloned and used from any thread without keeping the runtime alive
/// or retaining an association with the task's worker.
///
/// [`request_stop`](Self::request_stop) initiates shutdown without waiting.
#[derive(Debug, Clone)]
pub struct RuntimeOperations {
    dispatcher: DispatcherClient,
}

impl RuntimeOperations {
    /// Requests shutdown without blocking the calling thread.
    ///
    /// May be called repeatedly from any thread. Pending tasks are cancelled,
    /// and new submissions receive [`JoinError`](crate::task::JoinError).
    /// Running blocking callbacks are allowed to finish. The owner remains
    /// responsible for waiting for shutdown when it is stopped or dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{Runtime, RuntimeOperations};
    ///
    /// let runtime = Runtime::new()?;
    /// let operations = RuntimeOperations::from(&runtime);
    /// operations.request_stop();
    /// runtime.stop()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[cfg_attr(test, mutants::skip)] // It is impractical to test for "stuff not happening", so mutating this easily leads to timeouts.
    pub fn request_stop(&self) {
        self.dispatcher.stop();
    }
}

impl From<&Builtins> for RuntimeOperations {
    fn from(builtins: &Builtins) -> Self {
        Self {
            dispatcher: builtins.scheduler.dispatcher.clone(),
        }
    }
}

impl From<&Runtime> for RuntimeOperations {
    fn from(runtime: &Runtime) -> Self {
        Self {
            dispatcher: runtime.scheduler().dispatcher.clone(),
        }
    }
}
