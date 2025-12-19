// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod factory;
pub mod storage;

mod builtin;
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::hash::Hasher;
use std::ops::Deref;
use std::sync::{self, RwLock};

use crate::ThreadAware;
use crate::affinity::{MemoryAffinity, PinnedAffinity};
use crate::cell::factory::Factory;
use crate::closure::{ErasedClosureOnce, RelocateFnOnce, relocate_once};
pub use builtin::{PerCore, PerNuma, PerProcess};
pub use storage::{Storage, Strategy};

/// Transferable reference counted type.
///
/// This type works like a per-affinity (per-thread) [`sync::Arc`]. Each affinity gets a unique value that is shared by clones
/// of the `Arc`, but the [`ThreadAware`] implementation ensures that when moving to another affinity, the resulting
/// `Arc` will point to the value in the destination affinity. See [`new`](`Arc::new`) for information on constructing instances.
///
/// `ThreadAware` of different clones of the `Arc` result in "deduplication" in the destination affinity. The following
/// example demonstrates this using the counter implemented in the documentation for the [`ThreadAware`] trait.
///
/// ```rust
/// # use thread_aware::{Arc, ThreadAware, PerCore};
/// # use thread_aware::affinity::*;
/// # use std::sync::atomic::{AtomicI32, Ordering};
/// # let affinities = pinned_affinities(&[2]);
/// # let affinity1 = affinities[0].into();
/// # let affinity2 = affinities[1];
/// # #[derive(Clone)]
/// # struct Counter {
/// #     value: std::sync::Arc<AtomicI32>,
/// # }
/// #
/// # impl Counter {
/// #     fn new() -> Self {
/// #         Self {
/// #             value: std::sync::Arc::new(AtomicI32::new(0)),
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
/// #     fn relocated(self, source: MemoryAffinity, destination: PinnedAffinity) -> Self {
/// #         Self {
/// #             // Initialize a new value in the destination affinity independent
/// #             // of the source affinity.
/// #             value: std::sync::Arc::new(AtomicI32::new(0)),
/// #         }
/// #     }
/// # }
///
/// let arc_affinity1 = Arc::<_, PerCore>::new(Counter::new);
/// let arc_affinity1_clone = arc_affinity1.clone();
///
/// arc_affinity1.increment_by(42);
/// assert_eq!(arc_affinity1.value(), 42);
///
/// let arc_affinity2 = arc_affinity1.relocated(affinity1, affinity2);
/// assert_eq!(arc_affinity2.value(), 0);
/// assert_eq!(arc_affinity1_clone.value(), 42);
///
/// arc_affinity2.increment_by(11);
/// let arc_affinity2_clone = arc_affinity1_clone.relocated(affinity1, affinity2);
/// assert_eq!(arc_affinity2_clone.value(), 11);
/// ```
#[derive(Debug)]
pub struct Arc<T, S: Strategy> {
    storage: sync::Arc<RwLock<Storage<sync::Arc<T>, S>>>,
    value: sync::Arc<T>,
    factory: Factory<T>,
}

impl<T: PartialEq, S: Strategy> PartialEq for Arc<T, S> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T: Eq, S: Strategy> Eq for Arc<T, S> {}

impl<T: std::hash::Hash, S: Strategy> std::hash::Hash for Arc<T, S> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl<T: Ord, S: Strategy> Ord for Arc<T, S> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.value.cmp(&other.value)
    }
}

impl<T: PartialOrd, S: Strategy> PartialOrd for Arc<T, S> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.value.partial_cmp(&other.value)
    }
}

impl<T, S: Strategy> Clone for Arc<T, S> {
    fn clone(&self) -> Self {
        Self {
            storage: sync::Arc::clone(&self.storage),
            value: sync::Arc::clone(&self.value),
            factory: self.factory.clone(),
        }
    }
}

impl<T, S: Strategy> Deref for Arc<T, S> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T, S> Arc<T, S>
where
    T: Send + 'static,
    S: Strategy,
{
    /// Creates a new `Arc` with the given value and strategy.
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
    /// # use thread_aware::{Arc, ThreadAware, PerCore};
    /// # use thread_aware::affinity::*;
    /// # use std::sync::atomic::{AtomicI32, Ordering};
    /// # use std::sync;
    /// # #[derive(Clone)]
    /// # struct Counter {
    /// #     value: sync::Arc<AtomicI32>,
    /// # }
    /// #
    /// # impl Counter {
    /// #     fn new() -> Self {
    /// #         Self {
    /// #             value: sync::Arc::new(AtomicI32::new(0)),
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
    /// #     fn relocated(self, _source: MemoryAffinity, _destination: PinnedAffinity) -> Self {
    /// #         Self {
    /// #             // Initialize a new value in the destination affinity independent
    /// #             // of the source affinity.
    /// #             value: sync::Arc::new(AtomicI32::new(0)),
    /// #         }
    /// #     }
    /// # }
    ///
    /// let container = Arc::<_, PerCore>::new(Counter::new);
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
            fn relocated(self, _source: MemoryAffinity, _destination: PinnedAffinity) -> Self {
                self
            }
        }

        impl<T> RelocateFnOnce<T> for Ctor<T> {
            fn call_once(self) -> T {
                (self.f)()
            }
        }

        // Use Self::with_closure to ensure Factory::Closure path.
        Self::with_closure(Ctor { f: ctor })
    }
}

