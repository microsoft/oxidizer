// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{MemoryAffinity, PinnedAffinity, closure::ErasedClosureOnce};
use std::sync;

pub type DataFn<T> = fn(&T, MemoryAffinity, PinnedAffinity) -> T;

#[derive(Debug)]
pub enum Factory<T> {
    /// An external closure was provided to create the data.
    Closure(sync::Arc<ErasedClosureOnce<T>>, Option<MemoryAffinity>),

    /// The data is `ThreadAware` + Clone and will be cloned and transferred.
    Data(DataFn<T>),

    Manual,
}

impl<T> Clone for Factory<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Closure(closure, closure_source) => Self::Closure(sync::Arc::clone(closure), *closure_source),
            Self::Data(data_fn) => Self::Data(*data_fn),
            Self::Manual => Self::Manual,
        }
    }
}
