// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ops::Deref;
use std::sync::{Arc, RwLock};

use super::Trc;
use crate::cell::storage;
use crate::closure::RelocateFnOnce;
use crate::{MemoryAffinity, PinnedAffinity, Storage, ThreadAware, relocate_once};

/// Per-Processor Transferable reference counted type.
///
/// This type works like a per-processor [`Arc`]. Each processor gets a unique value that is shared by clones
/// of the `PerCore`, but the [`ThreadAware`] implementation ensures that when moving to another processor, the resulting
/// `PerCore` will point to the value in the destination affinity. See the [`new`](`PerCore::new`) and
/// [`new_with`](`PerCore::new_with`) methods for information on constructing `PerCore`s.
///
/// Transfer of different clones of the `PerCore` result in "deduplication" in the destination affinity. The following
/// example demonstrates this using the counter implemented in the documentation for the [`ThreadAware`] trait.
///
/// ```rust
/// # use thread_aware::{MemoryAffinity, ThreadAware, PerCore, create_manual_affinities};
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
/// let container_affinity1 = PerCore::new(Counter::new);
/// let container_affinity1_clone = container_affinity1.clone();
///
/// container_affinity1.increment_by(42);
/// assert_eq!(container_affinity1.value(), 42);
///
/// let container_affinity2 = container_affinity1.relocated(affinity1, affinity2);
/// assert_eq!(container_affinity2.value(), 0);
/// assert_eq!(container_affinity1_clone.value(), 42);
///
/// container_affinity2.increment_by(11);
/// let container_affinity2_clone = container_affinity1_clone.relocated(affinity1, affinity2);
/// assert_eq!(container_affinity2_clone.value(), 11);
/// ```
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PerCore<T>(Trc<T, storage::PerCoreStrategy>);

impl<T> From<Trc<T, storage::PerCoreStrategy>> for PerCore<T> {
    fn from(value: Trc<T, storage::PerCoreStrategy>) -> Self {
        Self(value)
    }
}

impl<T> Clone for PerCore<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> PerCore<T>
where
    T: Send + 'static,
{
    /// Creates a new `PerCore` with the given value.
    ///
    /// This variant takes a zero-argument constructor function (`fn() -> T`).
    /// The constructor is invoked lazily and independently for each
    /// processor the first time a `PerCore` is materialized on that processor (i.e. on
    /// the first transfer into that processor). This guarantees that every processor obtains its own
    /// freshly created `T` without requiring `T: Clone` or `T: ThreadAware`.
    ///
    /// Requirements:
    /// * `T` must be `Send + 'static` so it can live in the processor storage.
    /// * The provided function must be pure with respect to per-processor isolation (it should not
    ///   leak references into other processors). Any captured state should therefore be provided via
    ///   globally shareable mechanisms or prefer [`new_with`](Self::new_with) if you need to
    ///   capture data that itself implements [`ThreadAware`].
    ///
    /// When transferring to another affinity which doesn't yet contain a value, the constructor is
    /// called in the destination affinity to create a brand new instance.
    ///
    /// For example, the counter type we implemented in the documentation for [`ThreadAware`] trait
    /// can be used with `new` by passing the constructor function (note the absence of `()`):
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
    /// # impl ThreadAware for Counter {
    /// #     fn relocated(self, _source: MemoryAffinity, _destination: MemoryAffinity) -> Self {
    /// #         Self {
    /// #             // Initialize a new value in the destination affinity independent
    /// #             // of the source affinity.
    /// #             value: Arc::new(AtomicI32::new(0)),
    /// #         }
    /// #     }
    /// # }
    ///
    /// let container = PerCore::new(Counter::new);
    /// let container_clone = container.clone();
    /// container.increment_by(42);
    /// assert_eq!(container.value(), 42);
    /// assert_eq!(container_clone.value(), 42);
    /// ```
    pub fn new(ctor: fn() -> T) -> Self {
        // We wrap the function pointer in a tiny RelocateFnOnce implementation that
        // recreates the value independently for each affinity.
        struct Ctor<T> {
            f: fn() -> T,
        }

        impl<T> Clone for Ctor<T> {
            fn clone(&self) -> Self {
                Self { f: self.f }
            }
        }

        impl<T> ThreadAware for Ctor<T> {
            fn relocated(self, _source: MemoryAffinity, _destination: MemoryAffinity) -> Self {
                self
            }
        }

        impl<T> RelocateFnOnce<T> for Ctor<T> {
            fn call_once(self) -> T {
                (self.f)()
            }
        }

        // Use Trc::with_closure to ensure Factory::Closure path.
        Trc::with_closure(Ctor { f: ctor }).into()
    }
}

impl<T> PerCore<T>
where
    T: 'static + Clone + Send,
{
    pub fn from_unaware(value: T) -> Self {
        Trc::from_unaware(value).into()
    }
}

impl<T> PerCore<T>
where
    T: 'static,
{
    /// Creates a new `PerCore` with a closure that will be called once per-processor to create the inner value.
    ///
    /// The closure only gets called once for each processor, and it's called only when a `PerCore` is actually transferred
    /// to another processor. The closure behaves like a `RelocateFnOnce` to ensure it captures only values that are safe to
    /// transfer themselves.
    ///
    /// This function can be used to create a `PerCore` of a type that itself doesn't implement [`ThreadAware`] because
    /// we can ensure that each affinity will get its own, independenty-initialized value:
    ///
    /// ```rust
    /// # use std::sync::{Arc, Mutex};
    /// # use thread_aware::{PerCore};
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
    /// let container = PerCore::new_with((), |_| MyStruct::new());
    /// ```
    ///
    /// The constructor can depend on other values that implement [`ThreadAware`] (this example uses the Counter
    /// defined in [`ThreadAware`] documentation):
    ///
    /// ```rust
    /// # use thread_aware::{ThreadAware, MemoryAffinity, PerCore, create_manual_affinities};
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
    /// let container = PerCore::new_with(counter, |counter| MyStruct::new(counter.value()));
    /// ```
    pub fn new_with<D>(data: D, f: fn(D) -> T) -> Self
    where
        D: ThreadAware + Send + Sync + Clone + 'static,
    {
        Self(Trc::with_closure(relocate_once(data, f)))
    }
}

