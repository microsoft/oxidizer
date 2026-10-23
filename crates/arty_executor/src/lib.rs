// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// TODO(doc-coverage): remove once `missing_docs` is promoted to [workspace.lints.rust].
#![deny(missing_docs)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Async task executor for the Arty Runtime.
//!
//! `arty_executor` is an internal implementation detail of the [`arty`](https://docs.rs/arty)
//! crate. It is responsible for every stage of task execution, including registration, polling,
//! wake processing, cancellation, completion, telemetry, and panic handling. Applications should
//! use `arty` instead of depending on this crate directly.
//!
//! The purpose of the executor is to ensure that async tasks registered with the executor make
//! progress, quickly and efficiently reacting to `await` statements that complete.
//!
//! The executor is the internal building block through which `arty` implements task execution.
//! Application code interacts with `arty`'s own task and join APIs and has no direct interaction
//! with this crate.
//!
//! # Design tenets
//!
//! The executor is single-threaded. If you want to execute tasks on multiple threads, you need to
//! run one executor on each thread. At most one executor may be alive on a thread; a replacement
//! may be created after the previous executor is dropped. The [`remote`] module provides pooled
//! channels for the common runtime-integration scenario of returning task results to a different
//! thread.
//!
//! Tasks cannot be taken out of the executor - the only way for them to end is to either end
//! naturally (with the future returning `Poll::Ready`) or for the executor to be shut down. Not
//! only is there no "remove" function but similarly, there is no "cancel" function - once a task
//! has started executing, the only thing that can terminate it is the task itself, by completing.
//!
//! In a steady state, the executor is allocation-free, as all memory used by the executor is
//! reused for new tasks when old ones complete.
//!
//! Cloned task wakers may outlive their tasks. After completion or cancellation, an escaped waker
//! remains callable but becomes an inert no-op and cannot delay executor shutdown.
//!
//! Join handles may outlive the executor and report shutdown cancellation through [`JoinError`].
//! A panic in a directly registered executor task remains fail-fast.

mod builder;
#[cfg(all(debug_assertions, test))]
mod constants;
mod cycle_outcome;
mod executor;
mod executor_context;
mod executor_shared;
mod join_handle;
mod panic;
pub mod remote;
mod task;
mod task_handle;
mod task_list;
mod task_options;
mod task_set;
mod telemetry;
mod wake;

pub use builder::*;
#[cfg(all(debug_assertions, test))]
pub(crate) use constants::*;
pub use cycle_outcome::*;
pub use executor::*;
pub(crate) use executor_shared::*;
pub use join_handle::*;
pub use panic::*;
pub(crate) use task::*;
pub(crate) use task_handle::*;
pub(crate) use task_list::*;
pub use task_options::*;
pub use task_set::*;
pub(crate) use wake::*;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod testing;

#[cfg(all(debug_assertions, test))]
mod wake_diagnostic;
#[cfg(all(debug_assertions, test))]
pub(crate) use wake_diagnostic::*;

#[cfg(test)]
testing_aids::init_tracing!();
