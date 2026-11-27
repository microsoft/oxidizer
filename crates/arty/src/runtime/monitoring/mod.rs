// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Private task monitoring and native task inspection.
//!
//! The runtime owns its monitor connection and registration until its workers stop.
//! Each async worker owns a worker registration and its current poll state.
//! Blocking queues own worker registrations and runtime handles for submissions.
//! Each submission creates an immutable identity from its caller's source location.
//! The executor stores each async task's identity and sets it during polls.
//! Blocking tasks report their queue and execution lifecycle without task inspection.
//! Handles provide access to these registrations but do not own runtime shutdown.

mod blocking;
mod runtime;
mod task;

#[cfg(test)]
#[cfg(not(miri))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;

// Hold for the full lifetime of tests that use the process registry or recorder.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) use blocking::BlockingJob;
pub(crate) use runtime::Monitoring;
pub(crate) use task::current_origin;
