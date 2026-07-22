// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Runtime statistics for a [`Pool`](crate::Pool).
///
/// A zero-cost snapshot is returned by [`Pool::stats`](crate::Pool::stats),
/// available under the `stats` crate feature.
///
/// A pool's chunks are all the same fixed size and are freed only when the
/// pool itself is dropped, so both counters are monotonic lifetime totals:
/// they only ever rise as the pool grows to satisfy demand, and never fall
/// while the pool is alive. Consequently each field is simultaneously the
/// amount currently held from the underlying allocator and the total ever
/// allocated over the pool's lifetime.
///
/// The fields are `pub` because this is a value-semantic data type; the pool
/// owns the running counters internally and hands you a copy.
#[cfg_attr(docsrs, doc(cfg(feature = "stats")))]
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct PoolStats {
    /// Total chunks this pool has allocated from the underlying allocator.
    ///
    /// A chunk is the pool's unit of growth: one system allocation holding a
    /// fixed run of slots (see [`Pool::chunk_size`](crate::Pool::chunk_size)).
    /// Chunks are allocated on demand as the pool grows and are freed only at
    /// pool teardown, so this is both the number of chunks currently held and
    /// the total ever allocated.
    pub total_chunks_allocated: u64,

    /// Total bytes this pool has allocated from the underlying allocator.
    ///
    /// The sum of every chunk's full layout size — header, the slot payload,
    /// and any alignment padding — so it reflects the pool's real allocator
    /// footprint rather than the sum of the `T`-sized user values it holds.
    /// Like [`total_chunks_allocated`](Self::total_chunks_allocated), chunks
    /// are freed only at pool teardown, so this is both the bytes currently
    /// held and the total ever allocated.
    pub total_bytes_allocated: u64,
}
