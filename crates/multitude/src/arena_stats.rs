// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Runtime statistics for an [`Arena`](crate::Arena).
///
/// All fields are lifetime counters: they accumulate over the life of
/// the arena and never decrease. A zero-cost snapshot is returned by
/// [`Arena::stats`](crate::Arena::stats).
///
/// The fields are `pub` because this is a value-semantic data type; the
/// arena owns the running counters internally and hands you a copy.
#[cfg_attr(docsrs, doc(cfg(feature = "stats")))]
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct ArenaStats {
    /// Total normal-size local chunks ever allocated by this arena.
    ///
    /// Local chunks back simple references and `Local`-flavor smart
    /// pointers (`Arc`, `Box`).
    pub normal_local_chunks_allocated: u64,

    /// Total oversized stand-alone local chunks ever allocated by
    /// this arena.
    ///
    /// Oversized chunks hold a single allocation that
    /// exceeded `max_normal_alloc`; they are never cached.
    pub oversized_local_chunks_allocated: u64,

    /// Total normal-size shared chunks ever allocated by this arena.
    ///
    /// Shared chunks back `Arc`-flavor smart pointers.
    pub normal_shared_chunks_allocated: u64,

    /// Total oversized stand-alone shared chunks ever allocated by
    /// this arena.
    ///
    /// See `oversized_local_chunks_allocated` for the
    /// definition of "oversized".
    pub oversized_shared_chunks_allocated: u64,

    /// Sum of bytes requested by user allocations (i.e., the `size`
    /// field of each successful allocation's `Layout`).
    ///
    /// Excludes internal chunk overhead such as headers, alignment padding, and
    /// per-allocation drop-tracking metadata.
    pub total_bytes_allocated: u64,

    /// Bytes "wasted" as unused tail space when a chunk was rotated out
    /// — either by a follow-up allocation (refill) or by [`Arena::reset`](crate::Arena::reset)
    /// retiring its currently-active chunks.
    ///
    /// Does **not** include slack still in current chunks, slack at
    /// chunk teardown (when an `Arc`/`Box` releases the chunk's
    /// last refcount), or fragmentation inside a chunk (multiple
    /// allocations leaving gaps between them).
    pub wasted_tail_bytes: u64,

    /// Number of times a growing collection had to be moved to a
    /// fresh, larger buffer because it could not grow in place.
    ///
    /// Each relocation wastes memory (old buffer abandoned in chunk)
    /// and costs a copy. Pre-sizing collections or using larger chunks
    /// can reduce this.
    pub relocations: u64,
}
