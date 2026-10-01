// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Functionality for testing, examples and benchmarks.
//!
//! Publicly exposed via the `test-util` Cargo feature.

#[cfg(any(test, feature = "test-util"))]
mod functions;
#[cfg(any(test, feature = "test-util"))]
pub use functions::*;

/// Configured capacity of the executor's awakened-task queue for this build.
///
/// Use this to size overflow scenarios in tests and benchmarks. The value can vary
/// between build configurations.
#[cfg(any(test, feature = "test-util"))]
pub const AWAKENED_CAPACITY: usize = crate::AWAKENED_CAPACITY;

#[cfg(test)]
mod test_subject_future;
#[cfg(test)]
pub(crate) use test_subject_future::*;

#[cfg(test)]
mod test_waker;
#[cfg(test)]
pub(crate) use test_waker::*;