impl<T> From<PerCore<T>> for Arc<T> {
    fn from(value: PerCore<T>) -> Self {
        value.0.into_arc()
    }
}

impl<T> PerCore<T>
where
    T: 'static,
{
    /// Creates a new `PerCore` from the given storage and the current affinity.
    ///
    /// The storage must contain data for the current affinity and any other affinities that the resulting `PerCore` may be transferred to.
    ///
    /// # Panics
    /// This may panic if the storage does not contain data for the current affinity.
    pub fn from_storage(storage: Arc<RwLock<Storage<Arc<T>, storage::PerCoreStrategy>>>, current_affinity: PinnedAffinity) -> Self {
        Self(Trc::from_storage(storage, current_affinity))
    }

    /// Converts this `PerCore` into an `Arc<T>`, consuming self.
    #[must_use]
    pub fn into_arc(self) -> Arc<T> {
        self.0.into_arc()
    }
}

impl<T> Deref for PerCore<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> ThreadAware for PerCore<T> {
    fn relocated(self, source: MemoryAffinity, destination: MemoryAffinity) -> Self {
        Self(self.0.relocated(source, destination))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI32, Ordering};

    use super::*;
    use crate::{create_manual_memory_affinities, create_manual_pinned_affinities};

    #[derive(Clone, Debug)]
    struct Counter {
        value: Arc<AtomicI32>,
    }

    impl Counter {
        fn new() -> Self {
            Self {
                value: Arc::new(AtomicI32::new(0)),
            }
        }
        fn increment_by(&self, v: i32) {
            self.value.fetch_add(v, Ordering::AcqRel);
        }
        fn value(&self) -> i32 {
            self.value.load(Ordering::Acquire)
        }
    }

    impl ThreadAware for Counter {
        fn relocated(self, _source: MemoryAffinity, _destination: MemoryAffinity) -> Self {
            Self {
                value: Arc::new(AtomicI32::new(0)),
            }
        }
    }

    #[test]
    fn transfer_creates_new_value() {
        let affinities = create_manual_memory_affinities(&[2]);
        let pmr = PerCore::new(Counter::new);
        pmr.increment_by(10);
        let pmr2 = pmr.clone().relocated(affinities[0], affinities[1]);
        assert_eq!(pmr.value(), 10);
        assert_eq!(pmr2.value(), 0);
    }

    #[test]
    fn new_with_works() {
        let pmr = PerCore::new_with((), |()| Counter::new());
        pmr.increment_by(3);
        assert_eq!(pmr.value(), 3);
    }

    #[test]
    fn into_arc_returns_arc() {
        let pmr = PerCore::new(Counter::new);
        let arc: Arc<Counter> = Arc::from(pmr);
        assert_eq!(arc.value(), 0);
    }

    #[test]
    fn test_from_unaware() {
        // Create a PerCore from an unaware value (a simple i32)
        // This covers line 191 (from_unaware method)
        let per_core = PerCore::from_unaware(42);
        assert_eq!(*per_core, 42);

        // Verify it can be relocated
        let affinities = create_manual_memory_affinities(&[2]);
        let relocated = per_core.relocated(affinities[0], affinities[1]);
        assert_eq!(*relocated, 42);
    }

    #[test]
    fn test_from_storage() {
        // Create a storage and populate it with a value for a specific affinity
        // This covers line 303 (from_storage method)
        let affinities = create_manual_pinned_affinities(&[2]);
        let affinity1 = affinities[0];

        // Create a storage and populate it
        let mut storage = super::storage::Storage::new();
        let counter = Arc::new(Counter::new());
        counter.increment_by(100);
        storage.replace(affinity1, Arc::clone(&counter));

        let storage_arc = Arc::new(RwLock::new(storage));

        // Create a PerCore from the storage
        let per_core = PerCore::from_storage(Arc::clone(&storage_arc), affinity1);

        // Verify the value is correct
        assert_eq!(per_core.value(), 100);

        // Verify it points to the same Arc we put in storage
        assert!(Arc::ptr_eq(&per_core.into_arc(), &counter));
    }

    #[test]
    fn test_counter_relocated() {
        // This test explicitly covers line 355 (Counter's ThreadAware::relocated implementation)
        let affinities = create_manual_memory_affinities(&[2]);
        let affinity1 = affinities[0];
        let affinity2 = affinities[1];

        // Create a counter and set a value
        let counter = Counter::new();
        counter.increment_by(50);
        assert_eq!(counter.value(), 50);

        // Clone to keep a reference to the original Arc
        let original_arc = Arc::clone(&counter.value);

        // Call relocated directly on the Counter
        // This invokes line 355 which creates a new Counter with value 0
        let relocated_counter = counter.relocated(affinity1, affinity2);

        // Verify the relocated counter is a fresh instance with value 0
        assert_eq!(relocated_counter.value(), 0);

        // Verify they are different Arc instances
        assert!(!Arc::ptr_eq(&original_arc, &relocated_counter.value));

        // Verify the original Arc still has value 50
        assert_eq!(original_arc.load(Ordering::Acquire), 50);
    }
}
