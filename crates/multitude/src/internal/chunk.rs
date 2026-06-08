// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared behavior of arena chunks.

/// A contiguous block of memory that an arena carves bump allocations out of.
///
/// Both [`LocalChunk`](super::LocalChunk) and [`SharedChunk`](super::SharedChunk)
/// implement this trait. They differ in how the chunk and its allocations are
/// owned and shared:
///
/// - `LocalChunk` is used for allocations whose lifetime is tied to the arena
///   itself and never crosses thread boundaries; no synchronization is needed.
/// - `SharedChunk` is used for allocations whose lifetime can outlive the
///   arena (reference-counted handles), and uses atomics for cross-thread
///   refcounting.
///
/// Implementors are dynamically-sized types: the struct ends with a `[u8]`
/// payload that holds the actual bump-allocation buffer.
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

    /// Returns the number of drop entries currently stored at the tail of the
    /// chunk.
    fn drop_entry_count(&self) -> usize;

    /// Sets the number of drop entries currently stored at the tail of the
    /// chunk.
    fn set_drop_entry_count(&self, count: usize);
}
