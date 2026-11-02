// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Keep work on its worker, or move worker-aware values explicitly.
//!
//! A task stays on one worker while it runs. Its
//! [`Context`](crate::task::Context) provides a scheduler, clock, I/O handle,
//! associated with that worker.
//!
//! # Keep work on the same worker
//!
//! Cloning a scheduler keeps its worker association. This example uses the
//! clone from a blocking thread to submit work back to the original worker:
//!
//! ```
//! use arty::task::Context;
//!
//! # #[arty::main]
//! # async fn main(cx: Context) -> Result<(), arty::task::JoinError> {
//! let home = cx.thread().id();
//! let executed_on = cx
//!     .scheduler()
//!     .spawn_blocking({
//!         let scheduler = cx.scheduler().clone();
//!         move || scheduler.spawn(async |child| child.thread().id())
//!     })
//!     .await?
//!     .await?;
//! assert_eq!(executed_on, home);
//! # Ok(())
//! # }
//! ```
//!
//! # Let the runtime place new work
//!
//! [`Scheduler::spawn_anywhere`](crate::task::Scheduler::spawn_anywhere)
//! lets the runtime choose a worker for a new task. Pass a
//! [`ThreadAware`](crate::core::ThreadAware) value explicitly; Arty relocates it
//! to the chosen worker before the task uses it:
//!
//! ```
//! use arty::task::{Context, JoinError};
//!
//! # #[arty::main]
//! # async fn main(cx: Context) -> Result<(), JoinError> {
//! let home = cx.thread().clone();
//! let (worker, scheduler) = cx
//!     .scheduler()
//!     .spawn_anywhere(cx.clone(), |moved| async move {
//!         assert_eq!(moved.thread().id(), std::thread::current().id());
//!         (moved.thread().clone(), moved.scheduler().clone())
//!     })
//!     .await?;
//! let child = scheduler.spawn(async |child| child.thread().id()).await?;
//! assert_eq!(child, home.id());
//! // The coordinate records where the task ran; the scheduler rebinds on return.
//! let _executed_on = worker;
//! # Ok(())
//! # }
//! ```
//!
//! The runtime may choose the original worker. If a task only needs its own
//! services, the runtime-wide scheduler passes `Context` to its factory instead
//! of requiring an existing value to be moved.
//!
//! # What stays put
//!
//! `spawn_anywhere` starts a new task; it never moves a running one. Simply
//! moving or cloning `Context` does not change its worker association.
//! Scheduler handles and worker coordinates do not keep the runtime alive.
//!
//! Ordinary `spawn` and `block_on` do not relocate captures or returned values.
//! Worker-bound `spawn_anywhere` and `spawn_at` relocate their explicit payloads
//! and, when joined on an Arty worker, relocate their results to the joining
//! worker. This is normally the source worker; moving the join to another worker
//! changes the destination. Joining outside Arty does not relocate the result.
//! Runtime-wide `spawn_anywhere` does not relocate its result.
//!
//! `ThreadAware` builds on `Send`: values must remain safe to move even without
//! relocation. See the [`ThreadAware` contract](crate::core::ThreadAware) when
//! implementing it for your own types.
