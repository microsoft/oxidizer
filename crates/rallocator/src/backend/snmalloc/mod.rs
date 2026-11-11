// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The Rust snmalloc port and its cooperative state observations.

#[cfg(all(test, target_os = "windows"))]
pub(crate) use hal::disable_test_crash_reporting;

pub(crate) use crate::backend::abort;
mod allocator;
mod buddy;
mod classes;
mod combining;
mod core;
mod hal;
mod observation;
mod pagemap;
mod ranges;
mod remote;
mod snapshot;
mod thread;

pub use allocator::Snmalloc;
