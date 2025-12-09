// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Lock-free Bloom filter for fast negative lookups.
//!
//! This module provides a thread-safe Bloom filter implementation using atomic
//! operations. It's used by [`NumaCache`](crate::NumaCache) to quickly determine
//! if a key definitely doesn't exist in any shard, avoiding expensive cross-shard
//! searches for keys that were never inserted.

use std::hash::{BuildHasher, Hash};
use std::sync::atomic::{AtomicU64, Ordering};

/// A lock-free Bloom filter using atomic bit operations.
///
/// This implementation uses the Kirsch-Mitzenmacher optimization, which derives
/// multiple hash functions from just two base hashes using the formula:
/// `h_i(x) = h1(x) + i * h2(x)`
///
/// The filter supports concurrent insertions and queries without locking.
/// It does NOT support removal - once a bit is set, it stays set. This means
/// after removals from the cache, the Bloom filter may have "stale positives"
/// (reporting a key might exist when it doesn't), but it will never have
/// false negatives (reporting a key doesn't exist when it does).
pub(crate) struct BloomFilter<S> {
    /// Bit array stored as atomic u64s for lock-free access.
    bits: Box<[AtomicU64]>,
    /// Number of bits in the filter (always a power of 2 for fast modulo).
    num_bits: usize,
    /// Bit mask for fast modulo (`num_bits - 1`).
    bit_mask: usize,
    /// Number of hash functions to use.
    num_hashes: u32,
    /// Hash builder for computing hashes.
    hasher: S,
}

impl<S: BuildHasher> BloomFilter<S> {
    /// Creates a new Bloom filter sized for the expected number of items.
    ///
    /// The filter is sized to achieve approximately 1% false positive rate
    /// at the expected capacity. The actual size is rounded up to a power of 2
    /// for efficient bit indexing.
    ///
    /// # Arguments
    ///
    /// * `expected_items` - Expected number of unique items to be inserted
    /// * `hasher` - Hash builder for computing hashes
    pub fn new(expected_items: usize, hasher: S) -> Self {
        // Target ~1% false positive rate
        // Optimal bits per item ≈ -ln(p) / ln(2)^2 ≈ 9.6 for p=0.01
        // We use 10 bits per item for simplicity
        let bits_needed = expected_items.saturating_mul(10).max(64);

        // Round up to next power of 2 for fast modulo
        let num_bits = bits_needed.next_power_of_two();
        let num_u64s = num_bits.div_ceil(64);

        // Optimal number of hash functions ≈ (m/n) * ln(2) ≈ 0.693 * bits_per_item
        // For 10 bits per item: ~7 hash functions
        let num_hashes = 7;

        let bits: Vec<AtomicU64> = (0..num_u64s).map(|_| AtomicU64::new(0)).collect();

        Self {
            bits: bits.into_boxed_slice(),
            num_bits,
            bit_mask: num_bits - 1,
            num_hashes,
            hasher,
        }
    }

    /// Inserts a key into the Bloom filter.
    ///
    /// This operation is lock-free and thread-safe.
    pub fn insert<K: Hash>(&self, key: &K) {
        let (h1, h2) = self.compute_hashes(key);

        for i in 0..self.num_hashes {
            let bit_index = self.get_bit_index(h1, h2, i);
            self.set_bit(bit_index);
        }
    }

    /// Checks if a key might be in the filter.
    ///
    /// Returns:
    /// - `false` if the key is definitely NOT in the set (no false negatives)
    /// - `true` if the key MIGHT be in the set (possible false positives)
    ///
    /// This operation is lock-free and thread-safe.
    #[must_use]
    pub fn might_contain<K: Hash>(&self, key: &K) -> bool {
        let (h1, h2) = self.compute_hashes(key);

        for i in 0..self.num_hashes {
            let bit_index = self.get_bit_index(h1, h2, i);
            if !self.get_bit(bit_index) {
                return false;
            }
        }
        true
    }

    /// Computes two independent hashes for Kirsch-Mitzenmacher optimization.
    fn compute_hashes<K: Hash>(&self, key: &K) -> (u64, u64) {
        let hash = self.hasher.hash_one(key);

        // Split the 64-bit hash into two 32-bit values by rotating
        // This provides better independence than deriving h2 from h1
        let h1 = hash;
        let h2 = hash.rotate_left(32);

        (h1, h2)
    }

    /// Gets the bit index for the i-th hash function using Kirsch-Mitzenmacher.
    #[inline]
    #[expect(clippy::cast_possible_truncation, reason = "bit_mask ensures result fits in usize")]
    fn get_bit_index(&self, h1: u64, h2: u64, i: u32) -> usize {
        let combined = h1.wrapping_add(h2.wrapping_mul(u64::from(i)));
        (combined as usize) & self.bit_mask
    }

