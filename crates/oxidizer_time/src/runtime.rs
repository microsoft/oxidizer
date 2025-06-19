// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The runtime module provides the necessary infrastructure to integrate the [`Clock`][crate::Clock]
//! into the async runtime.

mod clock_driver;
mod inactive_clock;
#[cfg(test)]
mod mini_runtime;

pub use clock_driver::ClockDriver;
pub use inactive_clock::InactiveClock;
#[cfg(test)]
pub use mini_runtime::*;