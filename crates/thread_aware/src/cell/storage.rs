// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Primitives for thread-aware data storage.

use std::fmt::{self, Debug};
use std::hash::Hash;
use std::sync;

use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use thread_aware_core::{Owner, Thread};

/// Covers typical worker or NUMA partition counts without allocating for unusually large runtimes.
///
/// Raising this trades more eager allocation in every `Storage` for less frequent map growth.
const DEFAULT_PARTITION_CAPACITY: usize = 32;

/// Maps threads into strategy partitions for storage.
///
/// A strategy names the partition a [`Thread`] belongs to. Every thread mapping to the same key
/// shares one value inside a thread-aware [`Arc`](crate::Arc); threads mapping to different keys get
/// independently materialized values.
///
/// Keys are looked up rather than indexed, so a strategy does not have to know how many partitions
/// exist, and the ids it reads need not be dense or enumerable.
///
/// This trait is sealed: it can be named as a bound, but only this crate can implement it. The
/// implementations are [`PerThread`](crate::PerThread), [`PerNumaNode`](crate::PerNumaNode) and
/// [`PerProcess`](crate::PerProcess).
pub trait Strategy: sealed::Sealed {
    /// Identifies one strategy partition.
    type Key: Clone + Eq + Hash + Debug + Send + Sync + 'static;

    /// Whether every thread maps to the same partition.
    ///
    /// Relocation uses this to recognize that a carried value provably belongs to the destination
    /// partition even when the source is unknown, seeding the partition with that value instead of
    /// materializing a fresh one. Leave it `false` unless [`key`](Self::key) is constant.
    ///
    /// Only a strategy that is *always* single-partition may set this. A strategy that merely
    /// happens to yield one key on some machine — [`PerThread`](crate::PerThread) on a
    /// single-threaded process, say — must not, because the storage cannot tell that from a
    /// partition it has yet to see.
    const SINGLE_PARTITION: bool = false;

    /// Returns the partition key for `thread`.
    fn key(thread: &Thread) -> Self::Key;
}

pub(crate) mod sealed {
    /// Closes [`Strategy`](super::Strategy) to outside implementations.
    pub trait Sealed {}
}

/// Strategy-partitioned storage shared by every clone of an [`Arc`](crate::Arc).
///
/// A relocation into a thread publishes a value for that thread's strategy partition; later
/// relocations into any thread mapping to the same key read it back. This can also be built directly
/// and populated with [`insert`](Self::insert), then handed to
/// [`Arc::from_storage`](crate::Arc::from_storage) to produce an `Arc` backed by values prepared for
/// specific strategy partitions.
///
/// Partitions are keyed rather than indexed, so every thread is addressable: there is no fixed
/// coordinate space to fall outside of, no sizing step, and therefore no out-of-range access to
/// reject. Neither [`insert`](Self::insert) nor [`get`](Self::get) panics, and relocation into a
/// thread this storage has not seen before is an ordinary miss rather than a degraded path.
///
/// Storage binds to the first runtime owner that populates it. Calls naming a thread from another
/// owner cannot read or publish partition values. Published values remain until the storage is
/// dropped, so `PerThread` storage is best suited to stable worker sets.
pub struct Storage<T: ?Sized, S: Strategy> {
    owner: sync::OnceLock<Owner>,
    values: DashMap<S::Key, sync::Arc<sync::OnceLock<sync::Arc<T>>>>,
}

impl<T: ?Sized, S: Strategy> Storage<T, S> {
    /// Creates an empty storage, with no strategy partition populated.
    ///
    /// Not `const`: the partition map allocates, so a storage cannot live in a `static`. Build one
    /// at run time and share it through an [`Arc`](std::sync::Arc), which is what
    /// [`Arc::from_storage`](crate::Arc::from_storage) expects.
    #[must_use]
    pub fn new() -> Self {
        Self {
            owner: sync::OnceLock::new(),
            values: DashMap::with_capacity(DEFAULT_PARTITION_CAPACITY),
        }
    }

    /// Binds this storage to `owner`, or verifies that it is already bound to the same owner.
    pub(crate) fn bind_owner(&self, owner: &Owner) -> bool {
        self.owner.get_or_init(|| owner.clone()) == owner
    }

    /// Publishes the value for `thread`'s strategy partition if it is still empty.
    ///
    /// Each strategy partition is written at most once, so a value is stored only when the partition
    /// was empty: `Ok(())` is returned in that case. When the partition already holds a value, the
    /// passed-in `value` is handed back as `Err(value)`. This mirrors
    /// [`OnceLock::set`](std::sync::OnceLock::set) rather than the previous-value semantics of
    /// [`HashMap::insert`](std::collections::HashMap::insert).
    ///
    /// # Errors
    ///
    /// Returns `Err(value)`, handing the passed-in `value` back unchanged, when the strategy
    /// partition selected by `thread` already holds a value or the storage belongs to another
    /// runtime owner.
    #[inline]
    pub fn insert(&self, thread: &Thread, value: sync::Arc<T>) -> Result<(), sync::Arc<T>> {
        if !self.bind_owner(thread.owner()) {
            return Err(value);
        }

        let cell = match self.values.entry(S::key(thread)) {
            Entry::Occupied(occupied) => sync::Arc::clone(occupied.get()),
            Entry::Vacant(vacant) => {
                let cell = sync::Arc::new(sync::OnceLock::new());
                vacant.insert(sync::Arc::clone(&cell));
                cell
            }
        };

        cell.set(value)
    }