    /// Sets a bit at the given index using atomic OR.
    #[inline]
    fn set_bit(&self, bit_index: usize) {
        let word_index = bit_index / 64;
        let bit_offset = bit_index % 64;
        let mask = 1u64 << bit_offset;

        // Relaxed ordering is sufficient - we only care about eventual visibility
        // and the filter is tolerant of concurrent races (worst case: duplicate sets)
        self.bits[word_index].fetch_or(mask, Ordering::Relaxed);
    }

    /// Gets a bit at the given index.
    #[inline]
    fn get_bit(&self, bit_index: usize) -> bool {
        let word_index = bit_index / 64;
        let bit_offset = bit_index % 64;
        let mask = 1u64 << bit_offset;

        // Relaxed ordering is sufficient - false positives are acceptable,
        // and we never want false negatives (which can't happen with relaxed
        // reads since bits are only ever set, never cleared)
        (self.bits[word_index].load(Ordering::Relaxed) & mask) != 0
    }

    /// Returns the size of the filter in bits.
    #[cfg(test)]
    #[must_use]
    pub fn size_bits(&self) -> usize {
        self.num_bits
    }
}

impl<S> std::fmt::Debug for BloomFilter<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BloomFilter")
            .field("size_bits", &self.num_bits)
            .field("num_hashes", &self.num_hashes)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hashbrown::DefaultHashBuilder;

    #[test]
    fn test_bloom_filter_basic() {
        let filter: BloomFilter<DefaultHashBuilder> = BloomFilter::new(1000, DefaultHashBuilder::default());

        // Insert some keys
        filter.insert(&"hello");
        filter.insert(&"world");
        filter.insert(&42i32);

        // Inserted keys should be found
        assert!(filter.might_contain(&"hello"));
        assert!(filter.might_contain(&"world"));
        assert!(filter.might_contain(&42i32));

        // Non-inserted keys should (probably) not be found
        // Note: There's a small chance of false positives, but with 1000 capacity
        // and only 3 insertions, it's extremely unlikely
        assert!(!filter.might_contain(&"goodbye"));
        assert!(!filter.might_contain(&"universe"));
        assert!(!filter.might_contain(&999i32));
    }

    #[test]
    fn test_bloom_filter_no_false_negatives() {
        let filter: BloomFilter<DefaultHashBuilder> = BloomFilter::new(10000, DefaultHashBuilder::default());

        // Insert many keys
        for i in 0..1000 {
            filter.insert(&i);
        }

        // All inserted keys MUST be found (no false negatives allowed)
        for i in 0..1000 {
            assert!(filter.might_contain(&i), "key {i} should be found");
        }
    }

    #[test]
    fn test_bloom_filter_false_positive_rate() {
        let filter: BloomFilter<DefaultHashBuilder> = BloomFilter::new(10000, DefaultHashBuilder::default());

        // Insert 10000 keys
        for i in 0..10000 {
            filter.insert(&i);
        }

        // Check 10000 keys that were NOT inserted
        let mut false_positives = 0;
        for i in 10000..20000 {
            if filter.might_contain(&i) {
                false_positives += 1;
            }
        }

        // With 10 bits per item and 7 hash functions, we expect ~1% FP rate
        // Allow up to 3% to account for variance
        let fp_rate = f64::from(false_positives) / 10000.0;
        assert!(
            fp_rate < 0.03,
            "false positive rate {:.2}% is too high (expected < 3%)",
            fp_rate * 100.0
        );
    }

    #[test]
    fn test_bloom_filter_concurrent() {
        use std::sync::Arc;
        use std::thread;

        let filter = Arc::new(BloomFilter::<DefaultHashBuilder>::new(100_000, DefaultHashBuilder::default()));

        let mut handles = vec![];

        // Spawn multiple writer threads
        for t in 0..4 {
            let filter = Arc::clone(&filter);
            handles.push(thread::spawn(move || {
                for i in 0..10000 {
                    let key = t * 100_000 + i;
                    filter.insert(&key);
                }
            }));
        }

        // Wait for all writers
        for handle in handles {
            handle.join().expect("thread panicked");
        }

        // Verify all inserted keys are found
        for t in 0..4 {
            for i in 0..10000 {
                let key = t * 100_000 + i;
                assert!(filter.might_contain(&key), "key {key} should be found");
            }
        }
    }

    #[test]
    fn test_bloom_filter_sizing() {
        // Small filter
        let small: BloomFilter<DefaultHashBuilder> = BloomFilter::new(100, DefaultHashBuilder::default());
        assert!(small.size_bits() >= 1000); // At least 10 bits per item
        assert!(small.size_bits().is_power_of_two());

        // Large filter
        let large: BloomFilter<DefaultHashBuilder> = BloomFilter::new(1_000_000, DefaultHashBuilder::default());
        assert!(large.size_bits() >= 10_000_000);
        assert!(large.size_bits().is_power_of_two());
    }
}
