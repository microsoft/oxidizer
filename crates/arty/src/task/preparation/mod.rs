// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Task preparation and deferred execution wrappers.
//!
//! Submission code owns admission decisions. These helpers construct executable
//! payloads and their result channels without deciding whether work is accepted.

mod blocking;
mod cleanup;
mod deferred_task;
mod remote;

#[cfg(any(test, not(feature = "monitor")))]
pub(crate) use blocking::prepare_blocking;
#[cfg(feature = "monitor")]
pub(crate) use blocking::prepare_blocking_with_info;
pub(crate) use cleanup::discard_panicking_drop;
pub(crate) use deferred_task::DeferredTask;
pub(crate) use remote::{RemoteFutureFactory, prepare_remote};

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests;
