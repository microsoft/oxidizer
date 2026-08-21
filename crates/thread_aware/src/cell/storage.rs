// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Primitives for thread-aware data storage.

#[cfg(not(test))]
use alloc::boxed::Box;
#[cfg(not(test))]
use alloc::vec::Vec;
use std::marker::PhantomData;
#[cfg(test)]
use std::sync::RwLockReadGuard;
use std::sync::{self, OnceLock, RwLock, RwLockWriteGuard};

use crossbeam_utils::CachePadded;
use nm::Event;

use crate::affinity::Affinity;

/// A strategy for storing data in a affinity-aware manner.
pub trait Strategy {
    /// Returns the slot index for the given affinity.
    fn index(affinity: Affinity) -> usize;

    /// Returns the number of slots the storage holds.
    fn count(affinity: Affinity) -> usize;
}

/// A slot lock is never left poisoned, so acquiring it never fails.
///
/// Poisoning happens when a thread panics while holding the lock and the guard is
/// dropped during unwinding. The operations run under a slot lock — cloning,
/// storing and comparing the reference-counted handle it holds — cannot unwind,
/// so the only code that can panic there is the caller's factory on relocation's
/// miss path. `relocate` runs that under `catch_unwind` and drops the guard before
/// resuming the unwind, so the lock is released normally rather than poisoned.
/// Ref: docs/implementation.md, "Relocation locking".
const NEVER_POISONED: &str =
    "a slot lock is never left poisoned; a panic while one is held is caught and the lock released before the unwind resumes";

/// One slot: an independently-locked value cell, cache-line padded to curb false sharing.
type Slot<T> = CachePadded<RwLock<Option<T>>>;

/// Affinity-partitioned storage: one independently-locked slot per affinity.
///
/// This is the raw slot table. [`Storage`] wraps it as the handle an
/// `Arc` actually holds; the two are separate so this can be unit-tested with a
/// plain value type while the wrapper pins the stored type to `Arc<T>`.
///
/// Each slot owns its own `RwLock`, so relocations into different slots never
/// touch the same lock. The strategy decides how affinities map to slots:
/// `PerCore` gives each processor its own slot, while `PerNuma` and `PerProcess`
/// map several affinities onto one shared slot. The slots are cache-line padded,
/// using `CachePadded`'s target-specific alignment estimate, to curb false sharing
/// between neighboring locks rather than to guarantee physical isolation.
///
/// The slot array is sized once, on first use, to `S::count(affinity)` — a value
/// fixed for the process lifetime — so there is no growth path and therefore no
/// table-wide lock guarding it. After initialization the array and the pointer to
/// it are immutable, so reaching a slot is a plain atomic load that carries no
/// further synchronization; its cache behavior is left to the hardware.
#[derive(Debug)]
pub(crate) struct SlotTable<T, S: Strategy> {
    slots: OnceLock<Box<[Slot<T>]>>,
    _marker: PhantomData<S>,
}

impl<T, S: Strategy> SlotTable<T, S> {
    /// Creates a new empty `SlotTable` instance.
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            slots: OnceLock::new(),
            _marker: PhantomData,
        }
    }

    /// Returns the slot array, sizing it on first use to hold every affinity.
    fn slots(&self, affinity: Affinity) -> &[Slot<T>] {
        self.slots.get_or_init(|| {
            (0..S::count(affinity))
                .map(|_| CachePadded::new(RwLock::new(None)))
                .collect::<Vec<_>>()
                .into_boxed_slice()
        })
    }

    /// Returns the lock guarding the slot for `affinity`, or `None` when that affinity's index falls
    /// outside the sized table.
    ///
    /// The table is sized once, on first use, to the slot count the first affinity reported, so an
    /// index past its end means this affinity does not share that coordinate space. Rather than
    /// reaching into an unrelated slot, the lookup reports the slot as absent and leaves each caller
    /// to treat the affinity as unreachable.
    fn slot(&self, affinity: Affinity) -> Option<&RwLock<Option<T>>> {
        self.slots(affinity).get(S::index(affinity)).map(|slot| &**slot)
    }

    /// Replaces the data for the given affinity with the provided value.
    ///
    /// Returns the previous value if it existed, otherwise returns `None`. An affinity whose slot is
    /// out of range has nowhere to hold the value, so the value is dropped and `None` is returned.
    pub(crate) fn replace(&self, affinity: Affinity, value: T) -> Option<T> {
        self.slot(affinity)?.write().expect(NEVER_POISONED).replace(value)
    }

    /// Acquires the exclusive lock on the slot for `affinity`, or `None` when that affinity's slot is
    /// out of range.
    ///
    /// The caller drives the miss path with this: re-probe the slot, materialize the value, and store
    /// it, all while holding the returned guard so that only this affinity is affected and no other
    /// thread can materialize it in the meantime. An out-of-range affinity has no slot to claim, so
    /// `None` is returned and the anomaly recorded via the `thread_aware_arc_oob` metric.
    pub(crate) fn write(&self, affinity: Affinity) -> Option<RwLockWriteGuard<'_, Option<T>>> {
        let Some(slot) = self.slot(affinity) else {
            report_out_of_range_affinity();
            return None;
        };

        Some(slot.write().expect(NEVER_POISONED))
    }

    /// Acquires the shared lock on the slot for `affinity`, or `None` when that affinity's slot is out
    /// of range.
    ///
    /// Used by tests to pin a slot so racing relocations pile up on its exclusive
    /// lock; production relocation reads through [`get_clone`](Self::get_clone).
    #[cfg(test)]
    pub(crate) fn read(&self, affinity: Affinity) -> Option<RwLockReadGuard<'_, Option<T>>> {
        Some(self.slot(affinity)?.read().expect(NEVER_POISONED))
    }
}

