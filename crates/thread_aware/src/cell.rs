// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod per_core;
mod per_numa;
mod storage;

use std::cmp::Ordering;
use std::hash::Hasher;
use std::ops::Deref;
use std::sync::{Arc, RwLock};

pub use per_core::PerCore;
pub use per_numa::PerNuma;
pub use storage::{PerAppStorage, PerCoreStorage, PerCoreStrategy, PerNumaStorage, PerNumaStrategy, PerProcessStrategy, Storage, Strategy};

use crate::closure::ErasedClosureOnce;
use crate::{MemoryAffinity, PinnedAffinity, RelocateFnOnce, ThreadAware};

/// Transferable reference counted type.
///
/// This type works like a per-affinity (per-thread) [`Arc`]. Each affinity gets a unique value that is shared by clones
/// of the `Trc`, but the [`ThreadAware`] implementation ensures that when moving to another affinity, the resulting
/// `Trc` will point to the value in the destination affinity. See [`with_closure`](`Trc::with_closure`) for information on constructing instances.
///
/// `ThreadAware` of different clones of the `Trc` result in "deduplication" in the destination affinity. The following
/// example demonstrates this using the counter implemented in the documentation for the [`ThreadAware`] trait.
///
/// ```rust
/// # use thread_aware::{MemoryAffinity, ThreadAware, PerCore, relocate_once, create_manual_affinities};
/// # use std::sync::atomic::{AtomicI32, Ordering};
/// # use std::sync::Arc;
/// # let affinities = create_manual_affinities(&[2]);
/// # let affinity1 = affinities[0];
/// # let affinity2 = affinities[1];
/// # #[derive(Clone)]
/// # struct Counter {
/// #     value: Arc<AtomicI32>,
/// # }
/// #
/// # impl Counter {
/// #     fn new() -> Self {
/// #         Self {
/// #             value: Arc::new(AtomicI32::new(0)),
/// #         }
/// #     }
/// #
/// #     fn increment_by(&self, v: i32) {
/// #         self.value.fetch_add(v, Ordering::AcqRel);
/// #     }
/// #
/// #     fn value(&self) -> i32 {
/// #         self.value.load(Ordering::Acquire)
/// #     }
/// # }
/// #
/// # impl ThreadAware for Counter {
/// #     fn relocated(self, source: MemoryAffinity, destination: MemoryAffinity) -> Self {
/// #         Self {
/// #             // Initialize a new value in the destination affinity independent
/// #             // of the source affinity.
/// #             value: Arc::new(AtomicI32::new(0)),
/// #         }
/// #     }
/// # }
///
/// let trc_affinity1 = PerCore::new(Counter::new);
/// let trc_affinity1_clone = trc_affinity1.clone();
///
/// trc_affinity1.increment_by(42);
/// assert_eq!(trc_affinity1.value(), 42);
///
/// let trc_affinity2 = trc_affinity1.relocated(affinity1, affinity2);
/// assert_eq!(trc_affinity2.value(), 0);
/// assert_eq!(trc_affinity1_clone.value(), 42);
///
/// trc_affinity2.increment_by(11);
/// let trc_affinity2_clone = trc_affinity1_clone.relocated(affinity1, affinity2);
/// assert_eq!(trc_affinity2_clone.value(), 11);
/// ```
#[derive(Debug)]
pub(crate) struct Trc<T, S: Strategy> {
    storage: Arc<RwLock<Storage<Arc<T>, S>>>,
    value: Arc<T>,
    factory: Factory<T>,
}

impl<T: PartialEq, S: Strategy> PartialEq for Trc<T, S> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T: Eq, S: Strategy> Eq for Trc<T, S> {}

impl<T: std::hash::Hash, S: Strategy> std::hash::Hash for Trc<T, S> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl<T: Ord, S: Strategy> Ord for Trc<T, S> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.value.cmp(&other.value)
    }
}

impl<T: PartialOrd, S: Strategy> PartialOrd for Trc<T, S> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.value.partial_cmp(&other.value)
    }
}

impl<T, S: Strategy> Clone for Trc<T, S> {
    fn clone(&self) -> Self {
        Self {
            storage: Arc::clone(&self.storage),
            value: Arc::clone(&self.value),
            factory: self.factory.clone(),
        }
    }
}

type DataFn<T> = fn(&T, MemoryAffinity, MemoryAffinity) -> T;

