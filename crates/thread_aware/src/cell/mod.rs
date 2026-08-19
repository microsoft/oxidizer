// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod clone_fn;
mod factory;
pub mod storage;

mod builtin;
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests;

#[cfg(not(test))]
use alloc::boxed::Box;
use std::cmp::Ordering;
use std::hash::Hasher;
use std::ops::Deref;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{self};

pub use builtin::{PerCore, PerNuma, PerProcess};
pub(crate) use storage::{Storage, Strategy};

use crate::ThreadAware;
use crate::affinity::Affinity;
use crate::cell::factory::Factory;
use crate::closure::{ErasedClosureOnce, ThreadAwareFnOnce, closure_once};

/// Adapter that wraps a `ThreadAwareFnOnce<T>` to produce `Box<T>` instead.
struct BoxedRelocate<F>(F);

impl<F: Clone> Clone for BoxedRelocate<F> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<F: ThreadAware> ThreadAware for BoxedRelocate<F> {
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        self.0.relocate(source, destination);
    }
}

impl<T, F: ThreadAwareFnOnce<T>> ThreadAwareFnOnce<Box<T>> for BoxedRelocate<F> {
    fn call_once(self) -> Box<T> {
        Box::new(self.0.call_once())
    }
}

/// Transferable reference counted type.
///
/// This type works like a per-affinity (per-thread) [`sync::Arc`]. Each affinity gets a unique value that is shared by clones
/// of the `Arc`, but the [`trait@ThreadAware`] implementation ensures that when moving to another affinity, the resulting
/// `Arc` will point to the value in the destination affinity. See [`new`](`Arc::new`) for information on constructing instances.
///
/// `ThreadAware` of different clones of the `Arc` result in "deduplication" in the destination affinity. The following
/// example demonstrates this using the counter implemented in the documentation for the [`trait@ThreadAware`] trait.
///
/// ```rust
/// # use thread_aware::{Arc, ThreadAware, PerCore};
/// # use thread_aware::affinity::*;
/// # use std::sync::atomic::{AtomicI32, Ordering};
/// # let affinities = pinned_affinities(&[2]);
/// # let affinity1 = Some(affinities[0]);
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
/// #     fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {
/// #         // Initialize a new value in the destination affinity independent
/// #         // of the source affinity.
/// #         self.value = std::sync::Arc::new(AtomicI32::new(0));
/// #     }
/// # }
///
/// let mut arc_affinity1 = Arc::<_, PerCore>::new(Counter::new);
/// let arc_affinity1_clone = arc_affinity1.clone();
///
/// arc_affinity1.increment_by(42);
/// assert_eq!(arc_affinity1.value(), 42);
///
/// arc_affinity1.relocate(affinity1, affinity2);
/// assert_eq!(arc_affinity1.value(), 0);
/// assert_eq!(arc_affinity1_clone.value(), 42);
///
/// arc_affinity1.increment_by(11);
/// let mut arc_affinity2_clone = arc_affinity1_clone;
/// arc_affinity2_clone.relocate(affinity1, affinity2);
/// assert_eq!(arc_affinity2_clone.value(), 11);
/// ```
#[derive(Debug)]
pub struct Arc<T: ?Sized, S: Strategy> {
    storage: sync::Arc<Storage<T, S>>,
    value: sync::Arc<T>,
    factory: Factory<T>,
}

impl<T: PartialEq, S: Strategy> PartialEq for Arc<T, S> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T: Eq, S: Strategy> Eq for Arc<T, S> {}

impl<T: std::hash::Hash + ?Sized, S: Strategy> std::hash::Hash for Arc<T, S> {
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

impl<T: ?Sized, S: Strategy> Clone for Arc<T, S> {
    fn clone(&self) -> Self {
        Self {
            storage: sync::Arc::clone(&self.storage),
            value: sync::Arc::clone(&self.value),
            factory: self.factory.clone(),
        }
    }
}

impl<T: ?Sized, S: Strategy> Deref for Arc<T, S> {
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
    ///   capture data that itself implements [`trait@ThreadAware`].
    ///
    /// When transferring to another affinity which doesn't yet contain a value, the constructor is
    /// called in the destination affinity to create a brand new instance.
    ///
    /// For example, the counter type we implemented in the documentation for [`trait@ThreadAware`] trait
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
    /// #     fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {
    /// #         // Initialize a new value in the destination affinity independent
    /// #         // of the source affinity.
    /// #         self.value = sync::Arc::new(AtomicI32::new(0));
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
        // We wrap the function pointer in a tiny ThreadAwareFnOnce implementation that
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
            fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {}
        }

