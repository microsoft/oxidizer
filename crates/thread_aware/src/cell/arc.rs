// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(not(test))]
use alloc::boxed::Box;
use std::cmp::Ordering;
use std::hash::Hasher;
use std::ops::Deref;
use std::sync::{self};
#[cfg(test)]
use std::{cell::RefCell, rc::Rc};

use super::factory::Factory;
use super::storage::{Storage, Strategy};
use crate::ThreadAware;
use thread_aware_core::Thread;
use crate::closure::{ErasedClosureOnce, ThreadAwareFnOnce, closure_once};

/// Adapter that wraps a `ThreadAwareFnOnce<T>` to produce `Box<T>` instead.
struct BoxedRelocate<F>(F);

impl<F: Clone> Clone for BoxedRelocate<F> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<F: ThreadAware> ThreadAware for BoxedRelocate<F> {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.0.relocate(source, destination);
    }
}

impl<T, F: ThreadAwareFnOnce<T>> ThreadAwareFnOnce<Box<T>> for BoxedRelocate<F> {
    fn call_once(self) -> Box<T> {
        Box::new(self.0.call_once())
    }
}

/// Associates a one-shot test checkpoint with the guard that owns it.
#[cfg(test)]
struct RegisteredAfterFactoryUpdateHook {
    owner: Rc<()>,
    hook: Box<dyn FnOnce()>,
}

#[cfg(test)]
std::thread_local! {
    static AFTER_FACTORY_UPDATE_HOOK: RefCell<Option<RegisteredAfterFactoryUpdateHook>> =
        const { RefCell::new(None) };
}

/// Clears a test checkpoint if its owning test exits before relocation consumes it.
#[cfg(test)]
#[must_use = "keep the guard alive until relocation has consumed the registered hook"]
pub(super) struct AfterFactoryUpdateHookGuard {
    // The `Rc` keeps the guard on the thread that owns the thread-local registration.
    owner: Rc<()>,
}

#[cfg(test)]
impl Drop for AfterFactoryUpdateHookGuard {
    fn drop(&mut self) {
        AFTER_FACTORY_UPDATE_HOOK.with(|registered| {
            let mut registered = registered.borrow_mut();
            if registered
                .as_ref()
                .is_some_and(|registered| Rc::ptr_eq(&registered.owner, &self.owner))
            {
                registered.take();
            }
        });
    }
}

/// Registers a one-shot checkpoint after factory state is updated but before publication.
#[cfg(test)]
pub(super) fn set_after_factory_update_hook(hook: impl FnOnce() + 'static) -> AfterFactoryUpdateHookGuard {
    let owner = Rc::new(());

    AFTER_FACTORY_UPDATE_HOOK.with(|registered| {
        let mut registered = registered.borrow_mut();
        assert!(
            registered.is_none(),
            "only one factory-update test hook may be registered per thread"
        );
        *registered = Some(RegisteredAfterFactoryUpdateHook {
            owner: Rc::clone(&owner),
            hook: Box::new(hook),
        });
    });

    AfterFactoryUpdateHookGuard { owner }
}

#[cfg(test)]
pub(super) fn run_after_factory_update_hook() {
    let registered = AFTER_FACTORY_UPDATE_HOOK.with(|registered| registered.borrow_mut().take());
    if let Some(registered) = registered {
        (registered.hook)();
    }
}

