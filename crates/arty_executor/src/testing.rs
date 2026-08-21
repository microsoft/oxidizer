// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Functionality for testing, examples and benchmarks.
//!
//! Publicly exposed via the `test-util` Cargo feature.

#[cfg(any(test, feature = "test-util"))]
mod functions;
#[cfg(any(test, feature = "test-util"))]
pub use functions::*;

#[cfg(test)]
mod test_subject_future;
#[cfg(test)]
pub(crate) use test_subject_future::*;

#[cfg(test)]
mod test_waker;
#[cfg(test)]
pub(crate) use test_waker::*;
