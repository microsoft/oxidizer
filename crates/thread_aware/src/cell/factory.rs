// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::{fmt, sync};

use crate::affinity::Affinity;
use crate::closure::ErasedClosureOnce;

use super::clone_fn::ErasedCloneFn;

/// A function that clones data and optionally relocates the clone.
///
/// For `ThreadAware` types, the function clones and calls `relocate()`.
/// For non-`ThreadAware` types, it just clones (ignoring source/destination).
pub type DataFn<T> = fn(&T, Option<Affinity>, Affinity) -> Box<T>;

pub(crate) enum Factory<T: ?Sized> {
    /// An external closure was provided to create the data.
    Closure(sync::Arc<ErasedClosureOnce<Box<T>>>, Option<Affinity>),

    /// The data will be cloned and relocated via `DataFn`.
    Data(DataFn<T>),

    /// The data will be cloned and relocated via a type-erased clone function.
    /// Soundness of the internal unsafe cast is fully encapsulated by `ErasedValue`.
    ErasedCloneFn(ErasedCloneFn<T>),

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
            Self::ErasedCloneFn(erased) => Self::ErasedCloneFn(erased.clone()),
            Self::Manual => Self::Manual,
        }
    }
}
