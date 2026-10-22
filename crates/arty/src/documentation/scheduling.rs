// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Start tasks, choose where they run, and await their results.
//!
//! Keep the [`Runtime`](crate::runtime::Runtime) alive until the work you need
//! has finished. Scheduler handles do not keep it running.
//!
//! # Choose where work runs
//!
//! Borrow a [`RuntimeScheduler`](crate::task::RuntimeScheduler) from the runtime
//! to let it place a new task. A [`Scheduler`](crate::task::Scheduler)
//! from [`Builtins`](crate::task::Builtins) keeps child tasks on its worker,
//! even when cloned.
//!
//! Use [`spawn_anywhere`](crate::task::Scheduler::spawn_anywhere) to let the
//! runtime choose a worker for a new task and relocate a `ThreadAware` value;
//! [`spawn_everywhere`](crate::task::Scheduler::spawn_everywhere) starts one
//! task per worker. The runtime-wide `spawn_anywhere` also relocates its
//! `ThreadAware` input. Pass that input separately to a non-capturing function,
//! rather than capturing it in a closure. The order in which you submit tasks
//! does not determine when they finish.
//!
//! Results from worker-bound `Scheduler::spawn_anywhere` and
//! `spawn_everywhere` must also implement [`ThreadAware`](crate::core::ThreadAware),
//! which includes `Send`. Runtime-wide `RuntimeScheduler::spawn_anywhere` accepts
//! any `Send` result. Joining does not relocate results; relocate worker-aware
//! returned values explicitly when needed.
//!
//! # Create local state on a worker
//!
//! [`Scheduler::spawn`](crate::task::Scheduler::spawn) calls your
//! factory on the destination worker. This lets its future create and retain
//! non-`Send` state across awaits. The factory's captures and the task's result
//! must still be `Send` to cross threads. Here the `Rc` is created on the worker:
//!
//! ```
//! use std::rc::Rc;
//! use std::time::Duration;
//!
//! use arty::task::Builtins;
//!
//! # #[arty::main]
//! # async fn main(cx: Builtins) -> Result<(), arty::task::JoinError> {
//! let answer = cx
//!     .scheduler()
//!     .spawn(|child| {
//!         let value = Rc::new(42);
//!         async move {
//!             child.clock().delay(Duration::from_millis(1)).await;
//!             *value
//!         }
//!     })
//!     .await?;
//! assert_eq!(answer, 42);
//! # Ok(())
//! # }
//! ```
//!
//! Pass a factory, not an already-created future. Ordinary captures and
//! results are not relocated automatically.
//!
//! # Keep blocking work off async workers
//!
//! A long synchronous call stops an async worker from polling other tasks.
//! Use `spawn_blocking` for work such as reading a file. This example reads
//! `settings.toml` from the current directory:
//!
//! ```no_run
//! use arty::task::Builtins;
//!
//! # #[arty::main]
//! # async fn main(cx: Builtins) -> Result<(), ohno::AppError> {
//! let contents = cx
//!     .scheduler()
//!     .spawn_blocking(|| std::fs::read_to_string("settings.toml"))
//!     .await??;
//! println!("{contents}");
//! # Ok(())
//! # }
//! ```
//!
//! The first `?` handles task failure; the second handles the file error.
//! This runs I/O on a blocking thread, not an async I/O driver. Configure
//! [blocking pools](super::configuration#blocking-pools) separately from
//! async workers.
//!
//! A blocking callback owns one pool thread until it returns. Calling
//! [`RuntimeScheduler::block_on`](crate::task::RuntimeScheduler::block_on) from
//! that callback is allowed, but the async work must not await another callback
//! queued to the same saturated pool. That creates a circular dependency and
//! deadlocks. Directly polling a same-pool blocking handle is rejected, but
//! Arty does not infer transitive dependencies through async tasks.
//!
//! # Observe completion before shutdown
//!
//! Await a [`JoinHandle`](crate::task::JoinHandle) inside an async task. From
//! synchronous code, use
//! [`RuntimeScheduler::block_on`](crate::task::RuntimeScheduler::block_on) to
//! enter the runtime and await required work. A [`JoinError`](crate::task::JoinError)
//! reports a panic or shutdown; dropping the handle does not cancel the task.
//!
//! Shutdown cancels pending tasks and rejects new submissions without invoking
//! their factories. See [shutdown](super::shutdown) for what happens to pending
//! and running work when the runtime stops.
