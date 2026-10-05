// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exact arithmetic oracles use private cells, not racing process totals.

use std::sync::Arc;

use super::*;

fn drained_total(shards: &[CounterShard]) -> usize {
    shards
        .iter()
        .fold(0usize, |sum, shard| sum.wrapping_add(shard.drained.load(Ordering::Relaxed)))
}

#[test]
fn batched_drain_rmw_counts_blocks_not_batches() {
    let shard = CounterShard::new();
    for count in [1, 7, 31] {
        shard.record_drain(count);
    }
    assert_eq!(shard.drained.load(Ordering::Relaxed), 39);
}

#[test]
fn colliding_batched_drains_preserve_completed_blocks() {
    let shard = Arc::new(CounterShard::new());
    let threads: Vec<_> = [3, 7]
        .into_iter()
        .map(|count| {
            let shard = Arc::clone(&shard);
            std::thread::spawn(move || {
                for _ in 0..64 {
                    shard.record_drain(count);
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(shard.drained.load(Ordering::Relaxed), 640);
}

#[test]
fn batched_drain_wraps_within_a_full_width_slot() {
    let shard = CounterShard::new();
    shard.drained.store(usize::MAX - 1, Ordering::Relaxed);
    shard.record_drain(3);
    assert_eq!(shard.drained.load(Ordering::Relaxed), 1);
}

#[test]
fn batched_drain_fold_wraps_across_full_width_slots() {
    let shards = [CounterShard::new(), CounterShard::new()];
    shards[0].drained.store(usize::MAX - 2, Ordering::Relaxed);
    shards[1].record_drain(5);
    assert_eq!(drained_total(&shards), 2);
}

#[test]
fn ticket_wrap_collision_and_tls_storage_remain_bounded() {
    let tickets = AtomicUsize::new(usize::MAX);
    let before_wrap = tickets.fetch_add(1, Ordering::Relaxed) & (SHARD_COUNT - 1);
    let after_wrap = tickets.fetch_add(1, Ordering::Relaxed) & (SHARD_COUNT - 1);
    assert_eq!((before_wrap, after_wrap), (SHARD_COUNT - 1, 0));
    assert_eq!(slot_index(after_wrap), slot_index(after_wrap + SHARD_COUNT));
    assert_ne!(before_wrap, UNASSIGNED);
    assert_ne!(after_wrap, UNASSIGNED);
    assert_eq!(size_of::<[CounterShard; SHARD_COUNT]>(), 4096);
    assert_eq!(size_of::<Cell<usize>>(), size_of::<usize>());
    assert!(!std::mem::needs_drop::<Cell<usize>>());
}

#[test]
fn first_batched_drain_during_tls_destructor_needs_no_owner_tls() {
    struct OnExit;
    impl Drop for OnExit {
        fn drop(&mut self) {
            record_drain(7);
            record_free();
            assert!(std::ptr::eq(current_shard(), current_shard()));
        }
    }
    thread_local! {
        static ON_EXIT: OnExit = const { OnExit };
    }
    // No counter access until destruction; no exact process-wide delta assertion.
    std::thread::spawn(|| ON_EXIT.with(|_| {})).join().unwrap();
}
