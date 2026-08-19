// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Runtime allocation statistics for a pool.
///
/// Returned by [`Pool::stats`](crate::Pool::stats) and
/// [`MultiPool::stats`](crate::MultiPool::stats) under the `stats` feature.
/// For [`Pool`](crate::Pool), the counters describe that pool. For
/// [`MultiPool`](crate::MultiPool), each counter is summed across every layout
/// pool.
///
/// Chunks are retained until pool teardown, so the counters are monotonic and
/// also describe the current chunk allocation.
#[cfg_attr(docsrs, doc(cfg(feature = "stats")))]
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct PoolStats {
    /// Total chunks allocated for pooled storage.
    ///
    /// For [`MultiPool`](crate::MultiPool), this is the sum of chunks
    /// allocated by every layout pool.
    pub total_chunks_allocated: u64,

    /// Total bytes allocated for pooled chunks.
    ///
    /// For [`MultiPool`](crate::MultiPool), this is the sum of chunk bytes
    /// allocated by every layout pool. Includes chunk headers, slots, and
    /// alignment padding.
    pub total_bytes_allocated: u64,
}
