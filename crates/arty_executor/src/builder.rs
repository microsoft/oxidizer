// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;

use observed::Sink;
use thread_aware_core::Thread;

use crate::{Executor, ExecutorCore};

/// Builds an instance of [`Executor`].
#[derive(Debug)]
#[must_use]
pub struct ExecutorBuilder {
    owner_waker: Waker,
    sink: Sink,
    thread: Option<Thread>,
}

impl ExecutorBuilder {
    /// Creates a new [`ExecutorBuilder`] with default settings.
    pub fn new() -> Self {
        Self {
            owner_waker: Waker::noop().clone(),
            sink: Sink::noop(),
            thread: None,
        }
    }

    /// The waker that will wake up the executor's owner.
    ///
    /// The `owner_waker` is used by the executor to wake up its owner when more work has
    /// arrived for the executor and calling [`execute_cycle()`][Executor::execute_cycle]
    /// is desirable.
    ///
    /// The default is not to wake the owner even if there is more work the executor could do.
    pub fn owner_waker(mut self, owner_waker: Waker) -> Self {
        self.owner_waker = owner_waker;
        self
    }

    /// Configures the observed sink used for task lifecycle events.
    ///
    /// The default sink is a no-op.
    pub fn sink(mut self, sink: Sink) -> Self {
        self.sink = sink;
        self
    }

    /// Configures the thread coordinate owned by the executor.
    ///
    /// The coordinate must identify the current OS thread. If omitted, the
    /// executor uses the current OS thread directly.
    pub fn thread(mut self, thread: Thread) -> Self {
        self.thread = Some(thread);
        self
    }

    /// Builds the executor with the configured settings.
    ///
    /// # Panics
    ///
    /// Panics if another executor is already alive on the current thread or if
    /// the configured thread coordinate identifies a different OS thread.
    #[must_use]
    pub fn build(self) -> Executor {
        let current_thread_id = std::thread::current().id();
        let thread_id = self.thread.map_or(current_thread_id, |thread| {
            assert_eq!(
                thread.id(),
                current_thread_id,
                "Executor thread coordinate must identify the current OS thread"
            );
            thread.id()
        });
        Executor::new(ExecutorCore::new(self.owner_waker, self.sink, thread_id))
    }
}

impl Default for ExecutorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_builder() {
        _ = ExecutorBuilder::default();
    }
}
