// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Primitives for thread-aware data storage.

use std::fmt::{self, Debug};
use std::hash::Hash;
use std::sync;

use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use rustc_hash::FxBuildHasher;
use thread_aware_core::{Owner, Thread};

/// An explicit bounded-runtime heuristic, not an empirically established partition limit.
///
/// Reserving 32 entries avoids initial growth for runtimes configured with at most 32 partitions
/// while keeping eager allocation bounded. Larger runtimes grow the map normally. Raising this
/// trades more eager allocation in every partitioned `Storage` for less frequent map growth.
const DEFAULT_PARTITION_CAPACITY: usize = 32;

type PartitionMap<T, S> = DashMap<<S as Strategy>::Key, sync::Arc<T>, FxBuildHasher>;

enum Values<T: ?Sized, S: Strategy> {
    Single(sync::OnceLock<sync::Arc<T>>),
    Partitioned(PartitionMap<T, S>),
}

pub(crate) enum InsertError<T: ?Sized> {
    Occupied(sync::Arc<T>),
    ForeignOwner(sync::Arc<T>),
}

impl<T: ?Sized> InsertError<T> {
    fn into_value(self) -> sync::Arc<T> {
        match self {
            Self::Occupied(value) | Self::ForeignOwner(value) => value,
        }
    }
}

#[cfg_attr(test, mutants::skip)] // Returning zero early is equivalent to iterating an empty map.
fn empty_partition_count<T: ?Sized, S: Strategy>(values: &PartitionMap<T, S>) -> Option<usize> {
    values.is_empty().then_some(0)
}

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
///
/// A single-partition strategy stores its value in a [`OnceLock`](std::sync::OnceLock).
/// Partitioned strategies use a [`DashMap`], because their opaque keys are not dense or enumerable.
pub struct Storage<T: ?Sized, S: Strategy> {
    owner: sync::OnceLock<Owner>,
    values: Values<T, S>,
}

