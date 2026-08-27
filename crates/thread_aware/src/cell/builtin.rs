// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::thread::ThreadId;

use thread_aware_core::{NumaNode, Thread};

use crate::cell::Strategy;
use crate::cell::storage::sealed;

/// Defines one strategy partition per thread.
///
/// Threads with the same id map to the same partition. This is the default strategy used by
/// [`Arc`](crate::Arc).
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct PerThread;

impl sealed::Sealed for PerThread {}

impl Strategy for PerThread {
    type Key = ThreadId;

    fn key(thread: &Thread) -> Self::Key {
        thread.id()
    }
}

/// Defines one strategy partition per NUMA node.
///
/// Threads near the same memory map to the same partition.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct PerNumaNode;

impl sealed::Sealed for PerNumaNode {}

impl Strategy for PerNumaNode {
    type Key = NumaNode;

    fn key(thread: &Thread) -> Self::Key {
        thread.numa_node()
    }
}

/// Defines one strategy partition for the entire process.
///
/// All threads map to the same partition.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct PerProcess;

impl sealed::Sealed for PerProcess {}

impl Strategy for PerProcess {
    type Key = ();

    const SINGLE_PARTITION: bool = true;

    fn key(_thread: &Thread) -> Self::Key {}
}
