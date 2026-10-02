// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime ownership, completion, shutdown, and failure.
//!
//! [`Runtime`](crate::runtime::Runtime) starts its workers during construction
//! and owns their lifetime. Worker-bound schedulers and `Builtins` are capabilities,
//! not additional runtime owners.
//!
//! Prefer [`arty::main`](crate::main) and [`arty::test`](crate::test) for ordinary
//! entry points. This guide uses explicit runtime construction to show ownership,
//! borrowing, and shutdown that the attributes would otherwise manage.
//!
//! # Choosing an entry point
//!
//! | Operation | Ownership and completion |
//! | --- | --- |
//! | [`RuntimeScheduler::block_on`](crate::task::RuntimeScheduler::block_on) | Borrows the scheduler and returns `Result<T, runtime::Error>` from a task that may borrow the caller's stack |
//! | [`Runtime::stop`](crate::runtime::Runtime::stop) | Consumes the owner and requests shutdown; `Ok(())` means it completed |
//! | [`RuntimeOperations::request_stop`](crate::runtime::RuntimeOperations::request_stop) | Requests shutdown without blocking; repeated calls are allowed |
//! | Dropping `Runtime` | Requests shutdown and normally waits for it, with the blocking-task exception below |
//!
//! `block_on` executes its callback on an Arty worker, not on the calling thread.
//! It destroys its borrowing factory and future before returning success or failure.
//! Its result must still be
//! `Send + 'static`; return owned data or mutate the borrowed caller-owned data.
//!
//! ```
//! use arty::runtime::Runtime;
//!
//! let runtime = Runtime::new()?;
//! let mut message = String::from("Hello");
//! runtime
//!     .scheduler()
//!     .block_on(async |_| message.push_str(", Arty"))?;
//! assert_eq!(message, "Hello, Arty");
//! runtime.stop()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Cancellation and shutdown
//!
//! Await the joins whose results you need before requesting shutdown or letting
//! a macro entry point's body return. Shutdown cancels asynchronous tasks and queued
//! blocking callbacks; it does not wait for them to complete successfully.
//!
//! Already-running blocking callbacks finish before shutdown completes. They
//! cannot be interrupted, so a callback that never returns can prevent shutdown.
//!
//! Cancellation and rejection return [`JoinError`](crate::task::JoinError) with
//! `is_shutdown() == true`. Once shutdown starts, new submissions return an
//! immediately ready error without invoking their factories. Pending asynchronous
//! work is cancelled on its worker; queued blocking work is discarded before its
//! callback starts. A running callback may still return a successful result.
//!
//! A successful `stop` waits for worker shutdown and running blocking work,
//! rather than replacing task joins. Calling it from a context that cannot
//! wait still requests shutdown, but returns an error without waiting.
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
//!     .spawn_anywhere(async |_| 42)
//!     .wait()
//!     .expect_err("submission follows shutdown");
//! assert!(error.is_shutdown());
//! runtime.stop()?;
//! # Ok::<(), arty::runtime::Error>(())
//! ```
//!
//! Dropping a join does not request cancellation or rethrow its task's panic
//! elsewhere. The task may continue while the runtime is running, but its
//! result is no longer observed.
//!
//! # Blocking restrictions
//!
//! `RuntimeScheduler::block_on` returns an error on asynchronous Arty workers,
//! including workers of another runtime. `JoinHandle::wait` rejects that context
//! by panicking. `Runtime::stop` requests shutdown but returns an error instead of
//! waiting from a worker. Dropping the owner on any asynchronous Arty worker
//! requests shutdown without waiting or panicking.
//!
//! A blocking task may wait for asynchronous work. Stopping its own runtime returns
//! an error after requesting shutdown, because it cannot wait for itself.
//! Dropping its owner requests shutdown without waiting for that callback.
//! Neither this case nor destruction on an asynchronous worker guarantees that
//! shutdown has completed when `drop` returns.
//!
//! Explicit `stop` reports worker panics after joining all workers. Implicit
//! destruction retains the worker-entry diagnostics but cannot return an error.
//! Use explicit shutdown when the caller needs to observe its outcome.
//!
//! # Task failures
//!
//! Joins return `Result<T, JoinError>`. When unwinding is enabled, factory, future,
//! and blocking-callback panics become an error with `is_panic() == true`; joining
//! does not resume that panic. The failure is distinct from an application error
//! returned by the task: a task returning `Result<T, E>` produces
//! `Result<Result<T, E>, JoinError>` when joined.
//!
//! `RuntimeScheduler::block_on` wraps a task failure in `runtime::Error` with
//! the `JoinError` retained as its source. Entry-point macros stop their runtime
//! before returning the root's value or resuming its original panic payload.
//! Construction errors, root cancellation, and shutdown errors become panics.
//! A root failure takes precedence if shutdown also fails.
//!
//! An application error returned by the body remains its ordinary return value.
//! Handle task joins in the body, or use explicit `build`, `block_on`, and `stop`
//! when returned runtime errors should be handled instead of converted to panics.
//!
//! # Panics
//!
//! Receiving a panic error does not repair application state or poisoned locks.
//! Runtime capabilities are not universally `UnwindSafe` or `RefUnwindSafe`.
//! With `panic = "abort"`, a panic aborts the process instead of being transported.
//!
//! # Construction errors
//!
//! [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build) returns
//! [`runtime::Error`](crate::runtime::Error) for a zero processor count or a
//! selection that cannot be satisfied. Use its display text and error source
//! for diagnostics, not as a stable recovery classification.
//!
//! Not every startup failure is a returned error. Worker initialization and
//! OS thread creation can panic. A worker limit does not eliminate these failures.
