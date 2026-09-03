// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Construction helpers for runtime thread coordinates.

use std::thread::ThreadId;

use thread_aware_core::__private::v1::{new_numa_node, new_owner, new_thread};

use crate::{NumaNode, Owner, Thread};

/// Builds [`Thread`] coordinates that belong to one runtime.
///
/// A new builder creates a unique [`Owner`]. Cloning the builder preserves that
/// owner, allowing a runtime to construct coordinates for all of its worker
/// threads while selecting each worker's nearest NUMA node.
///
/// The default NUMA coordinate is node `0`, used as a topology-agnostic
/// single-node fallback. It does not assert that every worker is physically on
/// hardware node zero. A runtime that knows its topology should call
/// [`with_numa_node`](Self::with_numa_node) for each worker before building its coordinate.
#[derive(Clone, Debug)]
pub struct ThreadBuilder {
    owner: Owner,
    numa_node: NumaNode,
}

impl ThreadBuilder {
    /// Selects the NUMA node nearest to the thread being built, overriding the
    /// topology-agnostic node-zero fallback.
    #[must_use]
    pub fn with_numa_node(mut self, numa_node: u32) -> Self {
        self.numa_node = new_numa_node(numa_node);
        self
    }

    /// Builds a thread coordinate for `thread_id`.
    ///
    /// The ID must belong to a live worker thread. Rust may reuse a [`ThreadId`]
    /// after its thread exits, which could alias an existing per-thread partition.
    /// Runtimes should normally call this method with
    /// `std::thread::current().id()` from the worker being described.
    #[must_use]
    pub fn build(&self, thread_id: ThreadId) -> Thread {
        new_thread(self.owner.clone(), thread_id, self.numa_node.clone())
    }
}

impl Default for ThreadBuilder {
    fn default() -> Self {
        Self {
            owner: new_owner(),
            numa_node: new_numa_node(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::ThreadBuilder;

    #[test]
    fn clones_keep_owner_and_allow_distinct_numa_nodes() {
        let builder = ThreadBuilder::default();
        let first = builder.clone().with_numa_node(1).build(thread::current().id());
        let second = builder.with_numa_node(2).build(thread::current().id());

        assert_eq!(first.owner(), second.owner());
        assert_ne!(first.numa_node(), second.numa_node());
    }

    #[test]
    fn defaults_create_distinct_owners() {
        let first = ThreadBuilder::default().build(thread::current().id());
        let second = ThreadBuilder::default().build(thread::current().id());

        assert_ne!(first.owner(), second.owner());
    }
}
