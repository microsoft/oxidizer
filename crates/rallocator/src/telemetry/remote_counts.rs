// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Immediate cumulative remote events and producer progress; pending stays scalar.
//!
//! Fixed counters occupy 4096 bytes, plus one machine-word ticket counter and
//! a no-drop machine-word TLS slot per participating thread. First use takes one
//! ticket RMW. A normal free still records both progress RMWs and its free RMW;
//! a drain batch records one immediate RMW. The progress pair stays on the same
//! no-drop TLS slot, including during thread teardown.
//! Snapshots fold 192 relaxed loads. They may be stale or mix slot histories,
//! not a common-instant scalar observation or a quiescence barrier. After writers
//! synchronize and stop, totals are exact modulo `usize`; overflow precludes
//! numeric monotonicity. Heap-retirement admission and pending accounting are
//! separate and unchanged.

use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

// Fixed storage avoids allocation, late-TLS owner access and lazy registry
// publication. Colliding threads deliberately share atomic, not owner-only, slots.
const SHARD_COUNT: usize = 64;
const UNASSIGNED: usize = usize::MAX;
static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);
static COUNTS: [CounterShard; SHARD_COUNT] = [const { CounterShard::new() }; SHARD_COUNT];

thread_local! {
    static SLOT: Cell<usize> = const { Cell::new(UNASSIGNED) };
}

// Keep independently updated slots off each other's cache lines.
#[repr(align(64))]
struct CounterShard {
    frees: AtomicUsize,
    drained: AtomicUsize,
    pushes_in_progress: AtomicUsize,
}

impl CounterShard {
    const fn new() -> Self {
        Self {
            frees: AtomicUsize::new(0),
            drained: AtomicUsize::new(0),
            pushes_in_progress: AtomicUsize::new(0),
        }
    }

    fn record_drain(&self, count: usize) {
        self.drained.fetch_add(count, Ordering::Relaxed);
    }

    fn record_started_free(&self) {
        self.pushes_in_progress.fetch_add(1, Ordering::Relaxed);
        self.frees.fetch_add(1, Ordering::Relaxed);
    }

    fn record_finished_free(&self) {
        self.pushes_in_progress.fetch_sub(1, Ordering::Relaxed);
    }
}

const fn slot_index(ticket: usize) -> usize {
    ticket & (SHARD_COUNT - 1)
}

fn current_shard() -> &'static CounterShard {
    let slot = SLOT.with(|slot| {
        let current = slot.get();
        if current != UNASSIGNED {
            return current;
        }
        let assigned = NEXT_SLOT.fetch_add(1, Ordering::Relaxed) & (SHARD_COUNT - 1);
        slot.set(assigned);
        assigned
    });
    &COUNTS[slot_index(slot)]
}

