// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Keep work on its worker, or move worker-aware values explicitly.
//!
//! A task stays on one worker while it runs. Its
//! [`Builtins`](crate::task::Builtins) provides a scheduler, clock, and telemetry
//! sink associated with that worker.
//!
//! # Keep work on the same worker
//!
//! Cloning a scheduler keeps its worker association. This example uses the
//! clone from a blocking thread to submit work back to the original worker:
//!
//! ```
//! use arty::task::Builtins;
//!
//! #[arty::main]
//! async fn main(cx: Builtins) -> Result<(), arty::task::JoinError> {
//!     let home = cx.thread().id();
//!     let cloned = cx.scheduler().clone();
//!     let executed_on = cx
//!         .scheduler()
//!         .spawn_blocking(move || cloned.spawn(async |child| child.thread().id()).wait())
//!         .await??;
//!     assert_eq!(executed_on, home);
//!     Ok(())
//! }
//! ```
//!
//! # Let the runtime place new work
//!
//! [`TaskScheduler::spawn_anywhere`](crate::task::TaskScheduler::spawn_anywhere)
//! lets the runtime choose a worker for a new task. Pass a
//! [`ThreadAware`](crate::core::ThreadAware) value explicitly; Arty relocates it
//! to the chosen worker before the task uses it:
//!
//! ```
//! use arty::task::{Builtins, JoinError};
//!
//! #[arty::main]
//! async fn main(cx: Builtins) -> Result<(), JoinError> {
//!     let (worker, scheduler) = cx.scheduler()
//!         .spawn_anywhere(cx.clone(), |moved| async move {
//!             assert_eq!(moved.thread().id(), std::thread::current().id());
//!             assert!(moved.local_scheduler().is_some());
//!             (moved.thread().clone(), moved.scheduler().clone())
//!         })
//!         .await?;
//!     let child = scheduler.spawn(async |child| child.thread().id()).await?;
//!     assert_eq!(child, worker.id());
//!     Ok(())
//! }
//! ```
//!
//! The runtime may choose the original worker. If a task only needs its own
//! services, use the `Builtins` passed to its factory instead of moving an
//! existing value.
//!
//! # What stays put
//!
//! `spawn_anywhere` starts a new task; it never moves a running one. Simply
//! moving or cloning `Builtins` does not change its worker association, and
//! [`local_scheduler()`](crate::task::Builtins::local_scheduler) is only
//! available on that worker. Scheduler handles and worker coordinates do not
//! keep the runtime alive.
//!
//! Ordinary `spawn` and `block_on` do not relocate captures or returned values.
//! `spawn_anywhere` relocates its explicit payload, but not its result.
//!
//! `ThreadAware` builds on `Send`: values must remain safe to move even without
//! relocation. See the [`ThreadAware` contract](crate::core::ThreadAware) when
//! implementing it for your own types.
