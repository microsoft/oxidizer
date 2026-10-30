// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Starting tasks, choosing workers, and receiving results.
//!
//! Choose how Arty places a new task:
//!
//! - [`RuntimeScheduler`] submits work from outside the runtime and lets the
//!   runtime select a worker.
//! - [`Scheduler::spawn`] starts an ordinary child task on the scheduler's
//!   worker.
//! - [`Scheduler::spawn_anywhere`] lets the runtime select a worker.
//! - [`Scheduler::spawn_at`] uses a worker from
//!   [`RuntimeControl::workers`](crate::runtime::RuntimeControl::workers).
//!
//! A task factory is a function that creates a future.
//! Task APIs accept these factories instead of futures you already created.
//! Arty calls the factory on the selected worker.
//! The future can then keep non-[`Send`] state across awaits.
//! Values sent to a task must meet the trait requirements listed by the method.
//!
//! Relocation updates a value's state for a selected worker.
//! [`Scheduler::spawn_anywhere`] and [`Scheduler::spawn_at`] relocate the
//! [`ThreadAware`](crate::core::ThreadAware) data you pass to them.
//! Their results must also implement `ThreadAware`.
//! [`RuntimeScheduler::spawn_anywhere`] does not relocate its result.
//! Each method explains how it rejects tasks and returns results.
//!
//! Use [`Scheduler::spawn_blocking`] or [`RuntimeScheduler::spawn_blocking`] for
//! synchronous calls that can stop an async worker from running other tasks.
//! Blocking functions run on separate threads in
//! pools configured by [`BlockingPoolPolicy`](crate::runtime::BlockingPoolPolicy).
//! Each function uses one pool thread until it returns.
//!
//! Await [`JoinHandle`] to receive a result. [`JoinError`] reports task panic,
//! shutdown, or an invalid destination. Dropping a `JoinHandle` does not cancel
//! its task. Keep the [`Runtime`](crate::runtime::Runtime) alive until required
//! tasks finish.
//!
//! # Example
//!
//! Create state on an async worker and run a blocking function
//! outside the async worker. This example requires `macros`, which enables
//! `rt`:
//!
//! ```
//! use std::rc::Rc;
//!
//! # #[cfg(all(feature = "macros", feature = "rt"))]
//! # #[arty::main]
//! # async fn main(
//! #     cx: arty::task::Context,
//! # ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! let local = cx.scheduler().spawn(|_| {
//!     let value = Rc::new(40);
//!     async move { *value }
//! });
//! let blocking = cx.scheduler().spawn_blocking(|| 2);
//! assert_eq!(local.await? + blocking.await?, 42);
//! # Ok(())
//! # }
//! # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
//! ```

pub(crate) mod context;
pub(crate) mod handle;
pub(crate) mod preparation;
mod runtime_scheduler;
pub(crate) mod scheduler;

#[doc(inline)]
pub use context::Context;
#[doc(inline)]
pub use handle::{JoinError, JoinHandle};
#[doc(inline)]
pub use runtime_scheduler::RuntimeScheduler;
#[doc(inline)]
pub use scheduler::Scheduler;
