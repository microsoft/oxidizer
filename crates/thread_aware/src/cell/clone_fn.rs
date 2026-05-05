// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Self-contained type-erased clonable value.
//!
//! [`ErasedCloneFn<T>`] pairs a `sync::Arc<T>` (where `T` is typically `dyn Trait`)
//! with the clone function for its hidden concrete type `V`. Because both are
//! created together and the fields are private, the unsafe `&T` to `&V` cast
//! inside `clone_and_relocate` is sound at the module boundary - no outside
//! code can construct an `ErasedValue` with mismatched types.

use std::{fmt, sync};

use crate::ThreadAware;
use crate::affinity::Affinity;

/// A type-erased clonable value that pairs `sync::Arc<T>` with the clone
/// function for its hidden concrete type `V`.
///
/// The concrete type `V` is forgotten at the type level but remembered by
/// the internal [`CloneAdapter`]. Because both the value and the adapter
/// are created together in [`ErasedCloneFn::new`], and the fields are private,
/// the invariant that the `Arc<T>` is backed by `V` is upheld by construction.
pub(crate) struct ErasedCloneFn<T: ?Sized> {
    value: sync::Arc<T>,
    adapter: sync::Arc<dyn ErasedClone<T>>,
}

impl<T: ThreadAware + ?Sized + 'static> ErasedCloneFn<T> {
    /// Creates a new `ErasedValue` from a concrete value `V` and a clone function.
    ///
    /// The clone function is called immediately to produce the initial `Box<T>`,
    /// and the original `V` is dropped. Every subsequent `clone_and_relocate`
    /// uses the same `clone_fn`, preserving the `V` invariant.
    pub(crate) fn new<V: Send + 'static>(value: V, clone_fn: fn(&V) -> Box<T>) -> Self {
        let initial = clone_fn(&value);
        drop(value);

        Self {
            value: sync::Arc::from(initial),
            adapter: sync::Arc::new(CloneAdapter::<V, T> { clone_fn }),
        }
    }
}

impl<T: ?Sized> ErasedCloneFn<T> {
    /// Clones the inner value, relocates the clone, and returns a new
    /// `sync::Arc<T>` backed by the relocated clone.
    pub(crate) fn clone_and_relocate(&self, source: Option<Affinity>, destination: Affinity) -> sync::Arc<T> {
        let cloned = self.adapter.clone_and_relocate(&self.value, source, destination);
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

/// Object-safe trait for cloning a value of erased concrete type `V` into `Box<T>`.
trait ErasedClone<T: ?Sized>: Send + Sync {
    fn clone_and_relocate(&self, value: &T, source: Option<Affinity>, destination: Affinity) -> Box<T>;
}

/// Concrete implementation that remembers `V` and the user-provided clone function.
struct CloneAdapter<V, T: ?Sized> {
    clone_fn: fn(&V) -> Box<T>,
}

impl<V: 'static, T: ThreadAware + ?Sized + 'static> ErasedClone<T> for CloneAdapter<V, T> {
    fn clone_and_relocate(&self, value: &T, source: Option<Affinity>, destination: Affinity) -> Box<T> {
        // SAFETY: The concrete type behind `&T` is always `V` because:
        // 1. `CloneAdapter<V, T>` is private to this module
        // 2. It is only constructed in `ErasedValue::new<V>`, paired with an `Arc<T>` created
        //    from the same `clone_fn(&V) -> Box<T>`
        // 3. Every subsequent `Arc<T>` produced by `clone_and_relocate` also goes through the
        //    same `clone_fn`, so the concrete type remains `V`
        // 4. No outside code can construct an `ErasedValue` with mismatched types because
        //    both fields are private and only `new` creates instances
        let concrete = unsafe { &*(std::ptr::from_ref(value).cast::<V>()) };
        let mut cloned = (self.clone_fn)(concrete);
        cloned.relocate(source, destination);
        cloned
    }
}

impl<V, T: ?Sized> fmt::Debug for CloneAdapter<V, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CloneAdapter").finish_non_exhaustive()
    }
}

// SAFETY: CloneAdapter only holds a fn pointer which is inherently Send + Sync.
unsafe impl<V, T: ?Sized> Send for CloneAdapter<V, T> {}

// SAFETY: CloneAdapter only holds a fn pointer which is inherently Send + Sync.
unsafe impl<V, T: ?Sized> Sync for CloneAdapter<V, T> {}
