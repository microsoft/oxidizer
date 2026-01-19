// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A flag indicating that the required property is set.
#[non_exhaustive]
#[derive(Debug)]
pub struct Set;

/// A flag indicating that the required property has not been set.
#[non_exhaustive]
#[derive(Debug)]
pub struct NotSet;

crate::define_fn_wrapper!(EnableIf<In>(Fn(&In) -> bool));

impl<In> EnableIf<In> {
    /// Creates a new `EnableIf` instance that always returns `true`.
    pub fn always() -> Self {
        Self::new(|_| true)
    }

    /// Creates a new `EnableIf` instance that always returns `false`.
    pub fn never() -> Self {
        Self::new(|_| false)
    }
}

mod context;
pub use context::Context;

mod define_fn_wrapper;
pub(crate) use define_fn_wrapper::define_fn_wrapper;

mod attempt;
mod backoff;

pub use attempt::Attempt;
#[cfg(any(feature = "retry", test))]
pub(crate) use attempt::MaxAttempts;
pub use backoff::Backoff;
