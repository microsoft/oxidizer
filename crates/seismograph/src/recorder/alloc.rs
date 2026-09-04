// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation event types.

use super::event::Address;

macro_rules! identifier {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(transparent)]
        pub struct $name(u64);

        impl $name {
            /// Creates an identifier from its stable numeric representation.
            #[must_use]
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            /// Returns the stable numeric representation.
            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }
        }
    };
}

identifier!(EventThreadId, "Allocator-defined identity of a thread performing an operation.");
identifier!(AllocationId, "Identity shared by one allocation and its deallocation.");
identifier!(HeapId, "Identity assigned to an allocator heap.");

/// Allocator heap classification attached to allocation events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum HeapKind {
    /// A general-purpose heap.
    General,
    /// A bump heap.
    Bump,
    /// A thread-targeted heap.
    Thread,
}

/// Allocator-specific fields attached to an allocation or deallocation event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Allocation {
    /// Stable allocation identity shared by the allocation and deallocation events.
    pub allocation_id: AllocationId,
    /// Allocator-defined identity of the thread performing the operation.
    pub event_thread_id: EventThreadId,
    /// Logical or native heap identity.
    pub heap_id: HeapId,
    /// Heap classification.
    pub heap_kind: HeapKind,
    /// Whether a bump heap was released before this deallocation.
    pub freed_after_heap_release: bool,
    /// Allocation address.
    pub address: Address,
    /// Requested size in bytes.
    pub size: u64,
    /// Requested alignment in bytes.
    pub alignment: u64,
}