        impl<T> ThreadAwareFnOnce<Box<T>> for Ctor<T> {
            fn call_once(self) -> Box<T> {
                Box::new((self.f)())
            }
        }

        // Use Self::with_closure_boxed to ensure Factory::Closure path.
        Self::with_closure_boxed(Ctor { f: ctor })
    }
}

impl<T, S> Arc<T, S>
where
    T: Send + 'static + ?Sized,
    S: Strategy,
{
    /// Creates a new `Arc` with a constructor that returns `Box<T>`.
    ///
    /// This is the `?Sized`-compatible version of [`new`](Self::new). Use this when `T` is a
    /// trait object (e.g., `dyn Trait`) or other unsized type. The constructor produces a
    /// `Box<T>` which is then stored behind a [`sync::Arc`].
    ///
    /// ```rust
    /// # use thread_aware::{Arc, ThreadAware, PerCore};
    /// let arc = Arc::<dyn ThreadAware, PerCore>::new_boxed(|| Box::new(42u32));
    /// ```
    pub fn new_boxed(ctor: fn() -> Box<T>) -> Self {
        struct Ctor<T: ?Sized> {
            f: fn() -> Box<T>,
        }

        impl<T: ?Sized> Clone for Ctor<T> {
            fn clone(&self) -> Self {
                Self { f: self.f }
            }
        }

        impl<T: ?Sized> ThreadAware for Ctor<T> {
            fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {}
        }

        impl<T: ?Sized> ThreadAwareFnOnce<Box<T>> for Ctor<T> {
            fn call_once(self) -> Box<T> {
                (self.f)()
            }
        }

        Self::with_closure_boxed(Ctor { f: ctor })
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
    /// to another processor. The closure behaves like a `ThreadAwareFnOnce` to ensure it captures only values that are safe to
    /// transfer themselves.
    ///
    /// This function can be used to create an `Arc` of a type that itself doesn't implement [`trait@ThreadAware`] because
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
    /// The constructor can depend on other values that implement [`trait@ThreadAware`] (this example uses the Counter
    /// defined in [`trait@ThreadAware`] documentation):
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
    /// #     fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {
    /// #         // Initialize a new value in the destination affinity independent
    /// #         // of the source affinity.
    /// #         self.value = sync::Arc::new(AtomicI32::new(0));
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
        Self::with_closure_boxed(BoxedRelocate(closure_once(data, f)))
    }
}

