// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(any(test, feature = "unstable-testing"))]

//! This module contains testing utilities used in unit tests, examples and integration tests.
//! This is not an officially supported API and may change at any time.
//!
//! The non-cfg(test) contents are published only when the `unstable-testing` feature is enabled.

#[cfg(test)]
mod simulated_completion_queue;
#[cfg(test)]
pub(crate) use simulated_completion_queue::*;

#[cfg(test)]
mod unit_test_helpers;
#[cfg(test)]
pub(crate) use unit_test_helpers::*;

mod functions;
pub use functions::*;

mod io_pump_entrypoint;
pub use io_pump_entrypoint::*;

mod test_runtime;
pub use test_runtime::*;