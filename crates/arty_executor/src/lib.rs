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
//! The executor is a building block for an application runtime that provides foundational
//! capabilities like task execution, multithreaded task management, I/O, timers, and more.
//! Application logic may encounter [`JoinHandle`]s but otherwise has no direct interaction
//! with the executor.
//!
//! # Design tenets
//!
//! The executor is single-threaded. If you want to execute tasks on multiple threads, you need to
//! run multiple executors on different threads. If you wish to observe task results across
//! threads, you need to create a mechanism to ship the results across thread boundaries.
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
mod constants;
mod cycle_outcome;
mod executor;
mod executor_core;
mod join_handle;
mod panic;
mod ptr_hash;
mod task;
mod task_options;
mod task_ref;
mod task_set;
mod telemetry;
mod wake;

pub use builder::*;
pub(crate) use constants::*;
pub use cycle_outcome::*;
pub use executor::*;
pub(crate) use executor_core::*;
pub use join_handle::*;
pub use panic::*;
pub(crate) use ptr_hash::*;
pub(crate) use task::*;
pub use task_options::*;
pub(crate) use task_ref::*;
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod task_mock;
#[cfg(test)]
pub(crate) use task_mock::*;
