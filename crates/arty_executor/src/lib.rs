// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// TODO(doc-coverage): remove once `missing_docs` is promoted to [workspace.lints.rust].
#![deny(missing_docs)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Single-threaded async task executor for the Arty Runtime.
//!
//! `arty_executor` is an internal part of the [`arty`](https://docs.rs/arty) crate.
//! It registers, polls, wakes, cancels, and completes tasks. It also reports telemetry
//! and contains task panics. Applications use `arty` instead of this crate.
//!
//! `arty` uses this crate to run async tasks. Application code uses the task and join
//! APIs from `arty`.
//!
//! # Design
//!
//! The executor runs on one thread. Run one executor on each thread that needs to execute
//! tasks. Only one executor can be alive on a thread. You can create a replacement after
//! you drop the previous executor. A runtime integration supplies a [`TaskCompletion`]
//! target when it registers a task.
//!
//! A task ends when its future returns `Poll::Ready`, when it panics, or when
//! executor shutdown cancels it. The executor does not provide methods to
//! remove or cancel one task.
//!
//! During steady operation, the executor reuses task storage and does not allocate.
//!
//! A cloned task waker can outlive its task. After task completion or cancellation,
//! the waker does nothing and cannot delay executor shutdown.
//!
//! The executor contains task panics. Use
//! [`ExecutorBuilder::panic_policy`](crate::ExecutorBuilder::panic_policy) to select
//! whether a task panic starts executor shutdown.

mod builder;
mod cycle_outcome;
mod executor;
mod executor_context;
mod executor_shared;
mod panic;
mod panic_policy;
mod shutdown;
mod task;
mod task_handle;
mod task_list;
mod task_options;
mod task_set;
mod telemetry;
mod wake;

pub use builder::*;
pub use cycle_outcome::*;
pub use executor::*;
pub(crate) use executor_shared::*;
pub use panic::*;
pub use panic_policy::*;
pub use shutdown::*;
pub use task::TaskCompletion;
pub(crate) use task::*;
pub(crate) use task_handle::*;
pub(crate) use task_list::*;
pub use task_options::*;
pub use task_set::*;

mod inspection;
#[cfg(feature = "inspection")]
pub use inspection::{InspectionError, InspectionStatus, PollObserver, TaskState};
pub(crate) use wake::*;
