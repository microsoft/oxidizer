// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared behavior of arena chunks.

/// A contiguous block of memory that an arena carves bump allocations out of.
///
/// Implemented by [`LocalChunk`](super::local_chunk::LocalChunk) and
/// [`SharedChunk`](super::shared_chunk::SharedChunk). Both are DSTs with a payload tail;
/// local chunks are arena-thread confined, shared chunks use atomic refcounts.
pub(crate) trait Chunk {
    /// Returns the chunk's payload capacity in bytes (i.e. `data.len()`).
    fn capacity(&self) -> usize;

    /// Increments the chunk's reference count by one.
    ///
    /// Called whenever a new handle into this chunk's payload is created.
    /// Aborts the process on overflow.
    fn inc_ref(&self);

    /// Decrements the chunk's reference count by one.
    ///
    /// Returns `true` if the count reached zero, signaling that the caller is
    /// responsible for tearing down the chunk (running drop entries and
    /// routing the backing memory back to the provider or deallocator).
    fn dec_ref(&self) -> bool;
}
