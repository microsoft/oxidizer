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
identifier!(AllocationId, "Allocator-provided correlation key for allocation lifecycle events.");
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
    /// Allocator-provided correlation key, which can be a reused address.
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

/// One successful resize, recorded before the old block becomes reusable.
///
/// The actor is the thread in the event envelope. Equal addresses describe an
/// in-place resize. Failed requests do not create a successful resize event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reallocation {
    /// Correlation key of the old allocation, not necessarily a lifetime identity.
    ///
    /// Later events can keep this key or use the new address as an address-based key.
    pub allocation_id: AllocationId,
    /// Logical or native heap identity.
    pub heap_id: HeapId,
    /// Allocator heap classification.
    pub heap_kind: HeapKind,
    /// Address that remains live until this event is recorded.
    pub old_address: Address,
    /// Address of the prepared replacement.
    pub new_address: Address,
    /// Previous requested size in bytes.
    pub old_size: u64,
    /// New requested size in bytes.
    pub new_size: u64,
    /// Requested alignment of both storage ranges, in bytes.
    pub alignment: u64,
}
