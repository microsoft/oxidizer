// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A flag indicating that the required property is set.
#[non_exhaustive]
#[derive(Debug)]
#[doc(hidden)]
pub struct Set;

/// A flag indicating that the required property has not been set.
#[non_exhaustive]
#[derive(Debug)]
#[doc(hidden)]
pub struct NotSet;

mod context;
pub use context::ResilienceContext;

mod attempt;
mod backoff;

pub use attempt::Attempt;
#[cfg(any(feature = "retry", test))]
pub(crate) use attempt::MaxAttempts;
pub use backoff::Backoff;
