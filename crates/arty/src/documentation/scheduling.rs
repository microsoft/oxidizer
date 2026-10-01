// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Scheduling tasks and receiving their results.
//!
//! A [`Runtime`](crate::runtime::Runtime) owns the workers. A
//! [`RuntimeScheduler`](crate::task::RuntimeScheduler) is borrowed from that owner.
//! [`TaskScheduler`](crate::task::TaskScheduler) keeps child work on an associated worker.
//! Keep the runtime alive until the tasks whose results you need have
//! completed.
//!
//! # Choosing a scheduler
//!
//! | Operation | Where the work runs | What crosses the boundary |
//! | --- | --- | --- |
//! | [`RuntimeScheduler::spawn_anywhere`](crate::task::RuntimeScheduler::spawn_anywhere) | Workers selected round-robin | A `Send` factory and a `Send` result |
//! | `spawn` on `Builtins::scheduler()` | That capability's associated worker | A `Send` factory and a `Send` result, even when called on the same worker |
//! | [`LocalTaskScheduler::spawn`](crate::task::LocalTaskScheduler::spawn) | The calling worker | Nothing crosses threads; captures and results may be non-`Send` |
//! | [`spawn_anywhere`](crate::task::TaskScheduler::spawn_anywhere) | Workers selected round-robin | An explicit `ThreadAware` payload, then a `Send` result |
//! | [`spawn_everywhere`](crate::task::TaskScheduler::spawn_everywhere) | Each worker once | A cloneable `ThreadAware` payload, then one `Send` result per worker |
//! | [`spawn_blocking`](crate::task::TaskScheduler::spawn_blocking) | A blocking pool | A `Send` synchronous closure and a `Send` result |
//!
//! Runtime-scheduler submissions share a round-robin selection sequence. That
//! scheduler cannot be cloned or detached from the owner. Selection order is not
//! execution or completion order. A worker-bound scheduler stays bound when cloned or sent to
//! another OS thread. Use `spawn_anywhere` when new work should be distributed
//! rather than remain with the bound worker.
//!
//! # Send the factory, create the future on the worker
//!
//! `spawn` takes a callback that receives owned
//! [`Builtins`](crate::task::Builtins) and returns a future. The callback and
//! its captures must be `Send + 'static`, but the future need not be `Send`:
//! Arty constructs, polls, and destroys it on its destination worker.
//!
//! Here the `Rc` is created inside the factory, not captured from the caller.
//! It stays on the worker across an asynchronous wait.
//!
//! ```
//! use std::rc::Rc;
//! use std::time::Duration;
//!
//! use arty::task::Builtins;
//!
//! #[arty::main]
//! async fn main(cx: Builtins) -> Result<(), arty::task::JoinError> {
//!     let answer = cx
//!         .scheduler()
//!         .spawn(|child| {
//!             let value = Rc::new(42);
//!             async move {
//!                 child.clock().delay(Duration::from_millis(1)).await;
//!                 *value
//!             }
//!         })
//!         .await?;
//!     assert_eq!(answer, 42);
//!     Ok(())
//! }
//! ```
//!
//! An async closure such as `async |cx| { ... }` is another way to write this
//! factory. An `async { ... }` block by itself is already a future and is not a
//! factory. Ordinary captures and results are not automatically relocated.
//! Results need `Send`, not [`ThreadAware`](crate::core::ThreadAware).
//!
//! # Share non-Send state between local tasks
//!
//! Obtain a [`LocalTaskScheduler`](crate::task::LocalTaskScheduler) from
//! [`Builtins::local_scheduler`](crate::task::Builtins::local_scheduler)
//! while executing on the associated worker. Its factory takes no arguments
//! and is invoked immediately. Captures, futures, and results may all be
//! non-`Send`, but must still be `'static`; local spawning is not scoped borrowing.
//!
//! ```
//! use std::rc::Rc;
//!
//! use arty::task::Builtins;
//!
//! #[arty::main]
//! async fn main(cx: Builtins) -> Result<(), arty::task::JoinError> {
//!     let value = Rc::new(String::from("worker-local"));
//!     let captured = Rc::clone(&value);
//!     let local = cx
//!         .local_scheduler()
//!         .expect("the task runs on its associated worker");
//!     let returned = local.spawn(async move || captured).await?;
//!     assert!(Rc::ptr_eq(&value, &returned));
//!     Ok(())
//! }
//! ```
//!
//! Await local joins on their worker. Neither a local scheduler nor its join
//! handle can be sent to another thread. For a task that borrows the synchronous
//! caller's stack, use [`RuntimeScheduler::block_on`](crate::task::RuntimeScheduler::block_on)
//! instead.
//!
//! # Keep blocking work off asynchronous workers
//!
//! A long synchronous call prevents an asynchronous worker from polling its
//! other tasks and advancing timers. Use `spawn_blocking` for synchronous
//! operating-system or library calls. For example, this program reads a file
//! named `settings.toml` from its working directory:
//!
//! ```no_run
//! use arty::task::Builtins;
//!
//! #[arty::main]
//! async fn main(cx: Builtins) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//!     let contents = cx
//!         .scheduler()
//!         .spawn_blocking(|| std::fs::read_to_string("settings.toml"))
//!         .await??;
//!     println!("{contents}");
//!     Ok(())
//! }
//! ```
//!
//! The first `?` handles task failure (`JoinError`); the second handles the file
//! operation's `io::Error`. This runs blocking file I/O on another thread; it
//! does not add an asynchronous I/O driver. The pools are intended for blocking
//! calls, not general CPU-parallel workloads. Their
//! [configuration](super::configuration#blocking-pools) is separate from the
//! number of asynchronous workers.
//!
//! # Observe completion before shutdown
//!
//! A [`JoinHandle`](crate::task::JoinHandle) yields `Result<T, JoinError>`.
//! Use [`JoinError::is_panic`](crate::task::JoinError::is_panic) to identify a task
//! panic and [`is_shutdown`](crate::task::JoinError::is_shutdown) to identify
//! cancellation or rejection. Use `.await` inside asynchronous tasks and
//! `wait()` only from a blocking-safe thread. Dropping the handle does not cancel
//! the task.
//!
//! Shutdown cancels pending tasks and rejects new submissions without invoking
//! their factories. See [lifecycle](super::lifecycle) for the difference between
//! receiving task results and stopping workers.
