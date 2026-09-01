// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Private construction APIs for thread-aware runtimes, grouped by version.

/// Version 1 of the private construction API.
pub mod v1 {
    #[cfg(any(test, feature = "std"))]
    use std::thread::ThreadId;

    #[cfg(any(test, feature = "std"))]
    use crate::Thread;
    use crate::{NumaNode, Owner};

    /// Creates a unique runtime owner identifier.
    #[must_use]
    pub fn new_owner() -> Owner {
        Owner::new()
    }

    /// Creates a NUMA node identifier.
    #[inline]
    #[must_use]
    pub const fn new_numa_node(node: u32) -> NumaNode {
        NumaNode::new(node)
    }

    /// Creates a thread identifier from its runtime, OS thread, and NUMA node.
    #[cfg(any(test, feature = "std"))]
    #[inline]
    #[must_use]
    pub const fn new_thread(owner: Owner, id: ThreadId, numa_node: NumaNode) -> Thread {
        Thread::new(owner, id, numa_node)
    }
}
