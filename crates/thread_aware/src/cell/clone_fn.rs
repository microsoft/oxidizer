// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Self-contained type-erased clonable value.
//!
//! [`ErasedCloneFn<T>`] pairs a `sync::Arc<T>` (where `T` is typically `dyn Trait`)
//! with a [`CloneAdapter`] that stores the original concrete value `V` and its
//! clone function.

use std::{fmt, sync};

use crate::affinity::Affinity;
use crate::ThreadAware;

/// A type-erased clonable value that pairs `sync::Arc<T>` with a
/// [`CloneAdapter`] storing the concrete `V` and its clone function.
pub(crate) struct ErasedCloneFn<T: ?Sized> {
    // In a canonical case, we might have `T = dyn Foo`
    value: sync::Arc<T>, // `Arc<dyn Foo>`
    adapter: sync::Arc<dyn ErasedClone<T>>, // `Arc<dyn ErasedClone<dyn Foo>>` == `Arc<CloneAdapter<u32, dyn Foo>>`
}

impl<T: ThreadAware + ?Sized + 'static> ErasedCloneFn<T> {
    /// Creates a new `ErasedCloneFn` from a concrete value `V` and a clone function.
    ///
    /// The clone function is called to produce the initial `Arc<T>` for dereferencing.
    /// The original `V` is stored inside the adapter for future cloning.
    pub(crate) fn new<V: Send + Sync + 'static>(value: V, clone_fn: fn(&V) -> Box<T>) -> Self {
        // In a canonical case, we might have `V = u32`, `T = dyn Foo`, and `clone_fn = |&u32| -> Box<dyn Foo>`.

        // This here will produce a new Box<dyn Foo> (with the ptr bit pointing to u32). With an
        // aberrant clone_fn this would secretly become another type, e.g., `String`, and / or could
        // change type even every time `clone_fn` is called again.
        let initial = clone_fn(&value);

        Self {
            value: sync::Arc::from(initial),
            adapter: sync::Arc::new(CloneAdapter { concrete: value, clone_fn }),
        }
    }
}

impl<T: ?Sized> ErasedCloneFn<T> {
    /// Clones the inner value, relocates the clone, and returns a new
    /// `sync::Arc<T>` backed by the relocated clone.
    pub(crate) fn clone_and_relocate(&self, source: Option<Affinity>, destination: Affinity) -> sync::Arc<T> {
        // In a canonical case, we might have `V = u32`, `T = dyn Foo`. This will have called the erased
        // clone_fn, and returned us a new, relocated instance of `Box<dyn Foo>`, which we return as a new `Arc<dyn Foo>`, which
        // will essentially become the new instance after relocation on the new thread. The implication here is
        // that only ever the first `V` is stored and used for subsequent clones, regardless of which other
        // thread / arc they originate from.
        let cloned = self.adapter.clone_and_relocate(source, destination);
        sync::Arc::from(cloned)
    }

    /// Returns a reference to the inner `Arc<T>`.
    pub(crate) fn arc(&self) -> &sync::Arc<T> {
        &self.value
    }
}

impl<T: ?Sized> fmt::Debug for ErasedCloneFn<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ErasedCloneFn").finish_non_exhaustive()
    }
}

impl<T: ?Sized> Clone for ErasedCloneFn<T> {
    fn clone(&self) -> Self {
        Self {
            value: sync::Arc::clone(&self.value),
            adapter: sync::Arc::clone(&self.adapter),
        }
    }
}

/// Object-safe trait for cloning a stored concrete value into `Box<T>`.
trait ErasedClone<T: ?Sized>: Send + Sync {
    fn clone_and_relocate(&self, source: Option<Affinity>, destination: Affinity) -> Box<T>;
}

/// Stores the concrete `V` alongside the user-provided clone function.
struct CloneAdapter<V, T: ?Sized> {
    // In a canonical case, we might have `V = u32`, `T = dyn Foo`, and `clone_fn = |&u32| -> Box<dyn Foo>`.
    // The value stored here is the result of the first time calling `clone_fn`. Again, with an aberrant
    // clone_fn the concrete type underlying `T` (if it is a `dyn X`) might change.
    concrete: V,
    clone_fn: fn(&V) -> Box<T>,
}

impl<V: Send + Sync + 'static, T: ThreadAware + ?Sized + 'static> ErasedClone<T> for CloneAdapter<V, T> {
    fn clone_and_relocate(&self, source: Option<Affinity>, destination: Affinity) -> Box<T> {
        // In a canonical case, we might have `V = u32`, `T = dyn Foo`, and `clone_fn = |&u32| -> Box<dyn Foo>`.
        // This will invoke the clone function on the last known `V` produced by the clone_fn. On the first
        // relocate this will produce a new `Box<dyn Foo>` based on that value, then relocate and return it.
        let mut cloned = (self.clone_fn)(&self.concrete);
        cloned.relocate(source, destination);
        cloned
    }
}

impl<V, T: ?Sized> fmt::Debug for CloneAdapter<V, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CloneAdapter").finish_non_exhaustive()
    }
}
