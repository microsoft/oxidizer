// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Task scheduling and result handles.
//!
//! Borrow a [`RuntimeScheduler`] from
//! [`Runtime::scheduler`](crate::runtime::Runtime::scheduler) to let the runtime
//! place a new task. Each task receives [`Context`] with its worker's services.
//! [`Context::scheduler`] keeps child tasks on the same worker.
//!
//! Pass a factory, not an already-created future. Arty creates the future on
//! its worker, where it can retain non-[`Send`] state. Factory captures and
//! results cross the submission boundary and must be `Send`. Use
//! [`ThreadAware`](crate::core::ThreadAware) data with `spawn_anywhere` when
//! the runtime should choose a worker.
//!
//! Await a [`JoinHandle`] to receive a task's result. Panics and shutdown
//! cancellation return [`JoinError`] to the caller, rather than unwinding it.
//! A task returning `Result<T, E>` has two error layers: joining it returns
//! `Result<Result<T, E>, JoinError>`.
//!
//! # Examples
//!
//! Submit a child task and await its result:
//!
//! ```
//! # #[cfg(all(feature = "macros", feature = "rt"))]
//! # #[arty::main]
//! # async fn main(cx: arty::task::Context) -> Result<(), arty::task::JoinError> {
//! let task = cx.scheduler().spawn(async |_| 6 * 7);
//! assert_eq!(task.await?, 42);
//! # Ok(())
//! # }
//! # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
//! ```
//!
//! Await work you need before shutdown, which cancels pending tasks. Dropping
//! a join handle does not cancel its task. See [scheduling](crate::documentation::scheduling)
//! and [thread awareness](crate::documentation::thread_awareness) for examples.

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
