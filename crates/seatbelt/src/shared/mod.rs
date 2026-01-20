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

mod context;
pub use context::Context;

mod attempt;
mod backoff;

pub use attempt::Attempt;
#[cfg(any(feature = "retry", test))]
pub(crate) use attempt::MaxAttempts;
pub use backoff::Backoff;
