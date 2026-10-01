// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Task scheduling and result handles.
//!
//! Borrow a [`RuntimeScheduler`] from
//! [`Runtime::scheduler`](crate::runtime::Runtime::scheduler)
//! to distribute work across workers. Each task receives [`Builtins`] containing
//! its worker's services. [`Builtins::scheduler`] returns a [`TaskScheduler`]
//! that keeps child tasks on the same worker.
//!
//! Pass a factory, such as `async |cx| { /* work */ }`, rather than an
//! already-created future. Arty invokes it on the destination worker, so the
//! future can retain non-[`Send`] state. Use [`LocalTaskScheduler`] when the
//! factory's captures or its result also need to be non-`Send`.
//!
//! Await a [`JoinHandle`] or [`LocalJoinHandle`] to receive a task's result.
//! Panics and shutdown cancellation produce [`JoinError`] instead of unwinding
//! the joining caller. If the task returns its own `Result<T, E>`, joining it
//! produces `Result<Result<T, E>, JoinError>`.
//!
//! # Examples
//!
//! Submit a child task and await its result:
//!
//! ```
//! # #[cfg(feature = "macros")]
//! #[arty::main]
//! async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
//!     let task = cx.scheduler().spawn(async |_| 6 * 7);
//!     assert_eq!(task.await?, 42);
//!     Ok(())
//! }
//! # #[cfg(not(feature = "macros"))] fn main() {}
//! ```
//!
//! Complete required joins before shutdown, which cancels pending tasks.
//! Dropping a join handle does not cancel its task. See the crate's
//! [guides](crate#documentation) for local tasks, blocking work, and relocation.

pub(crate) mod builtins;
pub(crate) mod execution;
pub(crate) mod join;
pub(crate) mod local;
mod runtime_scheduler;
pub(crate) mod scheduler;

#[doc(inline)]
pub use builtins::Builtins;
#[doc(inline)]
pub use join::{JoinError, JoinHandle, LocalJoinHandle};
#[doc(inline)]
pub use local::LocalTaskScheduler;
#[doc(inline)]
pub use runtime_scheduler::RuntimeScheduler;
#[doc(inline)]
pub use scheduler::TaskScheduler;
