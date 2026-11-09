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
//! After a task starts, it stays on its assigned worker. If it awaits an
//! operation that is not ready, that worker can poll another ready task
//! assigned to it.
//!
//! Task APIs accept a factory, a function that creates a future.
//! Arty runs the factory on the selected worker, so the future can keep
//! non-[`Send`] state across awaits.
//!
//! Relocation updates a value's state for a selected worker.
//! [`Scheduler::spawn_anywhere`] and [`Scheduler::spawn_at`] relocate the
//! [`ThreadAware`](crate::core::ThreadAware) data passed to their factories
//! before the factories run. The data and result must implement `ThreadAware`.
//! When an Arty worker awaits the result, Arty relocates it to that worker.
//! [`RuntimeScheduler::spawn_anywhere`] does not relocate its result.
//!
//! Use [`Scheduler::spawn_blocking`] or [`RuntimeScheduler::spawn_blocking`] for
//! synchronous calls that can stop an async worker from running other tasks.
//! Blocking functions run on separate threads in
//! pools configured by [`BlockingPoolPolicy`](crate::runtime::BlockingPoolPolicy).
//!
//! Await [`JoinHandle`] to receive a result or [`JoinError`]. Dropping a
//! `JoinHandle` does not cancel its task.
//!
//! # Examples
//!
//! Let Arty choose a worker and send it a clock and a number. The task waits
//! for 1 ms, then returns the number:
//!
//! ```no_run
//! # #[cfg(all(feature = "macros", feature = "rt"))]
//! # #[arty::main]
//! # async fn main(cx: arty::task::Context) -> Result<(), arty::task::JoinError> {
//! let answer = cx
//!     .scheduler()
//!     .spawn_anywhere((cx.clock().clone(), 42_u32), |(clock, answer)| async move {
//!         clock.delay(std::time::Duration::from_millis(1)).await;
//!         answer
//!     })
//!     .await?;
//! assert_eq!(answer, 42);
//! # Ok(())
//! # }
//! # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
//! ```
//!
//! Tuples implement `ThreadAware` when every element does. [`Clock`](crate::time::Clock)
//! and `u32` implement it here. Arty relocates the clock before the factory
//! runs.
//!
//! Create state on an async worker and run a blocking function
//! outside the async worker. This example requires `macros`, which enables
//! `rt`:
//!
//! ```
//! use std::rc::Rc;
//!
//! # #[cfg(all(feature = "macros", feature = "rt"))]
//! # #[arty::main(monitor = disabled)]
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
