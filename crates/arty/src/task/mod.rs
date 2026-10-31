// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Task placement, worker-local futures, and result handles.
//!
//! [`RuntimeScheduler`] submits work from outside the runtime and lets the
//! runtime select a worker. [`Scheduler`] stays bound to one worker and keeps
//! ordinary child tasks there. [`Scheduler::spawn_at`] targets a worker from
//! [`RuntimeControl::workers`](crate::runtime::RuntimeControl::workers).
//!
//! Task APIs accept factories instead of created futures. Arty calls the
//! factory on the destination worker. The future can then keep non-[`Send`]
//! state across awaits. Values that cross the submission boundary must meet the
//! bounds documented by the selected method.
//!
//! [`Scheduler::spawn_anywhere`] and [`Scheduler::spawn_at`] relocate explicit
//! [`ThreadAware`](crate::core::ThreadAware) payloads. Their worker-bound result
//! types are also thread-aware. [`RuntimeScheduler::spawn_anywhere`] does not
//! relocate its result. See each method for rejection and join behavior.
//!
//! Use [`Scheduler::spawn_blocking`] or [`RuntimeScheduler::spawn_blocking`] for
//! synchronous calls that can stall an async worker. Blocking callbacks run in
//! pools configured by [`BlockingPoolPolicy`](crate::runtime::BlockingPoolPolicy).
//! A callback owns one pool thread until it returns.
//!
//! Await [`JoinHandle`] to receive a result. [`JoinError`] reports task panic,
//! shutdown, or an invalid destination. Dropping a join handle does not cancel
//! its task. Keep the [`Runtime`](crate::runtime::Runtime) alive until required
//! work completes.
//!
//! # Example
//!
//! Create worker-local state in an async task and run a blocking callback
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
