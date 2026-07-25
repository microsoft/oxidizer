// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Runtime statistics for an [`Arena`](crate::Arena).
///
/// Lifetime counters accumulate over the life of the arena. Counters with a
/// `_since_reset` suffix start at zero after each successful
/// [`Arena::reset`](crate::Arena::reset). Live gauges use atomic accounting
/// events around successful backing allocations, chunk reclamation, and cache
/// publication. Escaped thread-safe owners can return chunks concurrently, so
/// fields in one snapshot may reflect adjacent events rather than one globally
/// synchronized state.
#[cfg_attr(docsrs, doc(cfg(feature = "stats")))]
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct ArenaStats {
    /// Total cacheable, power-of-two chunks allocated.
    pub normal_chunks_allocated: u64,

    /// Total single-allocation chunks above `max_normal_alloc`. These chunks
    /// are not cached.
    pub oversized_chunks_allocated: u64,

    /// Total bytes currently held from the underlying allocator.
    ///
    /// This live gauge includes active, retired, and cached chunks plus their
    /// headers and alignment padding.
    pub total_bytes_allocated: u64,

    /// Maximum accounted value reached by
    /// [`total_bytes_allocated`](Self::total_bytes_allocated).
    ///
    /// Successful allocations are accounted when the backing allocator
    /// returns. Storage is removed from accounting after reclamation. Cached
    /// chunks remain allocated and contribute to this lifetime high-water mark.
    pub peak_bytes_allocated: u64,

    /// Number of normal chunks held by or being returned to the reusable
    /// cache.
    ///
    /// A concurrent return is counted before publication so a cache pop cannot
    /// make the gauge underflow. It may therefore briefly include an in-flight
    /// chunk that is not yet available for reuse.
    pub cached_chunks: u64,

    /// Total allocation footprint of chunks held by or being returned to the
    /// reusable cache, including their headers and alignment padding.
    pub cached_bytes: u64,

    /// Number of times a normal chunk has been acquired from the reusable
    /// cache instead of the backing allocator.
    pub normal_chunks_reused: u64,

    /// Number of completed calls to [`Arena::reset`](crate::Arena::reset).
    pub resets: u64,

    /// Unused tail bytes across currently held chunks.
    ///
    /// This live gauge excludes internal alignment gaps between allocations.
    pub wasted_tail_bytes: u64,

    /// Number of growing-collection buffer relocations.
    ///
    /// A relocation copies a collection to a larger buffer when in-place
    /// growth is unavailable.
    pub relocations: u64,

    /// Normal chunks allocated from the backing allocator since the last
    /// successful [`Arena::reset`](crate::Arena::reset).
    pub normal_chunks_allocated_since_reset: u64,

    /// Oversized one-shot chunks allocated since the last successful
    /// [`Arena::reset`](crate::Arena::reset).
    pub oversized_chunks_allocated_since_reset: u64,

    /// Total bytes successfully obtained from the backing allocator since the
    /// last successful [`Arena::reset`](crate::Arena::reset).
    ///
    /// This is a cumulative generation counter, not a live gauge. It includes
    /// bytes that were subsequently released.
    pub backing_bytes_allocated_since_reset: u64,

    /// Normal chunks acquired from the reusable cache since the last
    /// successful [`Arena::reset`](crate::Arena::reset).
    pub normal_chunks_reused_since_reset: u64,

    /// Growing-collection buffer relocations since the last successful
    /// [`Arena::reset`](crate::Arena::reset).
    pub relocations_since_reset: u64,
}