pub(super) fn record_free() {
    current_shard().frees.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn record_started_free() {
    current_shard().record_started_free();
}

pub(super) fn record_finished_free() {
    current_shard().record_finished_free();
}

pub(super) fn record_drain(count: usize) {
    current_shard().record_drain(count);
}

pub(super) fn frees() -> usize {
    COUNTS
        .iter()
        .fold(0usize, |sum, shard| sum.wrapping_add(shard.frees.load(Ordering::Relaxed)))
}

pub(super) fn drained() -> usize {
    COUNTS
        .iter()
        .fold(0usize, |sum, shard| sum.wrapping_add(shard.drained.load(Ordering::Relaxed)))
}

pub(super) fn pushes_in_progress() -> usize {
    COUNTS.iter().fold(0usize, |sum, shard| {
        sum.wrapping_add(shard.pushes_in_progress.load(Ordering::Relaxed))
    })
}

#[cfg(test)]
mod batch_tests;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn ticket_slots_cycle_through_every_shard_without_losing_wraparound() {
        let actual: Vec<_> = (0..SHARD_COUNT * 3).map(slot_index).collect();
        let expected: Vec<_> = (0..SHARD_COUNT).cycle().take(SHARD_COUNT * 3).collect();
        assert_eq!(actual, expected);
        assert_eq!(slot_index(usize::MAX), SHARD_COUNT - 1);
    }

    #[test]
    fn first_use_assigns_a_bounded_slot_and_reuses_it() {
        for _ in 0..=SHARD_COUNT {
            std::thread::spawn(|| {
                assert_eq!(SLOT.get(), UNASSIGNED);
                let first = current_shard();
                let assigned = SLOT.get();
                assert!(assigned < SHARD_COUNT);
                let second = current_shard();
                assert_eq!(SLOT.get(), assigned);
                assert!(std::ptr::eq(first, second));
                assert!(std::ptr::eq(first, &raw const COUNTS[assigned]));
            })
            .join()
            .unwrap();
        }
    }

    #[test]
    fn slot_bounds_and_counter_layout_are_fixed() {
        assert!(SHARD_COUNT.is_power_of_two());
        assert_eq!((size_of::<CounterShard>(), align_of::<CounterShard>()), (64, 64));
        for ticket in [0, 1, SHARD_COUNT - 1, SHARD_COUNT, usize::MAX] {
            assert!(slot_index(ticket) < SHARD_COUNT);
        }
    }

    #[test]
    fn collision_rmw_preserves_all_events() {
        let counter = Arc::new(CounterShard::new());
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let counter = Arc::clone(&counter);
                std::thread::spawn(move || {
                    for _ in 0..1024 {
                        counter.frees.fetch_add(1, Ordering::Relaxed);
                        counter.drained.fetch_add(1, Ordering::Relaxed);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(
            (counter.frees.load(Ordering::Relaxed), counter.drained.load(Ordering::Relaxed)),
            (4096, 4096)
        );
    }

    #[test]
    fn colliding_producers_preserve_progress_and_event_counts() {
        let counter = Arc::new(CounterShard::new());
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let counter = Arc::clone(&counter);
                std::thread::spawn(move || {
                    for _ in 0..1024 {
                        counter.record_started_free();
                        assert!((1..=4).contains(&counter.pushes_in_progress.load(Ordering::Relaxed)));
                        counter.record_finished_free();
                        counter.record_drain(1);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(
            (
                counter.frees.load(Ordering::Relaxed),
                counter.drained.load(Ordering::Relaxed),
                counter.pushes_in_progress.load(Ordering::Relaxed),
            ),
            (4096, 4096, 0)
        );
    }

    #[test]
    fn progress_updates_keep_modulo_accounting() {
        let counter = CounterShard::new();
        counter.pushes_in_progress.store(usize::MAX, Ordering::Relaxed);
        counter.record_started_free();
        assert_eq!(counter.pushes_in_progress.load(Ordering::Relaxed), 0);
        counter.record_finished_free();
        assert_eq!(counter.pushes_in_progress.load(Ordering::Relaxed), usize::MAX);
    }

    #[test]
    fn distributed_progress_zero_is_not_a_quiescence_barrier() {
        let shards = [CounterShard::new(), CounterShard::new()];
        shards[1].record_started_free();
        let first = shards[0].pushes_in_progress.load(Ordering::Relaxed);

        // A legal interleaving keeps a producer active throughout the observation.
        shards[0].record_started_free();
        shards[1].record_finished_free();
        let second = shards[1].pushes_in_progress.load(Ordering::Relaxed);
        let current = shards.iter().fold(0usize, |sum, shard| {
            sum.wrapping_add(shard.pushes_in_progress.load(Ordering::Relaxed))
        });

        assert_eq!((first.wrapping_add(second), current), (0, 1));
        shards[0].record_finished_free();
    }

    #[test]
    fn slot_remains_available_during_thread_destructors() {
        struct OnExit;
        impl Drop for OnExit {
            fn drop(&mut self) {
                assert!(std::ptr::eq(current_shard(), current_shard()));
            }
        }
        thread_local! {
            static ON_EXIT: OnExit = const { OnExit };
        }
        std::thread::spawn(|| ON_EXIT.with(|_| {})).join().unwrap();
    }
}
