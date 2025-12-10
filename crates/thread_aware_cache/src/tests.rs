// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the NUMA cache.

use crate::NumaCache;
use thread_aware::create_manual_pinned_affinities;

#[test]
fn test_eviction_under_pressure() {
    let affinities = create_manual_pinned_affinities(&[1]);
    let cache = NumaCache::<i32, i32>::builder()
        .affinities(&affinities)
        .capacity_per_shard(5)
        .build();
    let affinity = affinities[0];

    // Fill the cache
    for i in 0..5 {
        cache.insert(affinity, i, i * 10);
    }
    assert_eq!(cache.len(), 5);

    // Access some entries to mark them as visited
    let _ = cache.get(affinity, &0);
    let _ = cache.get(affinity, &1);

    // Insert more entries, triggering evictions
    for i in 5..10 {
        cache.insert(affinity, i, i * 10);
    }

    // Should still have exactly 5 entries
    assert_eq!(cache.len(), 5);

    // The most recently inserted and accessed entries should be present
    // SIEVE should have evicted unvisited entries first
    let mut found = 0;
    for i in 0..10 {
        if cache.get(affinity, &i).is_some() {
            found += 1;
        }
    }
    assert_eq!(found, 5);
}

#[test]
fn test_multi_shard_distribution() {
    let affinities = create_manual_pinned_affinities(&[1, 1, 1, 1]);
    let cache = NumaCache::<i32, i32>::builder()
        .affinities(&affinities)
        .capacity_per_shard(100)
        .build();

    // Insert entries across all affinities
    for (i, &affinity) in affinities.iter().enumerate() {
        let base = i32::try_from(i).expect("index fits in i32") * 100;
        for j in 0..25 {
            let key = base + j;
            cache.insert(affinity, key, key);
        }
    }

    // All entries should be retrievable from their respective affinities
    for (i, &affinity) in affinities.iter().enumerate() {
        let base = i32::try_from(i).expect("index fits in i32") * 100;
        for j in 0..25 {
            let key = base + j;
            assert_eq!(cache.get(affinity, &key), Some(key), "key {key} should be present");
        }
    }

    // Check that entries are distributed across shards
    let mut shard_counts = [0usize; 4];
    for (i, count) in shard_counts.iter_mut().enumerate() {
        *count = cache.shard(i).len();
    }

    // Each shard should have exactly 25 entries
    for (i, &count) in shard_counts.iter().enumerate() {
        assert_eq!(count, 25, "shard {i} should have 25 entries");
    }
}

#[test]
fn test_concurrent_access() {
    use std::sync::Arc;
    use std::thread;

    let affinities = create_manual_pinned_affinities(&[1, 1, 1, 1]);
    let cache = Arc::new(
        NumaCache::<i32, i32>::builder()
            .affinities(&affinities)
            .capacity_per_shard(1000)
            .build(),
    );
    let affinities = Arc::new(affinities);

    let mut handles = vec![];

    // Spawn writer threads
    for t in 0..4 {
        let cache = Arc::clone(&cache);
        let affinities = Arc::clone(&affinities);
        handles.push(thread::spawn(move || {
            let affinity = affinities[t];
            let base = i32::try_from(t).expect("index fits in i32") * 1000;
            for i in 0..250 {
                let key = base + i;
                cache.insert(affinity, key, key);
            }
        }));
    }

    // Spawn reader threads
    for t in 0..4 {
        let cache = Arc::clone(&cache);
        let affinities = Arc::clone(&affinities);
        handles.push(thread::spawn(move || {
            let affinity = affinities[t];
            let base = i32::try_from(t).expect("index fits in i32") * 1000;
            for i in 0..250 {
                let key = base + i;
                // May or may not find the key depending on timing
                let _ = cache.get(affinity, &key);
            }
        }));
    }

    // Wait for all threads to complete
    for handle in handles {
        handle.join().expect("thread should not panic");
    }

    // Cache should have entries from all writer threads
    assert!(!cache.is_empty());
}

