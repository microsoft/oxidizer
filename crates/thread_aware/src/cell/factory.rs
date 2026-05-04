// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::{fmt, sync};

use crate::ThreadAware;
use crate::affinity::Affinity;
use crate::closure::ErasedClosureOnce;

/// A function that clones data and optionally relocates the clone.
///
/// For `ThreadAware` types, the function clones and calls `relocated()`.
/// For non-`ThreadAware` types, it just clones (ignoring source/destination).
pub type DataFn<T> = fn(&T, Option<Affinity>, Affinity) -> Box<T>;

/// Object-safe trait for cloning a value of erased concrete type `V` into `Box<T>`,
/// then relocating the clone.
pub(crate) trait ErasedClone<T: ?Sized>: Send + Sync {
    fn clone_and_relocate(&self, value: &T, source: Option<Affinity>, destination: Affinity) -> Box<T>;
}

/// Concrete implementation that remembers `V` and the user-provided clone function.
struct CloneAdapter<V, T: ?Sized> {
    // Converts for example `&String` to `Box<dyn Foo>`.
    clone_fn: fn(&V) -> Box<T>,
}

impl<V: 'static, T: ThreadAware + ?Sized + 'static> ErasedClone<T> for CloneAdapter<V, T> {
    fn clone_and_relocate(&self, value: &T, source: Option<Affinity>, destination: Affinity) -> Box<T> {
        // SAFETY: we know the concrete type behind `T` is `V` because we stored it
        // at construction time and the value in the Arc was created from a `V`.
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

// SAFETY: CloneAdapter only holds a fn pointer which is Send + Sync.
unsafe impl<V, T: ?Sized> Send for CloneAdapter<V, T> {}
// SAFETY: CloneAdapter only holds a fn pointer which is Send + Sync.
unsafe impl<V, T: ?Sized> Sync for CloneAdapter<V, T> {}

pub enum Factory<T: ?Sized> {
    /// An external closure was provided to create the data.
    Closure(sync::Arc<ErasedClosureOnce<Box<T>>>, Option<Affinity>),

    /// The data will be cloned and relocated via `DataFn`.
    Data(DataFn<T>),

    /// The data will be cloned and relocated via a type-erased clone function.
    ErasedCloneFn(sync::Arc<dyn ErasedClone<T>>),

    Manual,
}

impl<T: ?Sized> fmt::Debug for Factory<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closure(_, _) => f.debug_tuple("Closure").finish(),
            Self::Data(_) => f.debug_tuple("Data").finish(),
            Self::ErasedCloneFn(_) => f.debug_tuple("Clone").finish(),
            Self::Manual => write!(f, "Manual"),
        }
    }
}

impl<T: ?Sized> Clone for Factory<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Closure(closure, closure_source) => Self::Closure(sync::Arc::clone(closure), *closure_source),
            Self::Data(data_fn) => Self::Data(*data_fn),
            Self::ErasedCloneFn(erased) => Self::ErasedCloneFn(sync::Arc::clone(erased)),
            Self::Manual => Self::Manual,
        }
    }
}

impl<T: ThreadAware + ?Sized + 'static> Factory<T> {
    /// Creates a new `Clone` factory from a concrete type `V` and a clone function.
    pub(crate) fn new_erased_clone_fn<V: 'static>(clone_fn: fn(&V) -> Box<T>) -> Self {
        Self::ErasedCloneFn(sync::Arc::new(CloneAdapter { clone_fn }))
    }
}