impl<T, S> Arc<T, S>
where
    T: 'static,
    S: Strategy,
{
    /// Creates a new `Arc` with a closure that will be called once per-processor to create the inner value.
    ///
    /// The closure only gets called once for each processor, and it's called only when a `Arc` is actually transferred
    /// to another processor. The closure behaves like a `RelocateFnOnce` to ensure it captures only values that are safe to
    /// transfer themselves.
    ///
    /// This function can be used to create an `Arc` of a type that itself doesn't implement [`ThreadAware`] because
    /// we can ensure that each affinity will get its own, independently-initialized value:
    ///
    /// ```rust
    /// # use std::sync::{self, Mutex};
    /// # use thread_aware::{Arc, PerCore};
    /// struct MyStruct {
    ///     inner: sync::Arc<Mutex<i32>>,
    /// }
    ///
    /// impl MyStruct {
    ///     fn new() -> Self {
    ///         Self {
    ///             inner: sync::Arc::new(Mutex::new(0)),
    ///         }
    ///     }
    /// }
    ///
    /// let container = Arc::<_, PerCore>::new_with((), |_| MyStruct::new());
    /// ```
    ///
    /// The constructor can depend on other values that implement [`ThreadAware`] (this example uses the Counter
    /// defined in [`ThreadAware`] documentation):
    ///
    /// ```rust
    /// # use thread_aware::{ThreadAware, Arc, PerCore};
    /// # use thread_aware::affinity::*;
    /// # use std::sync::atomic::{AtomicI32, Ordering};
    /// # use std::sync;
    /// # #[derive(Clone)]
    /// # struct Counter {
    /// #     value: sync::Arc<AtomicI32>,
    /// # }
    /// #
    /// # impl Counter {
    /// #     fn new() -> Self {
    /// #         Self {
    /// #             value: sync::Arc::new(AtomicI32::new(0)),
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
    /// #     fn relocated(self, source: MemoryAffinity, destination: PinnedAffinity) -> Self {
    /// #         Self {
    /// #             // Initialize a new value in the destination affinity independent
    /// #             // of the source affinity.
    /// #             value: sync::Arc::new(AtomicI32::new(0)),
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
    /// let container = Arc::<_, PerCore>::new_with(counter, |counter| MyStruct::new(counter.value()));
    /// ```
    pub fn new_with<D>(data: D, f: fn(D) -> T) -> Self
    where
        D: ThreadAware + Send + Sync + Clone + 'static,
    {
        Self::with_closure(relocate_once(data, f))
    }
}

impl<T, S: Strategy> Arc<T, S>
where
    T: ThreadAware + Clone + 'static + Send,
{
    /// Creates a new `Arc` with the given value.
    ///
    /// The value must implement `ThreadAware` and `Clone`. When transferring to another affinity
    /// which doesn't yet contain a value, a new value is created by cloning the value in current
    /// affinity and transferring it to the new affinity.
    ///
    /// For example, the counter type we implemented in the documentation for [`ThreadAware`] trait
    /// can be used with new:
    #[cfg(test)]
    pub(crate) fn with_value(value: T) -> Self {
        let value = sync::Arc::new(value);

        Self {
            storage: sync::Arc::new(RwLock::new(storage::Storage::new())),
            value,
            factory: Factory::Data(|data: &T, source, destination| {
                let data = data.clone();
                data.relocated(source, destination)
            }),
        }
    }
}