/// Transferable reference counted type.
///
/// The strategy parameter `S` partitions the available affinity space. Clones whose affinities map
/// to the same partition share one value, while different partitions use independently materialized
/// values. After an object graph moves to another thread, [`ThreadAware`] relocation switches each
/// `Arc` to the value assigned to the destination thread's affinity. See [`new`](Arc::new) for
/// construction details.
///
/// Relocate an `Arc` only among affinities that its [`Strategy`] interprets in one consistent
/// coordinate space: every such affinity must report the same partition count and map inside that
/// partitioning (see the design guide, "Affinities and strategies").
///
/// # Reentrant initialization
///
/// Creating a value for a previously unused strategy partition invokes the configured constructor,
/// clone function, and captured [`ThreadAware`] state. Reentrant initialization deadlocks: code
/// initializing a destination value must not directly or indirectly trigger another relocation that
/// requires the same value.
///
/// Relocation of different clones of the `Arc` results in deduplication in the destination strategy
/// partition. The following example demonstrates this using the counter implemented in the
/// documentation for the [`trait@ThreadAware`] trait.
///
/// ```rust
/// # use thread_aware::{Arc, ThreadAware, PerThread};
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
/// #     fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {
/// #         // Initialize a new value in the destination affinity independent
/// #         // of the source affinity.
/// #         self.value = std::sync::Arc::new(AtomicI32::new(0));
/// #     }
/// # }
///
/// let mut arc_affinity1 = Arc::<_, PerThread>::new(Counter::new);
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
    pub(super) storage: sync::Arc<Storage<T, S>>,
    pub(super) value: sync::Arc<T>,
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
    /// Creates an `Arc` with a constructor used to create partition-specific instances.
    ///
    /// This variant takes a zero-argument constructor function (`fn() -> T`). Construction creates
    /// the initial instance immediately. Additional instances are created lazily for each strategy
    /// partition, as determined by `S`, when relocation first reaches that partition.
    /// Each partition therefore obtains its own freshly created `T` without requiring `T: Clone` or
    /// `T: ThreadAware`.
    ///
    /// Requirements:
    /// * `T` must be `Send + 'static` so instances can operate in different strategy partitions.
    /// * The function must create instances that are independent across partitions. Use
    ///   [`new_with`](Self::new_with) when construction depends on state that itself implements
    ///   [`trait@ThreadAware`].
    ///
    /// The constructor participates in destination-value initialization and must obey the reentrant
    /// initialization restriction documented on [`Arc`].
    ///
    /// For example, the counter type we implemented in the documentation for [`trait@ThreadAware`] trait
    /// can be used with `new` by passing the constructor function (note the absence of `()`):
    ///
    /// ```rust
    /// # use thread_aware::{Arc, ThreadAware, PerThread};
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
    /// #     fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {
    /// #         // Initialize a new value in the destination affinity independent
    /// #         // of the source affinity.
    /// #         self.value = sync::Arc::new(AtomicI32::new(0));
    /// #     }
    /// # }
    ///
    /// let container = Arc::<_, PerThread>::new(Counter::new);
    /// let container_clone = container.clone();
    /// container.increment_by(42);
    /// assert_eq!(container.value(), 42);
    /// assert_eq!(container_clone.value(), 42);
    /// ```
    pub fn new(ctor: fn() -> T) -> Self {
        // We wrap the function pointer in a tiny ThreadAwareFnOnce implementation that
        // recreates the value independently for each strategy partition.
        struct Ctor<T> {
            f: fn() -> T,
        }

        impl<T> Clone for Ctor<T> {
            fn clone(&self) -> Self {
                Self { f: self.f }
            }
        }

        impl<T> ThreadAware for Ctor<T> {
            fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
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
    /// Creates an `Arc` with a boxed constructor for unsized values.
    ///
    /// This is the `?Sized`-compatible version of [`new`](Self::new). Use this when `T` is a
    /// trait object (e.g., `dyn Trait`) or other unsized type. The constructor produces a
    /// `Box<T>` which is then stored behind a [`sync::Arc`].
    /// Construction invokes the function immediately for the initial value and at most once for
    /// each additional strategy partition, when relocation first reaches that partition.
    ///
    /// The constructor participates in destination-value initialization and must obey the reentrant
    /// initialization restriction documented on [`Arc`].
    ///
    /// ```rust
    /// # use thread_aware::{Arc, ThreadAware, PerThread};
    /// let arc = Arc::<dyn ThreadAware, PerThread>::new_boxed(|| Box::new(42u32));
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
            fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
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
    /// Creates an `Arc` with a thread-aware constructor input.
    ///
    /// Construction invokes `f` immediately for the initial value. Relocation invokes it at most
    /// once for each additional strategy partition, when that partition is first reached. The
    /// captured `data` behaves like a [`ThreadAwareFnOnce`] input so it can be relocated safely to
    /// the destination affinity before `f` is invoked.
    ///
    /// Relocating `data` and invoking `f` are part of destination-value initialization. Their
    /// implementations must obey the reentrant initialization restriction documented on [`Arc`].
    ///
    /// This function can be used to create an `Arc` of a type that itself doesn't implement
    /// [`trait@ThreadAware`] because each strategy partition receives its own independently
    /// initialized value:
    ///
    /// ```rust
    /// # use std::sync::{self, Mutex};
    /// # use thread_aware::{Arc, PerThread};
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
    /// let container = Arc::<_, PerThread>::new_with((), |_| MyStruct::new());
    /// ```
    ///
    /// The constructor can depend on other values that implement [`trait@ThreadAware`] (this example uses the Counter
    /// defined in [`trait@ThreadAware`] documentation):
    ///
    /// ```rust
    /// # use thread_aware::{ThreadAware, Arc, PerThread};
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
    /// #     fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {
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
    /// let container = Arc::<_, PerThread>::new_with(counter, |counter| MyStruct::new(counter.value()));
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
    /// Creates an `Arc` by cloning the same value for each strategy partition.
    ///
    /// The value must implement [`trait@ThreadAware`] and [`Clone`]. When relocation first reaches an
    /// unmaterialized destination partition, a new value is created by cloning the value carried by
    /// the `Arc` and relocating it to the destination affinity.
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
    /// The value must implement [`Clone`]. When relocation first reaches an unmaterialized
    /// destination partition, a new value is created by cloning the value carried by the `Arc`.
    ///
    /// This is useful for types that do not implement [`trait@ThreadAware`]. In such cases, the value
    /// is cloned once for each strategy partition without any relocation logic.
    ///
    /// [`Clone::clone`] participates in destination-value initialization and must obey the reentrant
    /// initialization restriction documented on [`Arc`].
    ///
    /// For example, the counter type we implemented in the documentation for [`trait@ThreadAware`] trait
    /// can be used with new:
    ///
    /// ```rust
    /// # use thread_aware::{Arc, PerThread};
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
    /// let arc = Arc::<_, PerThread>::new(Counter::new);
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
    /// Creates an `Arc` from a value and a clone function.
    ///
    /// The object passed will be kept, and serves as the template for all subsequent clones
    /// that happen on relocation. A clone is also performed for the initial `Arc`.
    ///
    /// The `clone_fn` receives `&V` (the concrete type) and returns `Box<T>`, enabling
    /// use with `dyn Trait` where `Clone` is not object-safe. Each partition's clone is
    /// [`relocate`](ThreadAware::relocate) to the destination affinity that first materializes it.
    ///
    /// The clone function and the returned value's relocation participate in destination-value
    /// initialization. Their implementations must obey the reentrant initialization restriction
    /// documented on [`Arc`].
    ///
    /// ```rust
    /// # use thread_aware::{Arc, PerThread, ThreadAware};
    /// # #[derive(Clone)]
    /// # struct Foo(u32);
    /// # impl ThreadAware for Foo {
    /// #     fn relocate(&mut self, _: Option<thread_aware::affinity::Affinity>, _: thread_aware::affinity::Affinity) {}
    /// # }
    /// trait MyPlugin: ThreadAware {}
    /// impl MyPlugin for Foo {}
    ///
    /// let arc = Arc::<dyn MyPlugin, PerThread>::with_clone_fn(Foo(42), |v: &Foo| Box::new(v.clone()));
    /// ```
    pub fn with_clone_fn<V: Send + Sync + 'static>(value: V, clone_fn: fn(&V) -> Box<T>) -> Self {
        // In a canonical case, we might have `V = u32`, `T = dyn Foo`, and `clone_fn = |&u32| -> Box<dyn Foo>`.
        let erased = super::clone_fn::ErasedCloneFn::new(value, clone_fn);
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
    /// Creates an `Arc` from a thread-aware boxed closure.
    ///
    /// The closure is invoked immediately for the initial value and at most once for each additional
    /// strategy partition, when relocation first reaches that partition. It is a
    /// [`ThreadAwareFnOnce`] to ensure it captures only values that are safe to transfer.
    ///
    /// Relocating and invoking the closure happen while the destination partition is being
    /// initialized. The closure must not reenter that partition or create cyclic partition
    /// initialization dependencies.
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
    /// [`Storage::insert`] for the strategy partitions that should carry a value, then hand it here
    /// to obtain an `Arc` backed by those values.
    ///
    /// If the resulting `Arc` is relocated to an affinity whose strategy partition has no value in
    /// the storage, it behaves like a [`sync::Arc`].
    ///
    /// # Panics
    /// Panics if `current_affinity` falls outside the storage's coordinate space or its strategy
    /// partition has no value.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc as StdArc;
    ///
    /// use thread_aware::affinity::pinned_affinities;
    /// use thread_aware::storage::Storage;
    /// use thread_aware::{Arc, PerThread};
    ///
    /// let affinity = pinned_affinities(&[2])[0];
    ///
    /// let storage = Storage::new();
    /// storage.insert(affinity, StdArc::new(42)).unwrap();
    ///
    /// let arc = Arc::<_, PerThread>::from_storage(StdArc::new(storage), affinity);
    /// assert_eq!(*arc, 42);
    /// ```
    ///
    /// [`Storage`]: crate::storage::Storage
    /// [`Storage::insert`]: crate::storage::Storage::insert
    pub fn from_storage(storage: sync::Arc<Storage<T, S>>, current_thread: &Thread) -> Self {
        let value = storage.get(current_thread).expect("No data found for the current thread");

        Self {
            storage,
            value,
            factory: Factory::Manual,
        }
    }
}

impl<T, S: Strategy> Arc<T, S> {
    /// Gets the number of strong references to the value in the current strategy partition.
    ///
    /// This method returns the strong reference count for the underlying [`sync::Arc`]
    /// that holds the value carried by this `Arc`, excluding any internal references held by the
    /// storage for deduplication purposes. Affinities that map to the same strategy partition share
    /// one value and reference count; different partitions maintain separate values and counts.
    ///
    /// The count is approximate under concurrent relocation: a relocation publishing this value
    /// into another strategy partition can skew the sample. It saturates at zero rather than
    /// underflowing.
    ///
    /// # Examples
    ///
    /// ```
    /// use thread_aware::{Arc, PerThread};
    ///
    /// let arc = Arc::<_, PerThread>::new(|| 42);
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
    /// Records the original source thread in a closure factory once it is known.
    ///
    /// This is deterministic and runs no caller code. Only the closure factory carries source
    /// state; the other factory kinds are stateless. An unknown `source` records nothing, allowing a
    /// later relocation with a known source to establish the original thread.
    fn record_factory_source(&mut self, source: Option<&Thread>) {
        if let (Factory::Closure(_, recorded @ None), Some(source)) = (&mut self.factory, source) {
            *recorded = Some(source.clone());
        }
    }

    /// Produces the value `destination` will hold by running the configured factory.
    ///
    /// This runs caller-supplied code while the destination partition's entry is held for writing.
    /// Across all racing relocations into the same empty partition, the factory runs at most once
    /// and every racer adopts the one published value — the closure's documented "once per strategy
    /// partition" contract. The factory must not reenter this storage, which would deadlock on that
    /// entry. If it panics, the panic propagates and the partition is left empty for the next
    /// relocation into it to re-materialize.
    fn materialize_value(&self, source: Option<&Thread>, destination: &Thread) -> sync::Arc<T> {
        match &self.factory {
            Factory::Closure(factory, factory_source_thread) => {
                let mut factory_clone = (**factory).clone();

                // Prefer the source thread already recorded in the factory: it is set on the first
                // relocation and is the thread the factory was originally built for. Fall back to
                // `source` on that first relocation, when nothing has been recorded yet.
                let factory_source = factory_source_thread.as_ref().or(source);

                factory_clone.relocate(factory_source, destination);
                sync::Arc::from(factory_clone.call_once())
            }

            // The remaining kinds are stateless: they produce the value and keep themselves.
            Factory::Data(factory) => sync::Arc::from(factory(&self.value, source, destination)),

            Factory::ErasedCloneFn(erased) => erased.clone_and_relocate(source, destination),

            // Manual mode behaves like a plain `sync::Arc<T>`: clone the current value.
            Factory::Manual => sync::Arc::clone(&self.value),
        }
    }
}

impl<T: Send + Sync + ?Sized, S: Strategy + Send + Sync> ThreadAware for Arc<T, S> {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        if source.is_some_and(|source| source.owner() != destination.owner()) {
            return;
        }

        // Record the original source before any fast path can return. A clone whose first relocation
        // hits an existing destination value or stays within one strategy partition still needs that
        // source when a later relocation materializes a new value.
        // Ref: docs/implementation.md, "Relocation and publication".
        self.record_factory_source(source);

        // Relocation reads the destination's partition with a shard read. The steady state of any
        // thread is "already materialized", so this hit is the overwhelmingly common outcome.
        // Cloning the stored `sync::Arc` and dropping the previously carried value still update
        // their strong counts, which can contend when callers share either allocation.
        // Ref: docs/implementation.md, "Relocation and publication".
        if let Some(value) = self.storage.get(destination) {
            self.value = value;
            return;
        }

        // When the source resolves to the destination's own partition there is no cross-partition
        // move: the carried value already belongs to that partition. A single-partition strategy is
        // the same case even without a source — the carried value provably belongs to the one
        // partition — which is the whole of relocation under `PerProcess`. Seed the empty partition
        // with the carried value and keep it, rather than materializing a fresh value that would
        // diverge from the shared one.
        //
        // Only `SINGLE_PARTITION` strategies take that shortcut. Keyed storage cannot tell "this
        // machine happens to have one thread" from "a partition I have not seen yet", so a
        // source-less relocation under `PerThread` or `PerNumaNode` always materializes.
        let same_partition = match source {
            Some(source) => S::key(source) == S::key(destination),
            None => S::SINGLE_PARTITION,
        };
        if same_partition {
            // If a racer published first, the entry already holds its value; adopt it so every clone
            // converges on one identity.
            let published = self.storage.get_or_insert_with(destination, || sync::Arc::<T>::clone(&self.value));
            self.value = published;
            return;
        }

        // The test checkpoint pauses an adopting racer only after its per-clone factory state is
        // current, allowing the publisher to finish before this racer reaches the entry.
        // Production builds execute no additional logic here.
        #[cfg(test)]
        run_after_factory_update_hook();

        // The value this `Arc` carries belongs to `source`; keep a handle to seed the source cell.
        let old_value = sync::Arc::<T>::clone(&self.value);

        // Publish the destination value, running the caller's factory at most once across all racers:
        // the entry is held for writing while materialization runs and hands every racer the single
        // published value, so the closure's documented "once per strategy partition" contract holds
        // even under a concurrent first relocation. The factory must not reenter this storage, which
        // would deadlock on that entry. A panic propagates and leaves the partition empty for the
        // next relocation to retry.
        // Ref: docs/implementation.md, "Relocation and publication".
        let published = self.storage.get_or_insert_with(destination, || self.materialize_value(source, destination));
        self.value = published;

        // Record the value the `Arc` moved away from into the source partition, so a later
        // relocation back into it finds the original instead of materializing a fresh one. Reached
        // only on a cross-partition miss — the same-partition case returned above — so the source
        // partition is distinct from the destination just published. `insert` leaves an
        // already-populated source partition untouched; another thread may have recorded it with the
        // same value, so leaving it in place is correct.
        if let Some(source) = source {
            let _ = self.storage.insert(source, old_value);
        }
    }
}