impl<T: ?Sized, S: Strategy> Storage<T, S> {
    /// Creates an empty storage, with no strategy partition populated.
    ///
    /// Not `const`: the partition map allocates, so a storage cannot live in a `static`. Build one
    /// at run time and share it through an [`Arc`](std::sync::Arc), which is what
    /// [`Arc::from_storage`](crate::Arc::from_storage) expects.
    #[must_use]
    pub fn new() -> Self {
        let values = if S::SINGLE_PARTITION {
            Values::Single(sync::OnceLock::new())
        } else {
            Values::Partitioned(DashMap::with_capacity_and_hasher(DEFAULT_PARTITION_CAPACITY, FxBuildHasher))
        };

        Self {
            owner: sync::OnceLock::new(),
            values,
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
    /// runtime owner. The internal insertion path preserves those as distinct diagnostics while
    /// this public API retains its original rejected-value error type.
    #[inline]
    pub fn insert(&self, thread: &Thread, value: sync::Arc<T>) -> Result<(), sync::Arc<T>> {
        self.insert_with_reason(thread, value).map_err(InsertError::into_value)
    }

    pub(crate) fn insert_with_reason(&self, thread: &Thread, value: sync::Arc<T>) -> Result<(), InsertError<T>> {
        if !self.bind_owner(thread.owner()) {
            return Err(InsertError::ForeignOwner(value));
        }

        self.insert_key(S::key(thread), value)
    }

    pub(crate) fn insert_key(&self, key: S::Key, value: sync::Arc<T>) -> Result<(), InsertError<T>> {
        match &self.values {
            Values::Single(cell) => cell.set(value).map_err(InsertError::Occupied),
            Values::Partitioned(values) => match values.entry(key) {
                Entry::Occupied(_) => Err(InsertError::Occupied(value)),
                Entry::Vacant(vacant) => {
                    vacant.insert(value);
                    Ok(())
                }
            },
        }
    }

    pub(crate) fn owner_matches(&self, owner: &Owner) -> bool {
        self.owner.get().is_none_or(|bound| bound == owner)
    }

    pub(crate) fn get_key(&self, key: &S::Key) -> Option<sync::Arc<T>> {
        match &self.values {
            Values::Single(cell) => cell.get().map(sync::Arc::clone),
            Values::Partitioned(values) => values.get(key).map(|value| sync::Arc::clone(value.value())),
        }
    }

    /// Returns a clone of the value published for `thread`'s strategy partition, if any.
    ///
    /// Returns `None` when the partition is empty or the storage belongs to another runtime owner.
    #[inline]
    #[must_use]
    pub fn get(&self, thread: &Thread) -> Option<sync::Arc<T>> {
        if !self.owner_matches(thread.owner()) {
            return None;
        }

        self.get_key(&S::key(thread))
    }

    /// Returns the value for `key`, materializing it with `make` if it is empty.
    ///
    /// `make` runs at most once per partition: concurrent relocations all observe the single
    /// published value.
    ///
    /// For partitioned storage, `make` runs while the destination entry holds its `DashMap` shard for
    /// writing. This is an intentional first-use trade-off: materialization is rare, while retaining
    /// the guard guarantees one factory call per partition without speculative duplicate values.
    /// Keep `make` short and do not reenter this storage.
    ///
    /// A panic in `make` propagates and leaves the partition empty for the next relocation to retry.
    pub(crate) fn get_or_insert_key_with(&self, key: S::Key, make: impl FnOnce() -> sync::Arc<T>) -> sync::Arc<T> {
        match &self.values {
            Values::Single(cell) => sync::Arc::clone(cell.get_or_init(make)),
            Values::Partitioned(values) => sync::Arc::clone(values.entry(key).or_insert_with(make).value()),
        }
    }

    /// Counts published values for which `predicate` holds.
    ///
    /// The count is an estimate under concurrent relocation, matching the inherently racy nature of
    /// a strong-count query.
    pub(crate) fn count_where(&self, predicate: impl Fn(&sync::Arc<T>) -> bool) -> usize {
        match &self.values {
            Values::Single(cell) => usize::from(cell.get().is_some_and(predicate)),
            Values::Partitioned(values) => {
                empty_partition_count::<T, S>(values).unwrap_or_else(|| values.iter().filter(|entry| predicate(entry.value())).count())
            }
        }
    }

    fn len(&self) -> usize {
        match &self.values {
            Values::Single(cell) => usize::from(cell.get().is_some()),
            Values::Partitioned(values) => values.len(),
        }
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
            .field("values", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use super::{DEFAULT_PARTITION_CAPACITY, InsertError, Storage, Values};
    use crate::thread::ThreadBuilder;
    use crate::{PerProcess, PerThread};

    #[test]
    fn storage_starts_with_default_capacity() {
        let storage = Storage::<u32, PerThread>::new();

        assert!(matches!(&storage.values, Values::Partitioned(values) if values.capacity() >= DEFAULT_PARTITION_CAPACITY));
    }

    #[test]
    fn single_partition_storage_avoids_the_map() {
        let storage = Storage::<u32, PerProcess>::new();

        assert!(matches!(storage.values, Values::Single(_)));
    }

    #[test]
    fn single_partition_storage_supports_operations() {
        let thread = ThreadBuilder::default().build(thread::current().id());
        let storage = Storage::<u32, PerProcess>::new();
        let first = Arc::new(1);
        let second = Arc::new(2);

        assert_eq!(storage.insert(&thread, Arc::clone(&first)), Ok(()));
        assert_eq!(storage.insert(&thread, Arc::clone(&second)), Err(second));
        assert_eq!(storage.count_where(|value| **value == 1), 1);
        assert_eq!(storage.count_where(|value| **value == 2), 0);
        assert!(format!("{storage:?}").contains("values: 1"));
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

    #[test]
    fn insertion_reason_distinguishes_occupied_partition_from_foreign_owner() {
        let builder = ThreadBuilder::default();
        let thread = builder.build(thread::current().id());
        let foreign = ThreadBuilder::default().build(thread::current().id());
        let storage = Storage::<u32, PerThread>::new();
        storage.insert(&thread, Arc::new(1)).unwrap();

        let occupied = Arc::new(2);
        match storage.insert_with_reason(&thread, Arc::clone(&occupied)) {
            Err(InsertError::Occupied(rejected)) => assert!(Arc::ptr_eq(&rejected, &occupied)),
            Err(InsertError::ForeignOwner(_)) | Ok(()) => panic!("occupied partition must retain its diagnostic"),
        }

        let foreign_value = Arc::new(3);
        match storage.insert_with_reason(&foreign, Arc::clone(&foreign_value)) {
            Err(InsertError::ForeignOwner(rejected)) => assert!(Arc::ptr_eq(&rejected, &foreign_value)),
            Err(InsertError::Occupied(_)) | Ok(()) => panic!("foreign owner must retain its diagnostic"),
        }
    }

    #[test]
    fn debug_reports_published_value_count() {
        let builder = ThreadBuilder::default();
        let first = builder.build(thread::current().id());
        let second_id = thread::spawn(|| thread::current().id()).join().unwrap();
        let second = builder.build(second_id);
        let storage = Storage::<u32, PerThread>::new();

        storage.insert(&first, Arc::new(1)).unwrap();
        storage.insert(&second, Arc::new(2)).unwrap();

        assert!(format!("{storage:?}").contains("values: 2"));
    }
}
