// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Primitives for thread-aware data storage.

#[cfg(not(test))]
use alloc::vec::Vec;
use std::marker::PhantomData;
#[cfg(test)]
use std::sync::RwLockReadGuard;
use std::sync::{self, RwLock, RwLockWriteGuard};

use crossbeam_utils::CachePadded;

use crate::affinity::Affinity;

/// A strategy for storing data in a affinity-aware manner.
pub trait Strategy {
    /// Returns the slot index for the given affinity.
    fn index(affinity: Affinity) -> usize;

    /// Returns the number of slots the table is expected to hold for this affinity.
    ///
    /// The table sizes itself to this on first use as an allocation hint. It is not a hard bound:
    /// an [`index`](Self::index) beyond it grows the table to fit rather than being rejected.
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
/// The table grows on demand and is lock-free to read. It is a concurrent append-only
/// vector: the steady state is a hit, which finds the slot with a single lock-free indexed
/// read that scales across cores rather than contending on a shared table lock. The first
/// access for an index the table does not yet cover appends empty slots until it is reached,
/// sizing toward the strategy's reported count when that is larger so a well-behaved strategy
/// fills its slots on the first access. The table only ever appends and never moves or removes
/// a slot, so each slot keeps a stable address and a reference to one stays valid for the life
/// of the table even as later growth extends it.
#[derive(Debug)]
pub(crate) struct SlotTable<T, S: Strategy> {
    slots: boxcar::Vec<Slot<T>>,
    _marker: PhantomData<S>,
}

impl<T, S: Strategy> SlotTable<T, S> {
    /// Creates a new empty `SlotTable` instance.
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            slots: boxcar::Vec::new(),
            _marker: PhantomData,
        }
    }

    /// Returns the slot for `affinity`, growing the table to cover it if needed.
    ///
    /// The steady state is the fast path: a single lock-free indexed read. Reaching an index the
    /// table does not yet cover appends empty slots until it is present; the table only ever
    /// appends, so the returned reference stays valid for the life of the table.
    fn slot(&self, affinity: Affinity) -> &Slot<T> {
        let index = S::index(affinity);

        // Fast path: the slot already exists. The read is lock-free, so concurrent relocations
        // scale across cores instead of contending on a shared table lock.
        if let Some(slot) = self.slots.get(index) {
            return slot;
        }

        // Slow path: the index is past the end, so append empty slots until it is covered. Size
        // toward the strategy's reported count when it exceeds the index — a hint that lets a
        // well-behaved strategy fill all its slots on the first access rather than once per
        // affinity — while always appending at least one slot per iteration so the index is reached
        // in bounded steps even if the count under-reports it. Racing callers may over-provision
        // past the index, which is harmless: a surplus slot is an unused empty cell.
        let target = (index + 1).max(S::count(affinity));
        loop {
            self.slots.push(CachePadded::new(RwLock::new(None)));
            while self.slots.count() < target {
                self.slots.push(CachePadded::new(RwLock::new(None)));
            }
            if let Some(slot) = self.slots.get(index) {
                return slot;
            }
        }
    }

    /// Replaces the data for the given affinity with the provided value.
    ///
    /// Returns the previous value if it existed, otherwise returns `None`.
    pub(crate) fn replace(&self, affinity: Affinity, value: T) -> Option<T> {
        self.slot(affinity).write().expect(NEVER_POISONED).replace(value)
    }

    /// Acquires the exclusive lock on the slot for `affinity`.
    ///
    /// The caller drives the miss path with this: re-probe the slot, materialize
    /// the value, and store it, all while holding the returned guard so that only
    /// this affinity is affected and no other thread can materialize it in the
    /// meantime.
    pub(crate) fn write(&self, affinity: Affinity) -> RwLockWriteGuard<'_, Option<T>> {
        self.slot(affinity).write().expect(NEVER_POISONED)
    }

    /// Acquires the shared lock on the slot for `affinity`.
    ///
    /// Used by tests to pin a slot so racing relocations pile up on its exclusive
    /// lock; production relocation reads through [`get_clone`](Self::get_clone).
    #[cfg(test)]
    pub(crate) fn read(&self, affinity: Affinity) -> RwLockReadGuard<'_, Option<T>> {
        self.slot(affinity).read().expect(NEVER_POISONED)
    }
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
        self.slot(affinity).read().expect(NEVER_POISONED).clone()
    }

    /// Counts how many stored entries satisfy the given predicate.
    ///
    /// The count is an estimate under concurrent relocation, matching the
    /// inherently racy nature of a strong-count query.
    pub(crate) fn count_where(&self, predicate: impl Fn(&T) -> bool) -> usize {
        // Clone every present value out from under its slot lock into a snapshot, then apply the
        // predicate afterwards, so no caller code runs while any slot lock is held. Iterating the
        // append-only table is lock-free; slots are still written concurrently during the pass, so
        // the snapshot is visited one slot at a time rather than as a consistent whole — what leaves
        // the count an estimate.
        let snapshot: Vec<T> = self
            .slots
            .iter()
            .filter_map(|(_, slot)| slot.read().expect(NEVER_POISONED).clone())
            .collect();

        snapshot.iter().filter(|value| predicate(value)).count()
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

    /// Acquires the exclusive lock on the slot for `affinity`.
    pub(crate) fn write(&self, affinity: Affinity) -> RwLockWriteGuard<'_, Option<sync::Arc<T>>> {
        self.inner.write(affinity)
    }

    /// Counts published values for which `predicate` holds.
    pub(crate) fn count_where(&self, predicate: impl Fn(&sync::Arc<T>) -> bool) -> usize {
        self.inner.count_where(predicate)
    }

    /// Acquires the shared lock on the slot for `affinity`.
    #[cfg(test)]
    pub(crate) fn read(&self, affinity: Affinity) -> RwLockReadGuard<'_, Option<sync::Arc<T>>> {
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
    /// the table's grow-to-fit path when an index lands past the reported count.
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
    fn strategy_index_beyond_count_grows_table() {
        let affinity = pinned_affinities(&[1])[0];
        let table = SlotTable::<i32, InconsistentStrategy>::new();

        // Index 1 lands past the reported count of 1, so the table must grow to reach it rather
        // than reject the access.
        assert_eq!(table.replace(affinity, 7), None);
        assert_eq!(table.get_clone(affinity), Some(7));
    }

    /// A `Strategy` that maps to the processor index while under-reporting its count as one. Exists
    /// to force the table to reallocate its vector on a later, higher-indexed access.
    struct ProcessorIndexUnderCounted;

    impl Strategy for ProcessorIndexUnderCounted {
        fn index(affinity: crate::affinity::Affinity) -> usize {
            affinity.processor_index()
        }

        fn count(_affinity: crate::affinity::Affinity) -> usize {
            1
        }
    }

    #[test]
    fn slot_reference_survives_growth() {
        // The first access sizes the vector to one slot; a later access to a far higher index grows
        // and reallocates it. A reference into the first slot, taken before that growth, must stay
        // valid afterwards — the invariant the boxed slots exist to uphold.
        let affinities = pinned_affinities(&[8]);
        let table = SlotTable::<i32, ProcessorIndexUnderCounted>::new();

        assert_eq!(table.replace(affinities[0], 100), None);
        let low = table.slot(affinities[0]);

        // Access a far higher index, reallocating the vector while `low` is held.
        assert_eq!(table.replace(affinities[7], 200), None);

        assert_eq!(*low.read().expect("slot lock is never poisoned"), Some(100));
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