    /// Looks up a partition's initialization cell without retaining the map shard guard.
    fn cell(&self, thread: &Thread) -> Option<sync::Arc<sync::OnceLock<sync::Arc<T>>>> {
        self.values.get(&S::key(thread)).map(|cell| sync::Arc::clone(cell.value()))
    }

    /// Gets or creates a partition's initialization cell without retaining the map shard guard.
    fn get_or_insert_cell(&self, thread: &Thread) -> sync::Arc<sync::OnceLock<sync::Arc<T>>> {
        match self.values.entry(S::key(thread)) {
            Entry::Occupied(occupied) => sync::Arc::clone(occupied.get()),
            Entry::Vacant(vacant) => {
                let cell = sync::Arc::new(sync::OnceLock::new());
                vacant.insert(sync::Arc::clone(&cell));
                cell
            }
        }
    }

    /// Returns a clone of the value published for `thread`'s strategy partition, if any.
    ///
    /// Returns `None` when the partition is empty or the storage belongs to another runtime owner.
    #[inline]
    #[must_use]
    pub fn get(&self, thread: &Thread) -> Option<sync::Arc<T>> {
        if self.owner.get().is_some_and(|owner| owner != thread.owner()) {
            return None;
        }

        self.cell(thread).and_then(|cell| cell.get().map(sync::Arc::clone))
    }

    /// Returns the value for `thread`'s partition, materializing it with `make` if it is empty.
    ///
    /// `make` runs at most once per partition: a partition-local [`OnceLock`](sync::OnceLock)
    /// coordinates concurrent relocations, which all observe the single published value.
    ///
    /// Callers of the public constructors that accept a factory inherit two initialization
    /// constraints:
    ///
    /// * `make` must not reenter the same partition. Re-entering it deadlocks rather than recursing.
    /// * The map shard guard is released before `make` runs, so initialization of one partition does
    ///   not hold up another partition merely because their keys share a shard.
    ///
    /// A panic in `make` propagates and leaves the partition empty for the next relocation to retry.
    pub(crate) fn get_or_insert_with(&self, thread: &Thread, make: impl FnOnce() -> sync::Arc<T>) -> Option<sync::Arc<T>> {
        if !self.bind_owner(thread.owner()) {
            return None;
        }

        let cell = self.get_or_insert_cell(thread);
        Some(sync::Arc::clone(cell.get_or_init(make)))
    }

    /// Counts published values for which `predicate` holds.
    ///
    /// The count is an estimate under concurrent relocation, matching the inherently racy nature of
    /// a strong-count query.
    pub(crate) fn count_where(&self, predicate: impl Fn(&sync::Arc<T>) -> bool) -> usize {
        self.values
            .iter()
            .filter(|entry| entry.value().get().is_some_and(&predicate))
            .count()
    }
}

impl<T: ?Sized, S: Strategy> Default for Storage<T, S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: ?Sized, S: Strategy> Debug for Storage<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Storage")
            .field("owner", &self.owner.get())
            .field("partitions", &self.values.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use super::{DEFAULT_PARTITION_CAPACITY, Storage};
    use crate::PerThread;
    use crate::thread::ThreadBuilder;

    #[test]
    fn storage_starts_with_default_capacity() {
        let storage = Storage::<u32, PerThread>::new();

        assert!(storage.values.capacity() >= DEFAULT_PARTITION_CAPACITY);
    }

    #[test]
    fn insert_publishes_once_per_key() {
        let thread = ThreadBuilder::default().build(thread::current().id());
        let storage = Storage::<u32, PerThread>::new();
        let first = Arc::new(1);
        let second = Arc::new(2);

        assert_eq!(storage.insert(&thread, Arc::clone(&first)), Ok(()));
        assert_eq!(storage.insert(&thread, Arc::clone(&second)), Err(second));
        assert!(Arc::ptr_eq(&storage.get(&thread).unwrap(), &first));
    }

    #[test]
    fn storage_rejects_threads_from_another_owner() {
        let first = ThreadBuilder::default().build(thread::current().id());
        let second = ThreadBuilder::default().build(thread::current().id());
        let storage = Storage::<u32, PerThread>::new();
        let rejected = Arc::new(2);

        storage.insert(&first, Arc::new(1)).unwrap();

        assert!(Arc::ptr_eq(&storage.insert(&second, Arc::clone(&rejected)).unwrap_err(), &rejected));
        assert!(storage.get(&second).is_none());
    }
}