impl<T, S: Strategy> Arc<T, S>
where
    T: Clone + 'static + Send,
{
    /// Creates a new `Arc` with the given value.
    ///
    /// The value must implement `Clone`. When transferring to another affinity
    /// which doesn't yet contain a value, a new value is created by cloning the value in current
    /// affinity and transferring it to the new affinity.
    ///
    /// This is useful for types that do not implement [`ThreadAware`]. In such cases, the same value
    /// is cloned for each affinity without any relocation logic.
    ///
    /// For example, the counter type we implemented in the documentation for [`ThreadAware`] trait
    /// can be used with new:
    ///
    /// ```rust
    /// # use thread_aware::{Arc, PerCore};
    /// # use std::sync::atomic::{AtomicI32, Ordering};
    /// # use std::sync;
    /// # #[derive(Clone)]
    /// # struct Counter {
    /// #     value: sync::Arc<AtomicI32>,
    /// # }
    /// #
    /// # impl Counter {
    /// #     fn new() -> Self {
    /// #         Self {
    /// #             value: sync::Arc::new(AtomicI32::new(0)),
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
    ///
    /// let arc = Arc::<_, PerCore>::new(Counter::new);
    /// let arc_clone = arc.clone();
    /// arc.increment_by(42);
    /// assert_eq!(arc.value(), 42);
    /// assert_eq!(arc_clone.value(), 42);
    /// ```
    pub fn from_unaware(value: T) -> Self {
        let value = sync::Arc::new(value);

        Self {
            storage: sync::Arc::new(RwLock::new(storage::Storage::new())),
            value,
            factory: Factory::Data(|data: &T, _source, _destination| data.clone()),
        }
    }
}

impl<T, S: Strategy> Arc<T, S>
where
    T: 'static,
{
    /// Creates a new `Arc` with a closure that will be called once per-affinity to create the inner value.
    ///
    /// The closure only gets called once for each affinity, and it's called only when an `Arc` is actually transferred
    /// to another affinity. The closure is a [`RelocateFnOnce`] to ensure it captures only values that are safe to
    /// transfer themselves.
    pub(crate) fn with_closure<F>(closure: F) -> Self
    where
        F: RelocateFnOnce<T> + Clone + ThreadAware + 'static + Send + Sync,
    {
        let value = sync::Arc::new(closure.clone().call_once());

        Self {
            storage: sync::Arc::new(RwLock::new(storage::Storage::new())),
            value,
            factory: Factory::Closure(sync::Arc::new(ErasedClosureOnce::new(closure)), None), // We don't know the source affinity at this point
        }
    }

    /// Creates a new `Arc` from the given storage and the current affinity.
    ///
    /// If the resulting `Arc` is transferred to an affinity which does not have data in the storage,
    /// it will behave like a `sync::Arc`.
    ///
    /// # Panics
    /// This may panic if the storage does not contain data for the current affinity.
    pub fn from_storage(storage: sync::Arc<RwLock<Storage<sync::Arc<T>, S>>>, current_affinity: PinnedAffinity) -> Self {
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

impl<T, S: Strategy> Arc<T, S> {
    /// Converts the `Arc<T, S>` into an `sync::Arc<T>`.
    #[must_use]
    pub fn into_arc(self) -> sync::Arc<T> {
        self.value
    }
}

impl<T, S: Strategy> ThreadAware for Arc<T, S> {
    fn relocated(self, source: MemoryAffinity, destination: PinnedAffinity) -> Self {
        let mut guard = self.storage.write().expect("Failed to acquire write lock");

        let (value, new_factory) = if let Some(value) = guard.get_clone(destination) {
            (value, self.factory)
        } else {
            // We need to transfer or recreate the data
            let (data, factory) = match &self.factory {
                // We can use the closure to create new data
                Factory::Closure(factory, factory_source_affinity) => {
                    let factory_clone = (**factory).clone();

                    // In case factory source is stored in factory, use that - it means we already transferred the factory
                    // once, so we know the original source affinity. Otherwise, use source as that means this is the first
                    // time we're transferring the Arc, so source is the source affinity of the factory as well.
                    let factory_source = factory_source_affinity.unwrap_or(source);

                    (
                        sync::Arc::new(factory_clone.relocated(factory_source, destination).call_once()),
                        Factory::Closure(sync::Arc::clone(factory), Some(factory_source)),
                    )
                }

                // We can clone and transfer the data
                Factory::Data(factory) => (sync::Arc::new(factory(&self.value, source, destination)), self.factory),

                Factory::Manual => {
                    // If we are in manual mode, we just clone the data
                    // This effectively makes it behave like `sync::Arc<T>`
                    (sync::Arc::clone(&self.value), self.factory)
                }
            };

            let value = data;

            let old_data = guard.replace(destination, sync::Arc::<T>::clone(&value));
            assert!(
                old_data.is_none(),
                "Data already exists for the destination affinity. This should be unreachable due to the the early write lock."
            );

            (value, factory)
        };

        if let MemoryAffinity::Pinned(source) = source {
            guard.replace(source, self.value);
        }

        drop(guard);

        Self {
            storage: self.storage,
            value,
            factory: new_factory,
        }
    }
}
