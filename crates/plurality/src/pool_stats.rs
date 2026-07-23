// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Runtime statistics for a [`Pool`](crate::Pool).
///
/// Returned by [`Pool::stats`](crate::Pool::stats) under the `stats` feature.
/// Chunks are retained until pool teardown, so both counters are monotonic and
/// also describe the pool's current allocation.
#[cfg_attr(docsrs, doc(cfg(feature = "stats")))]
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct PoolStats {
    /// Total chunks this pool has allocated from the underlying allocator.
    ///
    /// Chunks currently held, which is also the lifetime total.
    pub total_chunks_allocated: u64,

    /// Total bytes this pool has allocated from the underlying allocator.
    ///
    /// Includes chunk headers, slots, and alignment padding.
    pub total_bytes_allocated: u64,
}
