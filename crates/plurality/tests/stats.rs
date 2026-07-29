// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![allow(clippy::allow_attributes, clippy::unwrap_used, reason = "test code")]

//! Tests for the `stats` feature: `PoolStats` and `Pool::stats`.

#[cfg(feature = "stats")]
mod stats_tests {
    use plurality::{Pool, PoolStats};

    /// A brand-new pool has allocated nothing yet.
    #[test]
    fn fresh_pool_reports_zero() {
        let pool = Pool::<u64>::new();
        assert_eq!(pool.stats(), PoolStats::default());
        assert_eq!(pool.stats().total_chunks_allocated, 0);
        assert_eq!(pool.stats().total_bytes_allocated, 0);
    }

    /// The first allocation lazily allocates exactly one chunk.
    #[test]
    fn first_alloc_allocates_one_chunk() {
        let pool = Pool::<u64>::builder().chunk_size(2).build();
        let _held = pool.alloc_box(1_u64);

        let stats = pool.stats();
        assert_eq!(stats.total_chunks_allocated, 1);
        assert_eq!(stats.total_chunks_allocated, u64::from(pool.chunks_allocated()));
        // A chunk holds a fixed run of slots, so its footprint is at least the
        // raw value payload; padding/header/refcount push it higher.
        assert!(stats.total_bytes_allocated >= 2 * size_of::<u64>() as u64);
    }

    /// Chunks are uniform, so growth scales the byte total linearly.
    #[test]
    fn growth_scales_bytes_linearly() {
        let pool = Pool::<u64>::builder().chunk_size(2).build();

        // Fill the first chunk (2 slots).
        let _a = pool.alloc_box(1_u64);
        let _b = pool.alloc_box(2_u64);
        let one_chunk = pool.stats();
        assert_eq!(one_chunk.total_chunks_allocated, 1);
        let bytes_per_chunk = one_chunk.total_bytes_allocated;
        assert!(bytes_per_chunk > 0);

        // The third allocation forces a second chunk.
        let _c = pool.alloc_box(3_u64);
        let two_chunks = pool.stats();
        assert_eq!(two_chunks.total_chunks_allocated, 2);
        assert_eq!(two_chunks.total_bytes_allocated, 2 * bytes_per_chunk);
        assert_eq!(two_chunks.total_chunks_allocated, u64::from(pool.chunks_allocated()));
    }

    /// Freeing handles returns slots to the pool but keeps the chunks, so the
    /// lifetime totals never regress.
    #[test]
    fn freeing_slots_does_not_shrink_stats() {
        let pool = Pool::<u64>::builder().chunk_size(2).build();

        let a = pool.alloc_box(1_u64);
        let b = pool.alloc_box(2_u64);
        let c = pool.alloc_box(3_u64); // forces a second chunk
        let grown = pool.stats();
        assert_eq!(grown.total_chunks_allocated, 2);

        drop((a, b, c));
        // Chunks are freed only at pool teardown, so the snapshot is unchanged.
        assert_eq!(pool.stats(), grown);

        // Reusing the freed slots allocates no new chunk.
        let _reused = pool.alloc_box(4_u64);
        assert_eq!(pool.stats(), grown);
    }

    /// `PoolStats` is a plain value-semantic snapshot.
    #[test]
    fn stats_is_copy_and_comparable() {
        let pool = Pool::<u8>::new();
        let s1 = pool.stats();
        let s2 = s1; // Copy
        assert_eq!(s1, s2);
        assert_eq!(format!("{s1:?}"), format!("{s2:?}"));
    }
}
