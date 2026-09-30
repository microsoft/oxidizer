// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime ownership, completion, shutdown, and failure.
//!
//! [`Runtime`](crate::runtime::Runtime) starts its workers during construction
//! and owns their lifetime. Schedulers and `Builtins` are capabilities, not
//! additional runtime owners.
//!
//! Prefer [`arty::main`](crate::main) and [`arty::test`](crate::test) for ordinary
//! entry points. This guide uses explicit runtime construction to show ownership,
//! borrowing, and shutdown that the attributes would otherwise manage.
//!
//! # Choosing an entry point
//!
//! | Operation | Ownership and completion |
//! | --- | --- |
//! | [`Runtime::run`](crate::runtime::Runtime::run) | Consumes the runtime; shuts down after the root finishes and returns its `Result<T, JoinError>` |
//! | [`Runtime::block_on`](crate::runtime::Runtime::block_on) | Borrows the runtime and returns `Result<T, JoinError>` from a task that may borrow the caller's stack |
//! | [`Runtime::stop`](crate::runtime::Runtime::stop) | Requests shutdown without blocking; repeated calls are allowed |
//! | [`Runtime::wait`](crate::runtime::Runtime::wait) | Waits for shutdown; does not request it |
//! | Dropping `Runtime` | Requests shutdown and normally waits for it, with the blocking-task exception below |
//!
//! Both `run` and `block_on` execute their callbacks on an Arty worker, not on
//! the calling thread. `block_on` destroys its borrowing factory and future
//! before returning success or failure. Its result must still be
//! `Send + 'static`; return owned data or mutate the borrowed caller-owned data.
//!
//! ```
//! use arty::runtime::Runtime;
//!
//! let runtime = Runtime::new()?;
//! let mut message = String::from("Hello");
//! runtime.block_on(async |_| message.push_str(", Arty"))?;
//! assert_eq!(message, "Hello, Arty");
//! runtime.stop();
//! runtime.wait();
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Cancellation and shutdown
//!
//! Await the joins whose results you need before requesting shutdown or letting
//! `run`'s root return. Shutdown cancels asynchronous tasks and queued blocking
//! callbacks; it does not wait for them to complete successfully.
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
//! `Runtime::wait()` observes worker shutdown and running blocking work, rather
//! than replacing task joins.
//!
//! A rejected submission can be handled immediately:
//!
//! ```
//! use arty::runtime::Runtime;
//!
//! let runtime = Runtime::new()?;
//! let scheduler = runtime.task_scheduler();
//! runtime.stop();
//! let error = scheduler
//!     .spawn(async |_| 42)
//!     .wait()
//!     .expect_err("submission follows shutdown");
//! assert!(error.is_shutdown());
//! runtime.wait();
//! # Ok::<(), arty::runtime::Error>(())
//! ```
//!
//! Dropping a join does not request cancellation or rethrow its task's panic
//! elsewhere. The task may continue while the runtime is running, but its
//! result is no longer observed.
//!
//! # Blocking restrictions
//!
//! Do not call `run`, `block_on`, `wait`, or `JoinHandle::wait` from an
//! asynchronous Arty worker. They reject that context. The same restriction
//! applies to dropping the runtime owner there; retain the owner on a
//! blocking-safe thread.
//!
//! A blocking task may wait for asynchronous work. It cannot explicitly wait
//! for its own runtime to shut down: shutdown needs that blocking task to
//! finish, so `Runtime::wait` rejects the call. Dropping its own runtime owner
//! instead requests shutdown without waiting for itself. This exception does
//! not make destruction an unconditional shutdown-completion barrier.
//!
//! # Task failures
//!
//! Joins return `Result<T, JoinError>`. When unwinding is enabled, factory, future,
//! and blocking-callback panics become an error with `is_panic() == true`; joining
//! does not resume that panic. The failure is distinct from an application error
//! returned by the task: a task returning `Result<T, E>` produces
//! `Result<Result<T, E>, JoinError>` when joined.
//!
//! `Runtime::run` and `block_on` return the same outer failure result. Entry-point
//! macros preserve the annotated function's declared return type: they return a
//! successful root's value, resume its original panic payload on panic, and panic
//! if shutdown cancels the root. Handle a join's `Result` in the task body when
//! failure should be recoverable instead.
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