/// Records that a relocation reached an affinity whose slot index falls outside the sized table.
///
/// The table is sized once to one affinity's slot count, so an index past its end means the affinity
/// does not share that coordinate space and has no slot of its own. Relocation treats such an
/// affinity as unreachable and leaves the `Arc` on the value it already carries rather than reaching
/// into an unrelated slot. This is a supported-but-degraded path, so it is recorded as an observable
/// metric rather than trapped: a process that suspects it is happening can inspect the
/// `thread_aware_arc_oob` event. Ref: docs/implementation.md, "Storage".
///
/// Marked `#[cold]` and split into its own function so the emission machinery is laid out off the hot
/// path.
#[cold]
fn report_out_of_range_affinity() {
    std::thread_local! {
        static ARC_OOB: Event = Event::builder().name("thread_aware_arc_oob").build();
    }

    ARC_OOB.with(Event::observe_once);
}

impl<T, S: Strategy> Default for SlotTable<T, S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, S: Strategy> SlotTable<T, S>
where
    T: Clone,
{
    /// Clone and gets the data for the given affinity if it exists.
    /// Returns `None` if the data does not exist for that affinity.
    #[must_use]
    pub(crate) fn get_clone(&self, affinity: Affinity) -> Option<T> {
        self.slot(affinity)?.read().expect(NEVER_POISONED).clone()
    }

    /// Counts how many stored entries satisfy the given predicate.
    ///
    /// The count is an estimate under concurrent relocation, matching the
    /// inherently racy nature of a strong-count query.
    pub(crate) fn count_where(&self, predicate: impl Fn(&T) -> bool) -> usize {
        let Some(slots) = self.slots.get() else {
            return 0;
        };

        // Clone each value out from under its slot lock and apply the predicate afterwards, so no
        // caller code runs while a lock is held. Slots are visited one at a time rather than under
        // a single consistent snapshot, which is what leaves the count an estimate.
        slots
            .iter()
            .filter_map(|slot| slot.read().expect(NEVER_POISONED).clone())
            .filter(|value| predicate(value))
            .count()
    }
}

/// Per-affinity storage shared by every clone of an `Arc`.
///
/// A relocation into an affinity publishes the value here; later relocations into
/// the same affinity read it back. This can also be built directly and populated
/// with [`insert`](Self::insert), then handed to [`Arc::from_storage`] to produce
/// an `Arc` backed by it — the way to hand an `Arc` a set of per-affinity values
/// prepared in advance.
///
/// [`Arc::from_storage`]: crate::Arc::from_storage
#[derive(Debug)]
pub struct Storage<T: ?Sized, S: Strategy> {
    inner: SlotTable<sync::Arc<T>, S>,
}

impl<T: ?Sized, S: Strategy> Storage<T, S> {
    /// Creates an empty storage, with no affinity populated.
    #[must_use]
    pub const fn new() -> Self {
        Self { inner: SlotTable::new() }
    }

    /// Sets the value for `affinity`, returning the previous one if there was one.
    pub fn insert(&self, affinity: Affinity, value: sync::Arc<T>) -> Option<sync::Arc<T>> {
        self.inner.replace(affinity, value)
    }

    /// Returns a clone of the value published for `affinity`, if any.
    #[must_use]
    pub fn get(&self, affinity: Affinity) -> Option<sync::Arc<T>> {
        self.inner.get_clone(affinity)
    }

    /// Acquires the exclusive lock on the slot for `affinity`, or `None` when that affinity's slot is
    /// out of range.
    pub(crate) fn write(&self, affinity: Affinity) -> Option<RwLockWriteGuard<'_, Option<sync::Arc<T>>>> {
        self.inner.write(affinity)
    }

    /// Counts published values for which `predicate` holds.
    pub(crate) fn count_where(&self, predicate: impl Fn(&sync::Arc<T>) -> bool) -> usize {
        self.inner.count_where(predicate)
    }

    /// Acquires the shared lock on the slot for `affinity`, or `None` when that affinity's slot is out
    /// of range.
    #[cfg(test)]
    pub(crate) fn read(&self, affinity: Affinity) -> Option<RwLockReadGuard<'_, Option<sync::Arc<T>>>> {
        self.inner.read(affinity)
    }
}

