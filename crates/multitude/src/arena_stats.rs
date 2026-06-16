// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Runtime statistics for an [`Arena`](crate::Arena).
///
/// Most fields are lifetime counters that accumulate over the life of
/// the arena. The exceptions are [`total_bytes_allocated`](Self::total_bytes_allocated)
/// and [`wasted_tail_bytes`](Self::wasted_tail_bytes), which are *live*
/// gauges reflecting current state. A zero-cost snapshot is returned by
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

    /// Total bytes currently held from the underlying allocator.
    ///
    /// The sum of every chunk (header + payload) the arena owns right now —
    /// active `current_*` chunks, retired chunks still kept alive (e.g. by
    /// outstanding `Arc`/`Box` handles), and chunks parked in the size-class
    /// cache.
    ///
    /// This is a **live gauge**, not a lifetime counter: it rises when a
    /// chunk is allocated from the underlying allocator and falls when a
    /// chunk is freed back to it. It includes internal chunk overhead
    /// (headers and alignment padding), so it reflects real allocator
    /// footprint rather than the sum of user-requested `Layout::size()`
    /// bytes.
    pub total_bytes_allocated: u64,

    /// Bytes "wasted" as unused tail space across the arena's chunks.
    ///
    /// The free region between the bump cursor and the drop-entry top, summed
    /// across every chunk the arena currently holds — both the active
    /// `current_local` / `current_shared` chunks and any chunks that have been
    /// retired but not yet returned to the cache or freed back to the underlying
    /// allocator (e.g. chunks held alive by outstanding `Arc`/`Box` handles).
    ///
    /// Bumped up when a chunk is retired from a current slot, bumped
    /// back down when the same chunk is later released to the size-
    /// class cache or returned to the underlying allocator. The
    /// active-chunks contribution is computed on demand at
    /// [`Arena::stats`](crate::Arena::stats) time.
    ///
    /// Does **not** include fragmentation inside a chunk (multiple
    /// allocations leaving gaps between them).
    pub wasted_tail_bytes: u64,

    /// Number of growing-collection buffer relocations.
    ///
    /// Counts how many times a growing collection had to be moved to a fresh,
    /// larger buffer because it could not grow in place.
    ///
    /// Each relocation wastes memory (old buffer abandoned in chunk)
    /// and costs a copy. Pre-sizing collections or using larger chunks
    /// can reduce this.
    pub relocations: u64,
}
