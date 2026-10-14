// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;

use observed::Sink;

use crate::{Executor, ExecutorCore};

/// Builds an instance of [`Executor`].
#[derive(Debug)]
#[must_use]
pub struct ExecutorBuilder {
    owner_waker: Waker,
    sink: Sink,
}

impl ExecutorBuilder {
    /// Creates a new [`ExecutorBuilder`] with default settings.
    pub fn new() -> Self {
        Self {
            owner_waker: Waker::noop().clone(),
            sink: Sink::noop(),
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

    /// Builds the executor with the configured settings.
    #[must_use]
    pub fn build(self) -> Executor {
        Executor::new(ExecutorCore::new(self.owner_waker, self.sink))
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
