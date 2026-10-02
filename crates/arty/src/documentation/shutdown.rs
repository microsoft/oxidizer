// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime shutdown and task cancellation.
//!
//! [`Runtime`](crate::runtime::Runtime) owns the workers. Scheduler and
//! [`Builtins`](crate::task::Builtins) handles do not keep the runtime running.
//! The [`main`](crate::main) and [`test`](crate::test) attributes stop their
//! runtime when the annotated body returns; await child tasks that must finish
//! before then.
//!
//! # Requesting and waiting for shutdown
//!
//! [`RuntimeOperations::request_stop`](crate::runtime::RuntimeOperations::request_stop)
//! requests shutdown without waiting and can be called repeatedly. Keep the
//! runtime owner until you are ready to wait for its workers.
//!
//! [`Runtime::stop`](crate::runtime::Runtime::stop) consumes the owner, requests
//! shutdown, and waits for workers to stop. Dropping the owner also requests
//! shutdown and normally waits, but cannot report shutdown errors. Prefer
//! explicit `stop` when the caller needs to observe the result.
//!
//! # Pending and running work
//!
//! Once shutdown begins, new submissions return an immediately ready
//! [`JoinError`](crate::task::JoinError) with `is_shutdown() == true` without
//! invoking their factories. Pending async tasks are cancelled on their worker,
//! and queued blocking callbacks do not start.
//!
//! Already-running blocking callbacks finish before shutdown completes; one
//! that never returns can prevent it. Await work you need before stopping the
//! runtime. Dropping a join handle does not cancel its task.
//!
//! A rejected submission can be handled immediately:
//!
//! ```
//! use arty::runtime::{Runtime, RuntimeOperations};
//!
//! let runtime = Runtime::new()?;
//! let scheduler = runtime.scheduler();
//! RuntimeOperations::from(&runtime).request_stop();
//! let error = scheduler
//!     .spawn_anywhere((), |_, ()| async { 42 })
//!     .wait()
//!     .expect_err("submission follows shutdown");
//! assert!(error.is_shutdown());
//! runtime.stop()?;
//! # Ok::<(), arty::runtime::Error>(())
//! ```
//!
//! # Calling from a worker
//!
//! `Runtime::stop` requests shutdown but returns an error rather than waiting
//! when called from an async Arty worker or one of the runtime's blocking
//! callbacks. Dropping the owner on any async Arty worker also requests shutdown
//! without waiting or panicking.
//!
//! Dropping the owner from its own blocking callback requests shutdown without
//! waiting for that callback. Neither case guarantees that shutdown has completed
//! when destruction returns.
//!
//! Explicit `stop` waits for all workers before reporting a worker failure.
//! Dropping the owner cannot return that error.