#[derive(Debug)]
enum Factory<T> {
    /// An external closure was provided to create the data.
    Closure(Arc<ErasedClosureOnce<T>>, Option<MemoryAffinity>),

    /// The data is `ThreadAware` + Clone and will be cloned and transferred.
    Data(DataFn<T>),

    Manual,
}

impl<T> Clone for Factory<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Closure(closure, closure_source) => Self::Closure(Arc::clone(closure), *closure_source),
            Self::Data(data_fn) => Self::Data(*data_fn),
            Self::Manual => Self::Manual,
        }
    }
}

impl<T, S: Strategy> Deref for Trc<T, S> {
    type Target = Arc<T>;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T, S: Strategy> Trc<T, S>
where
    T: ThreadAware + Clone + 'static + Send,
{
    /// Creates a new `Trc` with the given value.
    ///
    /// The value must implement `ThreadAware` and `Clone`. When transferring to another affinity
    /// which doesn't yet contain a value, a new value is created by cloning the value in current
    /// affinity and transferring it to the new affinity.
    ///
    /// For example, the counter type we implemented in the documentation for [`ThreadAware`] trait
    /// can be used with new:
    ///
    /// ```rust
    /// # use thread_aware::{ThreadAware, MemoryAffinity, PerCore, relocate_once, create_manual_affinities};
    /// # use std::sync::atomic::{AtomicI32, Ordering};
    /// # use std::sync::Arc;
    /// # let affinities = create_manual_affinities(&[2]);
    /// # let affinity1 = affinities[0];
    /// # let affinity2 = affinities[1];
    /// # #[derive(Clone)]
    /// # struct Counter {
    /// #     value: Arc<AtomicI32>,
    /// # }
    /// #
    /// # impl Counter {
    /// #     fn new() -> Self {
    /// #         Self {
    /// #             value: Arc::new(AtomicI32::new(0)),
    /// #         }
    /// #     }
    /// #
    /// #     fn increment_by(&self, v: i32) {
    /// #         self.value.fetch_add(v, Ordering::AcqRel);
    /// #     }
    /// #
    /// #     fn value(&self) -> i32 {
    /// #         self.value.load(Ordering::Acquire)
    /// #     }
    /// # }
    /// #
    /// # impl ThreadAware for Counter {
    /// #     fn relocated(self, source: MemoryAffinity, destination: MemoryAffinity) -> Self {
    /// #         Self {
    /// #             // Initialize a new value in the destination affinity independent
    /// #             // of the source affinity.
    /// #             value: Arc::new(AtomicI32::new(0)),
    /// #         }
    /// #     }
    /// # }
    ///
    /// let trc = PerCore::new(Counter::new);
    /// let trc_clone = trc.clone();
    /// trc.increment_by(42);
    /// assert_eq!(trc.value(), 42);
    /// assert_eq!(trc_clone.value(), 42);
    /// ```
    #[cfg(test)]
    pub(crate) fn new(value: T) -> Self {
        let value = Arc::new(value);

        Self {
            storage: Arc::new(RwLock::new(storage::Storage::new())),
            value,
            factory: Factory::Data(|data: &T, source, destination| {
                let data = data.clone();
                data.relocated(source, destination)
            }),
        }
    }
}

impl<T, S: Strategy> Trc<T, S>
where
    T: Clone + 'static + Send,
{
    /// Creates a new `Trc` with the given value.
    ///
    /// The value must implement `ThreadAware` and `Clone`. When transferring to another affinity
    /// which doesn't yet contain a value, a new value is created by cloning the value in current
    /// affinity and transferring it to the new affinity.
    ///
    /// For example, the counter type we implemented in the documentation for [`ThreadAware`] trait
    /// can be used with new:
    ///
    /// ```rust
    /// # use thread_aware::{ThreadAware, MemoryAffinity, PerCore, relocate_once, create_manual_affinities};
    /// # use std::sync::atomic::{AtomicI32, Ordering};
    /// # use std::sync::Arc;
    /// # let affinities = create_manual_affinities(&[2]);
    /// # let affinity1 = affinities[0];
    /// # let affinity2 = affinities[1];
    /// # #[derive(Clone)]
    /// # struct Counter {
    /// #     value: Arc<AtomicI32>,
    /// # }
    /// #
    /// # impl Counter {
    /// #     fn new() -> Self {
    /// #         Self {
    /// #             value: Arc::new(AtomicI32::new(0)),
    /// #         }
    /// #     }
    /// #
    /// #     fn increment_by(&self, v: i32) {
    /// #         self.value.fetch_add(v, Ordering::AcqRel);
    /// #     }
    /// #
    /// #     fn value(&self) -> i32 {
    /// #         self.value.load(Ordering::Acquire)
    /// #     }
    /// # }
    /// #
    /// # impl ThreadAware for Counter {
    /// #     fn relocated(self, source: MemoryAffinity, destination: MemoryAffinity) -> Self {
    /// #         Self {
    /// #             // Initialize a new value in the destination affinity independent
    /// #             // of the source affinity.
    /// #             value: Arc::new(AtomicI32::new(0)),
    /// #         }
    /// #     }
    /// # }
    ///
    /// let trc = PerCore::new(Counter::new);
    /// let trc_clone = trc.clone();
    /// trc.increment_by(42);
    /// assert_eq!(trc.value(), 42);
    /// assert_eq!(trc_clone.value(), 42);
    /// ```
    pub fn from_unaware(value: T) -> Self {
        let value = Arc::new(value);

        Self {
            storage: Arc::new(RwLock::new(storage::Storage::new())),
            value,
            factory: Factory::Data(|data: &T, _source, _destination| data.clone()),
        }
    }
}

impl<T, S: Strategy> Trc<T, S>
where
    T: 'static,
{
    /// Creates a new `Trc` with a closure that will be called once per-affinity to create the inner value.
    ///
    /// The closure only gets called once for each affinity, and it's called only when a Trc is actually transferred
    /// to another affinity. The closure is a [`RelocateFnOnce`] to ensure it captures only values that are safe to
    /// transfer themselves.
    ///
    /// This function can be used to create a `Trc` of a type that itself doesn't implement [`ThreadAware`] because
    /// we can ensure that each affinity will get its own, independenty-initialized value:
    ///
    /// ```rust
    /// # use std::sync::{Arc, Mutex};
    /// # use thread_aware::{PerCore, relocate_once};
    /// struct MyStruct {
    ///     inner: Arc<Mutex<i32>>,
    /// }
    ///
    /// impl MyStruct {
    ///     fn new() -> Self {
    ///         Self {
    ///             inner: Arc::new(Mutex::new(0)),
    ///         }
    ///     }
    /// }
    ///
    /// let trc = PerCore::new_with((), |_| MyStruct::new());
    /// ```
    ///
    /// The constructor can depend on other values that implement [`ThreadAware`] (this example uses the Counter
    /// defined in [`ThreadAware`] documentation):
    ///
    /// ```rust
    /// # use thread_aware::{ThreadAware, MemoryAffinity, PerCore, relocate_once, create_manual_affinities};
    /// # use std::sync::atomic::{AtomicI32, Ordering};
    /// # use std::sync::Arc;
    /// # let affinities = create_manual_affinities(&[2]);
    /// # let affinity1 = affinities[0];
    /// # let affinity2 = affinities[1];
    /// # #[derive(Clone)]
    /// # struct Counter {
    /// #     value: Arc<AtomicI32>,
    /// # }
    /// #
    /// # impl Counter {
    /// #     fn new() -> Self {
    /// #         Self {
    /// #             value: Arc::new(AtomicI32::new(0)),
    /// #         }
    /// #     }
    /// #
    /// #     fn increment_by(&self, v: i32) {
    /// #         self.value.fetch_add(v, Ordering::AcqRel);
    /// #     }
    /// #
    /// #     fn value(&self) -> i32 {
    /// #         self.value.load(Ordering::Acquire)
    /// #     }
    /// # }
    /// #
    /// # impl ThreadAware for Counter {
    /// #     fn relocated(self, source: MemoryAffinity, destination: MemoryAffinity) -> Self {
    /// #         Self {
    /// #             // Initialize a new value in the destination affinity independent
    /// #             // of the source affinity.
    /// #             value: Arc::new(AtomicI32::new(0)),
    /// #         }
    /// #     }
    /// # }
    ///
    /// struct MyStruct;
    ///
    /// impl MyStruct {
    ///     fn new(value: i32) -> Self {
    ///         Self
    ///     }
    /// }
    ///
    /// let counter = Counter::new();
    /// let trc = PerCore::new_with(counter, |counter| {
    ///     MyStruct::new(counter.value())
    /// });
    /// ```
    pub fn with_closure<F>(closure: F) -> Self
    where
        F: RelocateFnOnce<T> + Clone + ThreadAware + 'static + Send + Sync,
    {
        let value = Arc::new(closure.clone().call_once());

        Self {
            storage: Arc::new(RwLock::new(storage::Storage::new())),
            value,
            factory: Factory::Closure(Arc::new(ErasedClosureOnce::new(closure)), None), // We don't know the source affinity at this point
        }
    }

    /// Creates a new `Trc` from the given storage and the current affinity.
    ///
    /// If the resulting `Trc` is transferred to an affinity which does not have data in the storage,
    /// it will behave like an `Arc`.
    ///
    /// # Panics
    /// This may panic if the storage does not contain data for the current affinity.
    pub fn from_storage(storage: Arc<RwLock<Storage<Arc<T>, S>>>, current_affinity: PinnedAffinity) -> Self {
        let value = storage
            .read()
            .expect("Failed to acquire read lock")
            .get_clone(current_affinity)
            .expect("No data found for the current affinity");

        Self {
            storage,
            value,
            factory: Factory::Manual,
        }
    }
}

impl<T, S: Strategy> Trc<T, S> {
    /// Converts the `Trc<T>` into an `Arc<T>`.
    #[must_use]
    pub fn into_arc(self) -> Arc<T> {
        self.value
    }
}

impl<T, S: Strategy> ThreadAware for Trc<T, S> {
    fn relocated(self, source: MemoryAffinity, destination: MemoryAffinity) -> Self {
        let value = match destination {
            MemoryAffinity::Unknown => None,
            MemoryAffinity::Pinned(destination) => self.storage.read().expect("Failed to acquire read lock").get_clone(destination),
        };

        let (value, new_factory) = if let Some(value) = value {
            (value, self.factory)
        } else {
            // We need to transfer or recreate the data
            let (data, factory) = match &self.factory {
                // We can use the closure to create new data
                Factory::Closure(factory, factory_source_affinity) => {
                    let factory_clone = (**factory).clone();

                    // In case factory source is stored in factory, use that - it means we already transferred the factory
                    // once, so we know the original source affinity. Otherwise, use source as that means this is the first
                    // time we're transferring the Trc, so source is the source affinity of the factory as well.
                    let factory_source = factory_source_affinity.unwrap_or(source);

                    (
                        Arc::new(factory_clone.relocated(factory_source, destination).call_once()),
                        Factory::Closure(Arc::clone(factory), Some(factory_source)),
                    )
                }

                // We can clone and transfer the data
                Factory::Data(factory) => (Arc::new(factory(&self.value, source, destination)), self.factory),

                Factory::Manual => {
                    // If we are in manual mode, we just clone the data
                    // This effectively makes it behave like `Arc<T>`
                    (Arc::clone(&self.value), self.factory)
                }
            };

            let value = data;

            if let MemoryAffinity::Pinned(destination) = destination {
                let old_data = self
                    .storage
                    .write()
                    .expect("Failed to acquire write lock")
                    .replace(destination, Arc::<T>::clone(&value));

                assert!(old_data.is_none(), "Data already exists for the destination affinity");
            }


            (value, factory)
        };

        if let MemoryAffinity::Pinned(source) = source {
            self.storage
                .write()
                .expect("Failed to acquire write lock")
                .replace(source, self.value);
        }

        Self {
            storage: self.storage,
            value,
            factory: new_factory,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Unaware, relocate};

    // We don't use PerCore here because we want to test the raw Trc itself.
    type Trc<T> = super::Trc<T, super::storage::PerCoreStrategy>;

    #[test]
    fn test_partialeq() {
        let value1 = Trc::new(42);
        let value2 = Trc::new(42);
        let value3 = Trc::new(43);

        assert_eq!(value1, value2);
        assert_ne!(value1, value3);
    }

    #[test]
    fn test_hash() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let value1 = Trc::new(42);
        let value2 = Trc::new(42);
        let value3 = Trc::new(43);

        let mut hasher1 = DefaultHasher::new();
        value1.hash(&mut hasher1);
        let hash1 = hasher1.finish();

        let mut hasher2 = DefaultHasher::new();
        value2.hash(&mut hasher2);
        let hash2 = hasher2.finish();

        let mut hasher3 = DefaultHasher::new();
        value3.hash(&mut hasher3);
        let hash3 = hasher3.finish();

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_partialord() {
        let value1 = Trc::new(42);
        let value2 = Trc::new(43);

        assert!(value1 < value2);
        assert!(value2 > value1);
    }

    #[test]
    fn test_ord() {
        let value1 = Trc::new(42);
        let value2 = Trc::new(43);
        let value3 = Trc::new(42);

        assert_eq!(value1.cmp(&value2), std::cmp::Ordering::Less);
        assert_eq!(value2.cmp(&value1), std::cmp::Ordering::Greater);
        assert_eq!(value1.cmp(&value3), std::cmp::Ordering::Equal);
    }

    #[allow(clippy::redundant_clone, reason = "Testing clone behavior")]
    #[test]
    fn test_trc_clone() {
        let value = Trc::new(42);
        let cloned_value = value.clone();
        assert_eq!(*value, *cloned_value);
    }

    #[test]
    fn test_into_arc() {
        let trc = Trc::with_closure(relocate((), |()| 42));
        let _arc = trc.into_arc();

        let trc = Trc::new(42);
        let _arc = trc.into_arc();

        let trc = Trc::new(Unaware(42));
        let _arc = trc.into_arc();
    }

    #[test]
    fn test_from() {
        let trc = Trc::with_closure(relocate((), |()| 42));
        let _arc = trc.into_arc();

        let trc = Trc::new(42);
        let _arc = trc.into_arc();

        let trc = Trc::new(Unaware(42));
        let _arc = trc.into_arc().into_arc();
    }

    #[test]
    fn test_trc_relocated_with_factory_data() {
        use crate::{ThreadAware, create_manual_memory_affinities};

        let affinities = create_manual_memory_affinities(&[2]);
        let affinity1 = affinities[0];
        let affinity2 = affinities[1];

        // Create a Trc with a value that implements ThreadAware + Clone
        // This will use Factory::Data
        let trc_affinity1 = Trc::new(42);
        assert_eq!(**trc_affinity1, 42);

        // Relocate to another affinity, which should trigger Factory::Data path
        // and call data.relocated(source, destination) at line 219
        let trc_affinity2 = trc_affinity1.relocated(affinity1, affinity2);
        assert_eq!(**trc_affinity2, 42);
    }

    #[test]
    fn test_trc_relocated_reuses_existing_value() {
        use crate::{ThreadAware, create_manual_memory_affinities};

        let affinities = create_manual_memory_affinities(&[2]);
        let affinity1 = affinities[0];
        let affinity2 = affinities[1];

        // Create a Trc and clone it before relocating
        let trc1 = Trc::new(42);
        let trc2 = trc1.clone();

        // Relocate the first Trc to affinity2
        // This creates a new value in the destination storage
        let trc1_relocated = trc1.relocated(affinity1, affinity2);
        assert_eq!(**trc1_relocated, 42);

        // Relocate the cloned Trc to the same destination
        // This should hit line 428 where it finds the existing value in storage
        // and reuses it instead of creating a new one
        let trc2_relocated = trc2.relocated(affinity1, affinity2);
        assert_eq!(**trc2_relocated, 42);

        // Both relocated Trcs should point to the same Arc (deduplication)
        assert!(std::sync::Arc::ptr_eq(&*trc1_relocated, &*trc2_relocated));
    }

    #[test]
    fn test_from_storage() {
        use crate::create_manual_pinned_affinities;
        use std::sync::{Arc, RwLock};

        let affinities = create_manual_pinned_affinities(&[2]);
        let affinity1 = affinities[0];

        // Create a storage and populate it with a value for affinity1
        let mut storage = super::storage::Storage::new();
        let value = Arc::new(100);
        storage.replace(affinity1, Arc::clone(&value));

        let storage_arc = Arc::new(RwLock::new(storage));

        // Create a Trc from the storage at affinity1
        // This should call line 400 (from_storage method)
        let trc = Trc::from_storage(Arc::clone(&storage_arc), affinity1);

        // Verify the value is correct
        assert_eq!(**trc, 100);

        // Verify it points to the same Arc we put in storage
        assert!(Arc::ptr_eq(&*trc, &value));
    }

    #[test]
    fn test_factory_clone_with_data() {
        // This test covers line 142: Self::Data(data_fn) => Self::Data(*data_fn)
        // We create a Trc with Factory::Data, clone it, and verify the factory is properly cloned

        use crate::{ThreadAware, create_manual_memory_affinities};

        let affinities = create_manual_memory_affinities(&[2]);
        let affinity1 = affinities[0];
        let affinity2 = affinities[1];

        // Create a Trc with a value that uses Factory::Data (ThreadAware + Clone)
        let trc1 = Trc::new(42);

        // Clone the Trc - this should exercise line 142 in the Factory::clone method
        let trc2 = trc1.clone();

        // Verify both Trcs work correctly
        assert_eq!(**trc1, 42);
        assert_eq!(**trc2, 42);

        // Relocate both to verify the cloned factory works properly
        let trc1_relocated = trc1.relocated(affinity1, affinity2);
        let trc2_relocated = trc2.relocated(affinity1, affinity2);

        assert_eq!(**trc1_relocated, 42);
        assert_eq!(**trc2_relocated, 42);
    }

    #[test]
    fn test_factory_clone_with_closure() {
        // This test covers line 141: Self::Closure(closure, closure_source) => Self::Closure(Arc::clone(closure), *closure_source)
        // We create a Trc with Factory::Closure via with_closure, clone it, and verify the factory is properly cloned

        use crate::{ThreadAware, create_manual_memory_affinities};

        let affinities = create_manual_memory_affinities(&[2]);
        let affinity1 = affinities[0];
        let affinity2 = affinities[1];

        // Create a Trc with a closure that uses Factory::Closure
        let trc1 = Trc::with_closure(relocate((), |()| 100));

        // Clone the Trc - this should exercise line 141 in the Factory::clone method
        let trc2 = trc1.clone();

        // Verify both Trcs work correctly
        assert_eq!(**trc1, 100);
        assert_eq!(**trc2, 100);

        // Relocate both to verify the cloned factory (closure) works properly
        let trc1_relocated = trc1.relocated(affinity1, affinity2);
        let trc2_relocated = trc2.relocated(affinity1, affinity2);

        assert_eq!(**trc1_relocated, 100);
        assert_eq!(**trc2_relocated, 100);

        // Both relocated Trcs should point to the same Arc due to deduplication
        assert!(std::sync::Arc::ptr_eq(&*trc1_relocated, &*trc2_relocated));
    }

    #[test]
    fn test_factory_clone_with_manual() {
        // This test covers line 143: Self::Manual => Self::Manual
        // We create a Trc from storage (Factory::Manual), clone it, and verify the factory is properly cloned

        use crate::create_manual_pinned_affinities;
        use std::sync::{Arc, RwLock};

        let affinities = create_manual_pinned_affinities(&[2]);
        let affinity1 = affinities[0];

        // Create a storage and populate it with a value for affinity1
        let mut storage = super::storage::Storage::new();
        let value = Arc::new(200);
        storage.replace(affinity1, Arc::clone(&value));

        let storage_arc = Arc::new(RwLock::new(storage));

        // Create a Trc from storage - this uses Factory::Manual
        let trc1 = Trc::from_storage(Arc::clone(&storage_arc), affinity1);

        // Clone the Trc - this should exercise line 143 in the Factory::clone method
        let trc2 = trc1.clone();

        // Verify both Trcs work correctly
        assert_eq!(**trc1, 200);
        assert_eq!(**trc2, 200);

        // Both should point to the same Arc
        assert!(Arc::ptr_eq(&*trc1, &*trc2));
    }

    #[test]
    fn test_factory_manual_relocated() {
        // This test covers line 453: Factory::Manual branch in relocated()
        // When a Trc is created from storage (Factory::Manual) and relocated to a new affinity,
        // it should behave like Arc<T> and just clone the value without creating new data

        use crate::{ThreadAware, create_manual_pinned_affinities};
        use std::sync::{Arc, RwLock};

        let affinities = create_manual_pinned_affinities(&[2]);
        let affinity1 = affinities[0];
        let affinity2 = affinities[1];

        // Create a storage with a value at affinity1
        let mut storage = super::storage::Storage::new();
        let value = Arc::new(100);
        storage.replace(affinity1, Arc::clone(&value));

        let storage_arc = Arc::new(RwLock::new(storage));

        // Create a Trc from storage - this uses Factory::Manual
        let trc = Trc::from_storage(Arc::clone(&storage_arc), affinity1);
        assert_eq!(**trc, 100);

        // Relocate to affinity2 where no data exists
        // This should trigger line 453 (Factory::Manual branch)
        // and behave like Arc<T> by just cloning the reference
        let trc_relocated = trc.relocated(affinity1.into(), affinity2.into());

        // The value should still be 100
        assert_eq!(**trc_relocated, 100);

        // The relocated Trc should point to the same Arc as the original
        // because Factory::Manual just clones the Arc
        assert!(Arc::ptr_eq(&*trc_relocated, &value));
    }
}
