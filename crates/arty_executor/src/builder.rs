// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;

use observed::Sink;
use thread_aware_core::Thread;

use crate::{Executor, PanicPolicy};

/// Builds an instance of [`Executor`].
#[derive(Debug)]
#[must_use]
pub struct ExecutorBuilder {
    owner_waker: Waker,
    sink: Sink,
    thread: Option<Thread>,
    panic_policy: PanicPolicy,
}

impl ExecutorBuilder {
    /// Creates an [`ExecutorBuilder`] with default configuration.
    pub fn new() -> Self {
        Self {
            owner_waker: Waker::noop().clone(),
            sink: Sink::noop(),
            thread: None,
            panic_policy: PanicPolicy::default(),
        }
    }

    /// Sets the waker that notifies the executor owner.
    ///
    /// The executor calls `owner_waker` for task wake notifications and
    /// shutdown requests. Registering a task does not call this waker. The
    /// caller must arrange an [`execute_cycle()`][Executor::execute_cycle]
    /// after it registers a task.
    ///
    /// By default, the executor does not notify the owner.
    pub fn owner_waker(mut self, owner_waker: Waker) -> Self {
        self.owner_waker = owner_waker;
        self
    }

    /// Sets the observed sink for task lifecycle events.
    ///
    /// The default sink is a no-op.
    pub fn sink(mut self, sink: Sink) -> Self {
        self.sink = sink;
        self
    }

    /// Sets the thread coordinate that the executor owns.
    ///
    /// The coordinate must identify the current OS thread. If omitted, the
    /// executor uses the current OS thread directly.
    pub fn thread(mut self, thread: Thread) -> Self {
        self.thread = Some(thread);
        self
    }

    /// Sets how the executor responds to a contained task panic.
    pub fn panic_policy(mut self, policy: PanicPolicy) -> Self {
        self.panic_policy = policy;
        self
    }

    /// Builds the executor with this configuration.
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
        Executor::new(self.owner_waker, self.sink, thread_id, self.panic_policy)
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

    #[test]
    fn panic_policy_is_configurable() {
        assert!(matches!(ExecutorBuilder::new().panic_policy, PanicPolicy::Shutdown));
        assert!(matches!(
            ExecutorBuilder::new().panic_policy(PanicPolicy::Ignore).panic_policy,
            PanicPolicy::Ignore
        ));
    }

    #[test]
    fn configured_thread_must_identify_the_current_thread() {
        let foreign_id = std::thread::spawn(|| std::thread::current().id()).join().unwrap();
        let thread = thread_aware_core::__private::v1::new_thread(
            thread_aware_core::__private::v1::new_owner(),
            foreign_id,
            thread_aware_core::__private::v1::new_numa_node(0),
        );

        let result = std::panic::catch_unwind(|| ExecutorBuilder::new().thread(thread).build());

        result.unwrap_err();
    }

    #[test]
    fn configured_current_thread_builds_the_executor() {
        let thread = thread_aware_core::__private::v1::new_thread(
            thread_aware_core::__private::v1::new_owner(),
            std::thread::current().id(),
            thread_aware_core::__private::v1::new_numa_node(0),
        );

        drop(ExecutorBuilder::new().thread(thread).build());
    }
}