#[test]
fn test_update_marks_visited() {
    let affinities = create_manual_pinned_affinities(&[1]);
    let cache = NumaCache::<i32, i32>::builder()
        .affinities(&affinities)
        .capacity_per_shard(3)
        .build();
    let affinity = affinities[0];

    // Insert 3 entries
    cache.insert(affinity, 1, 10);
    cache.insert(affinity, 2, 20);
    cache.insert(affinity, 3, 30);

    // Update entry 1 (should mark it as visited)
    cache.insert(affinity, 1, 15);

    // Insert a new entry, triggering eviction
    cache.insert(affinity, 4, 40);

    // Entry 1 should still exist (it was visited via update)
    assert_eq!(cache.get(affinity, &1), Some(15));
}

#[test]
fn test_get_marks_visited() {
    let affinities = create_manual_pinned_affinities(&[1]);
    let cache = NumaCache::<i32, i32>::builder()
        .affinities(&affinities)
        .capacity_per_shard(3)
        .build();
    let affinity = affinities[0];

    // Insert 3 entries
    cache.insert(affinity, 1, 10);
    cache.insert(affinity, 2, 20);
    cache.insert(affinity, 3, 30);

    // Access entry 2 (should mark it as visited)
    assert_eq!(cache.get(affinity, &2), Some(20));

    // Insert a new entry, triggering eviction
    cache.insert(affinity, 4, 40);

    // Entry 2 should still exist (it was visited via get)
    assert_eq!(cache.get(affinity, &2), Some(20));
}

#[test]
fn test_remove_frees_slot() {
    let affinities = create_manual_pinned_affinities(&[1]);
    let cache = NumaCache::<i32, i32>::builder()
        .affinities(&affinities)
        .capacity_per_shard(3)
        .build();
    let affinity = affinities[0];

    // Fill the cache
    cache.insert(affinity, 1, 10);
    cache.insert(affinity, 2, 20);
    cache.insert(affinity, 3, 30);
    assert_eq!(cache.len(), 3);

    // Remove one entry
    cache.remove(affinity, &2);
    assert_eq!(cache.len(), 2);

    // Should be able to insert without eviction
    cache.insert(affinity, 4, 40);
    assert_eq!(cache.len(), 3);

    // All three remaining entries should be accessible
    assert_eq!(cache.get(affinity, &1), Some(10));
    assert_eq!(cache.get(affinity, &3), Some(30));
    assert_eq!(cache.get(affinity, &4), Some(40));
}

#[test]
fn test_string_keys() {
    let affinities = create_manual_pinned_affinities(&[1, 1]);
    let cache = NumaCache::<String, String>::builder()
        .affinities(&affinities)
        .capacity_per_shard(10)
        .build();

    cache.insert(affinities[0], "hello".to_string(), "world".to_string());
    cache.insert(affinities[1], "foo".to_string(), "bar".to_string());

    assert_eq!(cache.get(affinities[0], &"hello".to_string()), Some("world".to_string()));
    assert_eq!(cache.get(affinities[1], &"foo".to_string()), Some("bar".to_string()));
}

#[test]
fn test_cross_shard_get_clones_locally() {
    // Test that getting a key from a different shard clones it locally
    let affinities = create_manual_pinned_affinities(&[1, 1]);
    let cache = NumaCache::<String, i32>::builder()
        .affinities(&affinities)
        .capacity_per_shard(10)
        .build();

    // Insert on shard 0
    cache.insert(affinities[0], "key".to_string(), 42);
    assert_eq!(cache.shard(0).len(), 1);
    assert_eq!(cache.shard(1).len(), 0);

    // Get from shard 1 - should find in shard 0 and clone to shard 1
    assert_eq!(cache.get(affinities[1], &"key".to_string()), Some(42));

    // Now both shards should have the key
    assert_eq!(cache.shard(0).len(), 1);
    assert_eq!(cache.shard(1).len(), 1);

    // Both shards should return the value
    assert_eq!(cache.get(affinities[0], &"key".to_string()), Some(42));
    assert_eq!(cache.get(affinities[1], &"key".to_string()), Some(42));
}