impl<T: ?Sized, S: Strategy> Default for Storage<T, S> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use crate::affinity::pinned_affinities;
    use crate::storage::{SlotTable, Strategy};
    use crate::{PerCore, PerNuma, PerProcess};

    #[test]
    fn replace_returns_previous_value() {
        let affinities = pinned_affinities(&[1]);
        let storage = SlotTable::<String, PerCore>::default();
        let affinity = affinities[0];

        // First replace should return None (no previous value)
        let previous = storage.replace(affinity, "First".to_string());
        assert_eq!(previous, None);

        // Second replace should return the previous value
        let previous = storage.replace(affinity, "Second".to_string());
        assert_eq!(previous, Some("First".to_string()));

        // Third replace should return the new previous value
        let previous = storage.replace(affinity, "Third".to_string());
        assert_eq!(previous, Some("Second".to_string()));
    }

    #[test]
    fn get_clone() {
        let affinities = pinned_affinities(&[1]);

        let storage = SlotTable::<String, PerCore>::default();
        let affinity = affinities[0];

        assert!(storage.get_clone(affinity).is_none());

        storage.replace(affinity, "Hello".to_string());
        assert_eq!(storage.get_clone(affinity), Some("Hello".to_string()));
    }

    /// A `Strategy` that hands out an index outside the slot count it reports. Exists only to drive
    /// the out-of-range path in `SlotTable`.
    struct InconsistentStrategy;

    impl Strategy for InconsistentStrategy {
        fn index(_affinity: crate::affinity::Affinity) -> usize {
            1
        }

        fn count(_affinity: crate::affinity::Affinity) -> usize {
            1
        }
    }

    #[test]
    fn out_of_range_affinity_is_a_no_op() {
        let affinity = pinned_affinities(&[1])[0];
        let table = SlotTable::<i32, InconsistentStrategy>::new();

        // `index` reports 1 for a table sized to a single slot, so the requested slot is out of
        // range. The affinity has no slot of its own, so a write through it stores nothing and a read
        // finds nothing rather than reaching into an unrelated slot.
        assert!(table.replace(affinity, 42).is_none());
        assert_eq!(table.get_clone(affinity), None);
    }

    #[test]
    fn out_of_range_affinity_records_oob_metric() {
        use nm::Report;

        fn oob_count() -> u64 {
            Report::collect()
                .events()
                .find(|event| event.name().as_ref() == "thread_aware_arc_oob")
                .map_or(0, nm::EventMetrics::count)
        }

        let affinity = pinned_affinities(&[1])[0];
        let table = SlotTable::<i32, InconsistentStrategy>::new();

        // The anomalous strategy indexes past the single-slot table, so escalating to the slot's
        // exclusive lock finds no slot and records the out-of-range metric. The registry is
        // process-wide, so the count is asserted as a strict increase rather than an absolute value.
        let before = oob_count();
        assert!(table.write(affinity).is_none());
        let after = oob_count();

        assert!(
            after > before,
            "the out-of-range access must record the thread_aware_arc_oob metric (before={before}, after={after})"
        );
    }

    #[test]
    fn per_app() {
        let affinities = pinned_affinities(&[1, 1]);

        let index = PerProcess::index(affinities[0]);
        let count = PerProcess::count(affinities[0]);
        assert_eq!(index, 0);
        assert_eq!(count, 1);
    }

    #[test]
    fn per_memory_region() {
        let affinities = pinned_affinities(&[1, 1]);

        for affinity in affinities {
            let index = PerNuma::index(affinity);
            let count = PerNuma::count(affinity);
            assert_eq!(index, affinity.memory_region_index());
            assert_eq!(count, affinity.memory_region_count());
        }
    }

    #[test]
    fn per_processor() {
        let affinities = pinned_affinities(&[1, 1]);

        for affinity in affinities {
            let index = PerCore::index(affinity);
            let count = PerCore::count(affinity);
            assert_eq!(index, affinity.processor_index());
            assert_eq!(count, affinity.processor_count());
        }
    }

    #[test]
    fn test_default_implementation() {
        // This test covers line 101: Self::new() in the Default trait implementation
        let affinities = pinned_affinities(&[1]);

        // Create storage using Default trait - this exercises line 101
        let storage = SlotTable::<String, PerCore>::default();
        let affinity = affinities[0];

        // Verify the default storage is empty (no data for any affinity)
        assert!(storage.get_clone(affinity).is_none());

        // Verify it works the same as SlotTable::new()
        storage.replace(affinity, "test".to_string());
        assert_eq!(storage.get_clone(affinity), Some("test".to_string()));
    }
}