impl<T, S: Strategy> Arc<T, S>
where
    T: ThreadAware + Clone + 'static + Send,
{
    /// Creates a new `Arc` with the given value.
    ///
    /// The value must implement [`trait@ThreadAware`] and [`Clone`]. When transferring to another affinity
    /// which doesn't yet contain a value, a new value is created by cloning the value in current
    /// affinity and transferring it to the new affinity.
    ///
    /// For example, the counter type we implemented in the documentation for the [`trait@ThreadAware`] trait
    /// can be used with new.
    #[cfg(test)]
    pub(crate) fn with_value(value: T) -> Self {
        let value = sync::Arc::new(value);

        Self {
            storage: sync::Arc::new(Storage::new()),
            value,
            factory: Factory::Data(|data: &T, source, destination| {
                let mut data = data.clone();
                data.relocate(source, destination);
                Box::new(data)
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
    /// The value must implement [`Clone`]. When transferring to another affinity
    /// which doesn't yet contain a value, a new value is created by cloning the value in current
    /// affinity and transferring it to the new affinity.
    ///
    /// This is useful for types that do not implement [`trait@ThreadAware`]. In such cases, the same value
    /// is cloned for each affinity without any relocation logic.
    ///
    /// For example, the counter type we implemented in the documentation for [`trait@ThreadAware`] trait
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
            storage: sync::Arc::new(Storage::new()),
            value,
            factory: Factory::Data(|data: &T, _source, _destination| Box::new(data.clone())),
        }
    }
}

impl<T, S: Strategy> Arc<T, S>
where
    T: ThreadAware + 'static + ?Sized,
{
    /// Creates a new `Arc` from a value and a clone function, supporting trait objects.
    ///
    /// The object passed will be kept, and serves as the template for all subsequent clones
    /// that happen on relocation. A clone is also performed for the initial `Arc`.
    ///
    /// The `clone_fn` receives `&V` (the concrete type) and returns `Box<T>`, enabling
    /// use with `dyn Trait` where `Clone` is not object-safe. Each clone is
    /// [`relocate`](ThreadAware::relocate) to its target affinity.
    ///
    /// ```rust
    /// # use thread_aware::{Arc, PerCore, ThreadAware};
    /// # #[derive(Clone)]
    /// # struct Foo(u32);
    /// # impl ThreadAware for Foo {
    /// #     fn relocate(&mut self, _: Option<thread_aware::affinity::Affinity>, _: thread_aware::affinity::Affinity) {}
    /// # }
    /// trait MyPlugin: ThreadAware {}
    /// impl MyPlugin for Foo {}
    ///
    /// let arc = Arc::<dyn MyPlugin, PerCore>::with_clone_fn(Foo(42), |v: &Foo| Box::new(v.clone()));
    /// ```
    pub fn with_clone_fn<V: Send + Sync + 'static>(value: V, clone_fn: fn(&V) -> Box<T>) -> Self {
        // In a canonical case, we might have `V = u32`, `T = dyn Foo`, and `clone_fn = |&u32| -> Box<dyn Foo>`.
        let erased = clone_fn::ErasedCloneFn::new(value, clone_fn);
        let value = sync::Arc::clone(erased.arc());

        Self {
            storage: sync::Arc::new(Storage::new()),
            value,
            factory: Factory::ErasedCloneFn(erased),
        }
    }
}

impl<T, S: Strategy> Arc<T, S>
where
    T: 'static + ?Sized,
{
    /// Creates a new `Arc` with a closure that produces `Box<T>`, called once per-affinity to create the inner value.
    ///
    /// The closure only gets called once for each affinity, and it's called only when an `Arc` is actually transferred
    /// to another affinity. The closure is a [`ThreadAwareFnOnce`] to ensure it captures only values that are safe to
    /// transfer themselves.
    pub(crate) fn with_closure_boxed<F>(closure: F) -> Self
    where
        F: ThreadAwareFnOnce<Box<T>> + Clone + ThreadAware + 'static + Send + Sync,
    {
        let value = sync::Arc::from(closure.clone().call_once());

        Self {
            storage: sync::Arc::new(Storage::new()),
            value,
            factory: Factory::Closure(sync::Arc::new(ErasedClosureOnce::new(closure)), None),
        }
    }

    /// Creates a new `Arc` from the given storage and the current affinity.
    ///
    /// This is the counterpart to building a [`Storage`] directly: populate it with
    /// [`Storage::insert`] for the affinities that should carry a value, then hand it here to
    /// obtain an `Arc` backed by those values.
    ///
    /// If the resulting `Arc` is transferred to an affinity which does not have data in the storage,
    /// it will behave like a [`sync::Arc`].
    ///
    /// # Panics
    /// Panics if the storage does not contain data for the current affinity.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc as StdArc;
    ///
    /// use thread_aware::affinity::pinned_affinities;
    /// use thread_aware::storage::Storage;
    /// use thread_aware::{Arc, PerCore};
    ///
    /// let affinity = pinned_affinities(&[2])[0];
    ///
    /// let storage = Storage::new();
    /// storage.insert(affinity, StdArc::new(42));
    ///
    /// let arc = Arc::<_, PerCore>::from_storage(StdArc::new(storage), affinity);
    /// assert_eq!(*arc, 42);
    /// ```
    ///
    /// [`Storage`]: crate::storage::Storage
    /// [`Storage::insert`]: crate::storage::Storage::insert
    pub fn from_storage(storage: sync::Arc<Storage<T, S>>, current_affinity: Affinity) -> Self {
        let value = storage.get(current_affinity).expect("No data found for the current affinity");

        Self {
            storage,
            value,
            factory: Factory::Manual,
        }
    }
}

impl<T, S: Strategy> Arc<T, S> {
    /// Gets the number of strong references to the value in the current thread/affinity.
    ///
    /// This method returns the strong reference count for the underlying [`sync::Arc`]
    /// that holds the value for the current affinity, excluding any internal references
    /// held by the storage for deduplication purposes. Each affinity maintains its own
    /// separate value with its own reference count.
    ///
    /// # Examples
    ///
    /// ```
    /// use thread_aware::{Arc, PerCore};
    ///
    /// let arc = Arc::<_, PerCore>::new(|| 42);
    /// assert_eq!(Arc::strong_count(&arc), 1);
    ///
    /// let arc2 = arc.clone();
    /// assert_eq!(Arc::strong_count(&arc), 2);
    /// assert_eq!(Arc::strong_count(&arc2), 2);
    /// ```
    #[must_use]
    pub fn strong_count(this: &Self) -> usize {
        let raw = sync::Arc::strong_count(&this.value);
        let internal = this.storage.count_where(|stored| sync::Arc::ptr_eq(stored, &this.value));

        // `sync::Arc::strong_count` is an unsynchronized snapshot, stale the instant it is read, so
        // `raw` and `internal` can never be reconciled into one consistent view regardless of how
        // they are sampled: a concurrent relocation can publish this value into another slot,
        // leaving `internal` momentarily larger than the now-stale `raw`. Saturate rather than
        // underflow.
        raw.saturating_sub(internal)
    }

    /// Converts the `Arc<T, S>` into an `sync::Arc<T>`.
    #[must_use]
    pub fn into_arc(self) -> sync::Arc<T> {
        self.value
    }
}

impl<T: Send + Sync + ?Sized, S: Strategy + Send + Sync> Arc<T, S> {
    /// Produces the value for `destination` and a replacement factory.
    ///
    /// The first element is the value `destination` will hold. The second is a
    /// replacement for [`self.factory`](Self), or `None` when the factory is
    /// unchanged. Only the closure factory ever changes, and only on an `Arc`'s
    /// first relocation: it records the source affinity it started from so later
    /// relocations reproduce the original transfer. The other factory kinds are
    /// stateless and never need replacing.
    ///
    /// This runs the configured factory, which is caller-supplied code and the
    /// only such code `relocate` runs while holding a slot lock. `relocate` wraps
    /// this call in `catch_unwind`; if the factory panics, the destination slot is
    /// left empty and the next relocation into that affinity re-materializes.
    fn materialize(&self, source: Option<Affinity>, destination: Affinity) -> (sync::Arc<T>, Option<Factory<T>>) {
        match &self.factory {
            Factory::Closure(factory, factory_source_affinity) => {
                let mut factory_clone = (**factory).clone();

                // Prefer the source affinity already recorded in the factory: it is set on the first
                // relocation and is the affinity the factory was originally built for. Fall back to
                // `source` on that first relocation, when nothing has been recorded yet.
                let factory_source = factory_source_affinity.or(source);

                factory_clone.relocate(factory_source, destination);
                let data = sync::Arc::from(factory_clone.call_once());

                // Record the source affinity the first time we learn it; afterwards the factory is
                // identical and does not need replacing.
                let updated = factory_source_affinity
                    .is_none()
                    .then(|| Factory::Closure(sync::Arc::clone(factory), factory_source));

                (data, updated)
            }

            // The remaining kinds are stateless: they produce the value and keep themselves.
            Factory::Data(factory) => (sync::Arc::from(factory(&self.value, source, destination)), None),

            Factory::ErasedCloneFn(erased) => (erased.clone_and_relocate(source, destination), None),

            // Manual mode behaves like a plain `sync::Arc<T>`: clone the current value.
            Factory::Manual => (sync::Arc::clone(&self.value), None),
        }
    }
}

impl<T: Send + Sync + ?Sized, S: Strategy + Send + Sync> ThreadAware for Arc<T, S> {
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        // Relocation is a two-stage locking operation, scoped to a single affinity's slot: a
        // shared-lock probe of the destination slot, escalating to that slot's exclusive lock only
        // when it turns out to be empty.
        //
        // The steady state of any affinity is "already materialized", so the probe is the
        // overwhelmingly common outcome and it only reads an already-published slot. Because each
        // affinity owns its own lock, relocations targeting different affinities never contend, and
        // even a miss blocks only the affinity it materializes.
        // Ref: docs/implementation.md, "Relocation locking".
        if let Some(value) = self.storage.get(destination) {
            self.value = value;
            return;
        }

        // A miss into a slot different from the source records two slots. The destination slot gets
        // the freshly materialized value. The source slot gets the value the `Arc` is carrying: that
        // value belongs to the source slot, and on a first relocation it has never been written to
        // the table, so recording it lets a later relocation back into the source slot find it
        // instead of materializing a fresh one.
        //
        // The destination slot's exclusive lock is held across materialization so exactly one thread
        // populates that slot. The source slot is written afterwards, under its own lock and never
        // while the destination lock is held, so two threads relocating in opposite directions
        // cannot each end up waiting for the lock the other holds.
        //
        // When the source resolves to the destination's own slot there is no cross-slot move: the
        // carried value already belongs to that slot, so a miss seeds the slot with it and keeps it
        // rather than materializing a fresh value that would diverge from the shared one. This is
        // the whole of relocation under `PerProcess`, where every affinity shares one slot.
        // Ref: docs/implementation.md, "Relocation locking".
        let same_slot = source.is_some_and(|source| S::index(source) == S::index(destination));

        let (old_value, replaced_empty_slot) = {
            let mut destination_slot = self.storage.write(destination);

            // The slot is re-probed because the lock was released between the two stages, during
            // which another thread may have materialized this same destination slot.
            if let Some(value) = destination_slot.clone() {
                self.value = value;
                return;
            }

            // Same slot as the source: the carried value already belongs here, so seed the empty
            // slot with it and keep it, without materializing a fresh value.
            if same_slot {
                *destination_slot = Some(sync::Arc::<T>::clone(&self.value));
                return;
            }

            // Materializing runs the caller's factory — the only caller code executed while a slot
            // lock is held. Run it under `catch_unwind` so that a panic unwinds only after the guard
            // is dropped, leaving the slot lock unpoisoned.
            // Ref: docs/implementation.md, "Relocation locking".
            let (data, new_factory) = match panic::catch_unwind(AssertUnwindSafe(|| self.materialize(source, destination))) {
                Ok(materialized) => materialized,
                Err(payload) => {
                    drop(destination_slot);
                    panic::resume_unwind(payload);
                }
            };

            let old_value = std::mem::replace(&mut self.value, data);

            let old_data = destination_slot.replace(sync::Arc::<T>::clone(&self.value));

            // Record the factory only when materialization actually changed it (the closure factory,
            // on the first relocation). The stateless kinds return `None` and are left untouched.
            if let Some(factory) = new_factory {
                self.factory = factory;
            }

            (old_value, old_data.is_none())
        };

        // The re-probe under the write lock proved the slot empty, so the publish above must have
        // filled an empty slot. Check this only after releasing the lock: an assertion firing while
        // the lock was held would poison it and break the never-poison guarantee.
        // Ref: docs/implementation.md, "Relocation locking".
        debug_assert!(replaced_empty_slot, "slot was occupied after a re-probe under its own write lock");

        // Record the value the `Arc` moved away from into the source slot. This is reached only on a
        // cross-slot miss — the same-slot case returned above — so the source slot is always
        // distinct from the destination slot just written.
        if let Some(source) = source {
            let mut source_slot = self.storage.write(source);

            // Store only if the source slot has no value yet. Another thread may have recorded it
            // while this one held the destination lock; that is the same value this would store, so
            // leaving it in place is correct.
            if source_slot.is_none() {
                *source_slot = Some(old_value);
            }
        }
    }
}