#[test]
fn test_cross_shard_get_local_first() {
    // Test that local shard is checked first (fast path)
    let affinities = create_manual_pinned_affinities(&[1, 1]);
    let cache = NumaCache::<String, i32>::builder()
        .affinities(&affinities)
        .capacity_per_shard(10)
        .build();

    // Insert on shard 0
    cache.insert(affinities[0], "key".to_string(), 42);

    // Get from shard 0 - should find locally, no cross-shard lookup
    assert_eq!(cache.get(affinities[0], &"key".to_string()), Some(42));

    // Shard 1 should still be empty (no unnecessary cloning)
    assert_eq!(cache.shard(1).len(), 0);
}

#[test]
fn test_cross_shard_remove_clears_all() {
    // Test that remove clears from all shards
    let affinities = create_manual_pinned_affinities(&[1, 1, 1]);
    let cache = NumaCache::<String, i32>::builder()
        .affinities(&affinities)
        .capacity_per_shard(10)
        .build();

    // Insert on shard 0
    cache.insert(affinities[0], "key".to_string(), 42);

    // Get from shard 1 and shard 2 - clones to both
    let _ = cache.get(affinities[1], &"key".to_string());
    let _ = cache.get(affinities[2], &"key".to_string());

    // All three shards should have the key
    assert_eq!(cache.shard(0).len(), 1);
    assert_eq!(cache.shard(1).len(), 1);
    assert_eq!(cache.shard(2).len(), 1);
    assert_eq!(cache.len(), 3);

    // Remove from any shard - should clear from ALL shards
    assert_eq!(cache.remove(affinities[1], &"key".to_string()), Some(42));

    // All shards should be empty now
    assert_eq!(cache.shard(0).len(), 0);
    assert_eq!(cache.shard(1).len(), 0);
    assert_eq!(cache.shard(2).len(), 0);

    // Key should not be found anywhere
    assert!(cache.get(affinities[0], &"key".to_string()).is_none());
    assert!(cache.get(affinities[1], &"key".to_string()).is_none());
    assert!(cache.get(affinities[2], &"key".to_string()).is_none());
}

#[test]
fn test_cross_shard_eviction_local_only() {
    // Test that eviction is still shard-local
    let affinities = create_manual_pinned_affinities(&[1, 1]);
    let cache = NumaCache::<i32, i32>::builder()
        .affinities(&affinities)
        .capacity_per_shard(3)
        .build();

    // Fill shard 0 with keys 1, 2, 3
    cache.insert(affinities[0], 1, 10);
    cache.insert(affinities[0], 2, 20);
    cache.insert(affinities[0], 3, 30);

    // Clone key 1 to shard 1
    let _ = cache.get(affinities[1], &1);
    assert_eq!(cache.shard(1).len(), 1);

    // Fill shard 1 by adding more keys (causes eviction on shard 1 only)
    cache.insert(affinities[1], 100, 1000);
    cache.insert(affinities[1], 101, 1010);
    // This insert may evict key 1 from shard 1 if it wasn't visited
    cache.insert(affinities[1], 102, 1020);

    // Shard 0 should still have all 3 original keys
    assert_eq!(cache.shard(0).len(), 3);
    assert_eq!(cache.get(affinities[0], &1), Some(10));
    assert_eq!(cache.get(affinities[0], &2), Some(20));
    assert_eq!(cache.get(affinities[0], &3), Some(30));
}

#[test]
fn test_cross_shard_multiple_affinities() {
    // Test cross-shard behavior with many affinities
    let affinities = create_manual_pinned_affinities(&[1, 1, 1, 1]);
    let cache = NumaCache::<String, i32>::builder()
        .affinities(&affinities)
        .capacity_per_shard(100)
        .build();

    // Insert data on each shard
    for (i, &aff) in affinities.iter().enumerate() {
        let idx = i32::try_from(i).expect("index fits in i32");
        cache.insert(aff, format!("key_{i}"), idx);
    }

    // Each shard should have 1 entry
    for i in 0..4 {
        assert_eq!(cache.shard(i).len(), 1);
    }

    // Access all keys from affinity 0 - should clone them all locally
    for i in 0..4i32 {
        assert_eq!(cache.get(affinities[0], &format!("key_{i}")), Some(i));
    }

    // Shard 0 should now have all 4 keys
    assert_eq!(cache.shard(0).len(), 4);

    // Other shards still have their original key
    for i in 1..4 {
        assert_eq!(cache.shard(i).len(), 1);
    }
}

