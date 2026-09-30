// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Task submission and completion.
//!
//! [`TaskScheduler`] submits asynchronous and blocking tasks. A detached
//! scheduler distributes work round-robin; a worker-bound scheduler preserves
//! affinity. [`LocalTaskScheduler`] accepts non-`Send` captures and results on
//! the associated worker.
//!
//! Futures are created on their destination worker and need not be `Send`.
//! Remote results must be `Send` and are not automatically relocated.
//! [`JoinHandle`] and [`LocalJoinHandle`] receive the result or propagate a task
//! panic. A cancelled task leaves its join pending rather than returning a
//! cancellation error.
//!
//! Pass a factory, such as `async |cx| { /* work */ }`, to
//! [`TaskScheduler::spawn`], rather than an already-created future. The factory
//! receives owned [`Builtins`](crate::runtime::Builtins) on the destination worker.
//! Local factories take no arguments and run on their calling worker.
//!
//! See the [documentation guides](crate#documentation) for scheduling examples
//! and lifecycle rules before coordinating task completion with shutdown.

pub(crate) mod execution;
pub(crate) mod join;
pub(crate) mod local;
pub(crate) mod scheduler;

pub use join::{JoinHandle, LocalJoinHandle};
pub use local::LocalTaskScheduler;
pub use scheduler::TaskScheduler;
