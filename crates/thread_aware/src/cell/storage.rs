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

use crate::affinity::Affinity;

/// A strategy for storing data in a affinity-aware manner.
pub trait Strategy {
    /// Returns the slot index for the given affinity.
    fn index(affinity: Affinity) -> usize;

    /// Returns the total number of slots for the given affinity.
    fn count(affinity: Affinity) -> usize;
}

/// Message used when a slot lock is found poisoned by a panic in another thread.
const POISONED: &str = "storage slot lock poisoned by a panic in another thread";

/// One affinity's independently-locked, cache-line-isolated storage slot.
type Slot<T> = CachePadded<RwLock<Option<T>>>;

/// Affinity-partitioned storage: one independently-locked slot per affinity.
///
/// This is the raw slot table. [`SharedStorage`] wraps it as the handle an
/// `Arc` actually holds; the two are separate so this can be unit-tested with a
/// plain value type while the wrapper pins the stored type to `Arc<T>`.
///
/// Each affinity owns its own `RwLock`, so relocations targeting different
/// affinities never touch the same lock or the same cache line. The slots are
/// cache-line padded so that neighboring affinities do not share a line.
///
/// The slot array is sized once, on first use, to `S::count(affinity)` — a value
/// fixed for the process lifetime — so there is no growth path and therefore no
/// table-wide lock guarding it. After initialization, reaching a slot is a plain
/// atomic load of the `OnceLock` pointer, which stays resident and shared in
/// every core's cache and generates no coherence traffic.
#[derive(Debug)]
pub(crate) struct Storage<T, S: Strategy> {
    slots: OnceLock<Box<[Slot<T>]>>,
    _marker: PhantomData<S>,
}

impl<T, S: Strategy> Storage<T, S> {
    /// Creates a new empty `Storage` instance.
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

    /// Returns the lock guarding the slot for `affinity`.
    fn slot(&self, affinity: Affinity) -> &RwLock<Option<T>> {
        &self.slots(affinity)[S::index(affinity)]
    }

    /// Replaces the data for the given affinity with the provided value.
    ///
    /// Returns the previous value if it existed, otherwise returns `None`.
    #[cfg(test)]
    pub(crate) fn replace(&self, affinity: Affinity, value: T) -> Option<T> {
        self.slot(affinity).write().expect(POISONED).replace(value)
    }

    /// Acquires the exclusive lock on the slot for `affinity`.
    ///
    /// The caller drives the miss path with this: re-probe the slot, materialize
    /// the value, and store it, all while holding the returned guard so that only
    /// this affinity is affected and no other thread can materialize it in the
    /// meantime.
    pub(crate) fn write(&self, affinity: Affinity) -> RwLockWriteGuard<'_, Option<T>> {
        self.slot(affinity).write().expect(POISONED)
    }

    /// Acquires the shared lock on the slot for `affinity`.
    ///
    /// Used by tests to pin a slot so racing relocations pile up on its exclusive
    /// lock; production relocation reads through [`get_clone`](Self::get_clone).
    #[cfg(test)]
    pub(crate) fn read(&self, affinity: Affinity) -> RwLockReadGuard<'_, Option<T>> {
        self.slot(affinity).read().expect(POISONED)
    }
}

impl<T, S: Strategy> Default for Storage<T, S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, S: Strategy> Storage<T, S>
where
    T: Clone,
{
    /// Clone and gets the data for the given affinity if it exists.
    /// Returns `None` if the data does not exist for that affinity.
    #[must_use]
    pub(crate) fn get_clone(&self, affinity: Affinity) -> Option<T> {
        self.slot(affinity).read().expect(POISONED).clone()
    }
}

impl<T, S: Strategy> Storage<T, S> {
    /// Counts how many stored entries satisfy the given predicate.
    ///
    /// The slots are read one at a time rather than under a single consistent
    /// snapshot, so the count is an estimate under concurrent relocation — which
    /// matches the inherently racy nature of a strong-count query.
    pub(crate) fn count_where(&self, predicate: impl Fn(&T) -> bool) -> usize {
        match self.slots.get() {
            None => 0,
            Some(slots) => slots
                .iter()
                .filter(|slot| slot.read().expect(POISONED).as_ref().is_some_and(&predicate))
                .count(),
        }
    }
}

/// Opaque handle to an `Arc`'s per-affinity storage, shared by its clones.
///
/// Hides the per-slot locking so the representation can change without breaking
/// callers. `T` is `?Sized` because a slot stores an `Arc<T>`, which is a sized
/// value even when `T` is not.
#[derive(Debug)]
pub struct SharedStorage<T: ?Sized, S: Strategy> {
    inner: Storage<sync::Arc<T>, S>,
}

impl<T: ?Sized, S: Strategy> SharedStorage<T, S> {
    /// Creates an empty handle, with no affinity populated.
    pub(crate) const fn new() -> Self {
        Self { inner: Storage::new() }
    }

    /// Clones out the value published for `affinity`, if any.
    pub(crate) fn get_clone(&self, affinity: Affinity) -> Option<sync::Arc<T>> {
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

    /// Publishes `value` for `affinity`, returning any previous value.
    #[cfg(test)]
    pub(crate) fn replace(&self, affinity: Affinity, value: sync::Arc<T>) -> Option<sync::Arc<T>> {
        self.inner.replace(affinity, value)
    }
}

#[cfg(test)]
mod tests {
    use crate::affinity::pinned_affinities;
    use crate::storage::{Storage, Strategy};
    use crate::{PerCore, PerNuma, PerProcess};

    #[test]
    fn replace_returns_previous_value() {
        let affinities = pinned_affinities(&[1]);
        let storage = Storage::<String, PerCore>::default();
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

        let storage = Storage::<String, PerCore>::default();
        let affinity = affinities[0];

        assert!(storage.get_clone(affinity).is_none());

        storage.replace(affinity, "Hello".to_string());
        assert_eq!(storage.get_clone(affinity), Some("Hello".to_string()));
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
        let storage = Storage::<String, PerCore>::default();
        let affinity = affinities[0];

        // Verify the default storage is empty (no data for any affinity)
        assert!(storage.get_clone(affinity).is_none());

        // Verify it works the same as Storage::new()
        storage.replace(affinity, "test".to_string());
        assert_eq!(storage.get_clone(affinity), Some("test".to_string()));
    }
}