#[test]
fn test_cross_shard_concurrent_access() {
    use std::sync::Arc;
    use std::thread;

    let affinities = create_manual_pinned_affinities(&[1, 1, 1, 1]);
    let cache = Arc::new(
        NumaCache::<i32, i32>::builder()
            .affinities(&affinities)
            .capacity_per_shard(1000)
            .build(),
    );
    let affinities = Arc::new(affinities);

    // Insert some data on shard 0
    for i in 0..100 {
        cache.insert(affinities[0], i, i * 10);
    }

    let mut handles = vec![];

    // Spawn reader threads on all affinities trying to access the same keys
    for t in 0..4 {
        let cache = Arc::clone(&cache);
        let affinities = Arc::clone(&affinities);
        handles.push(thread::spawn(move || {
            let affinity = affinities[t];
            for i in 0..100 {
                // All threads try to read the same keys, triggering cross-shard cloning
                let result = cache.get(affinity, &i);
                assert_eq!(result, Some(i * 10), "key {i} should be found from affinity {t}");
            }
        }));
    }

    // Wait for all threads to complete
    for handle in handles {
        handle.join().expect("thread should not panic");
    }

    // All keys should be accessible from any affinity
    for &aff in affinities.iter() {
        for i in 0..100 {
            assert_eq!(cache.get(aff, &i), Some(i * 10));
        }
    }
}

#[test]
fn test_bloom_filter_optimization() {
    // Test that the Bloom filter correctly identifies keys that don't exist
    let affinities = create_manual_pinned_affinities(&[1, 1, 1, 1]);
    let cache = NumaCache::<i32, i32>::builder()
        .affinities(&affinities)
        .capacity_per_shard(100)
        .build();

    // Insert some keys on shard 0
    for i in 0..50 {
        cache.insert(affinities[0], i, i * 10);
    }

    // Keys that exist should be found (Bloom filter returns positive)
    for i in 0..50 {
        assert_eq!(cache.get(affinities[1], &i), Some(i * 10), "key {i} should be found");
    }

    // Keys that don't exist should return None efficiently
    // (Bloom filter should return negative, avoiding cross-shard search)
    for i in 1000..1050 {
        assert!(cache.get(affinities[0], &i).is_none(), "key {i} should not exist");
        assert!(cache.get(affinities[1], &i).is_none(), "key {i} should not exist");
    }
}

#[test]
fn test_bloom_filter_no_false_negatives_in_cache() {
    // Verify the Bloom filter never causes false negatives
    let affinities = create_manual_pinned_affinities(&[1, 1, 1, 1]);
    let cache = NumaCache::<i32, i32>::builder()
        .affinities(&affinities)
        .capacity_per_shard(1000)
        .build();

    // Insert keys across different shards
    for (idx, &aff) in affinities.iter().enumerate() {
        let base = i32::try_from(idx).expect("index fits in i32") * 1000;
        for i in 0..100 {
            cache.insert(aff, base + i, i);
        }
    }

    // All inserted keys MUST be found from any affinity
    // (tests that Bloom filter never produces false negatives)
    for (idx, _) in affinities.iter().enumerate() {
        let base = i32::try_from(idx).expect("index fits in i32") * 1000;
        for (check_idx, &check_aff) in affinities.iter().enumerate() {
            for i in 0..100 {
                let result = cache.get(check_aff, &(base + i));
                assert_eq!(
                    result,
                    Some(i),
                    "key {} should be found from affinity {} (inserted on affinity {})",
                    base + i,
                    check_idx,
                    idx
                );
            }
        }
    }
}
