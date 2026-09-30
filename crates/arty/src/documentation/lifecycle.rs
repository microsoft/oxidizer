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
//! | [`Runtime::run`](crate::runtime::Runtime::run) | Consumes the runtime, waits for a root task's result, then shuts down |
//! | [`Runtime::block_on`](crate::runtime::Runtime::block_on) | Borrows the runtime and runs a task that may borrow the caller's stack |
//! | [`Runtime::stop`](crate::runtime::Runtime::stop) | Requests shutdown without blocking; repeated calls are allowed |
//! | [`Runtime::wait`](crate::runtime::Runtime::wait) | Waits for shutdown; does not request it |
//! | Dropping `Runtime` | Requests shutdown and normally waits for it, with the blocking-task exception below |
//!
//! Both `run` and `block_on` execute their callbacks on an Arty worker, not on
//! the calling thread. `block_on` destroys its borrowing factory and future
//! before returning or propagating a task panic. Its result must still be
//! `Send + 'static`; return owned data or mutate the borrowed caller-owned data.
//!
//! ```
//! use arty::runtime::Runtime;
//!
//! let runtime = Runtime::new()?;
//! let mut message = String::from("Hello");
//! runtime.block_on(async |_| message.push_str(", Arty"));
//! assert_eq!(message, "Hello, Arty");
//! runtime.stop();
//! runtime.wait();
//! # Ok::<(), arty::runtime::Error>(())
//! ```
//!
//! # Cancellation and shutdown
//!
//! Shutdown is **not a drain of asynchronous tasks**. It cancels asynchronous
//! work, closes admission, and waits for already accepted blocking work. Await
//! the joins you require before requesting shutdown or allowing `run`'s root
//! task to return. A blocking callback that never returns can prevent shutdown
//! from completing.
//!
//! A task cancelled before producing a result leaves its join pending
//! indefinitely. Submission after admission closes also returns a pending join
//! rather than an error. This includes local submission from a still-valid token
//! during cancellation: its factory is not invoked.
//!
//! Consequently, `JoinHandle::wait()` can remain blocked forever, as can `run`
//! or `block_on` if their task is cancelled. `Runtime::wait()` observes worker
//! shutdown, not completion of outstanding join handles. Do not try to finish a
//! cancelled join after `wait()` returns, or rely on a stopped runtime's timers
//! to provide an independent shutdown deadline.
//!
//! This example polls a rejected join once instead of waiting for an outcome
//! that will never arrive:
//!
//! ```
//! use std::future::Future;
//! use std::pin::pin;
//! use std::task::{Context, Waker};
//!
//! use arty::runtime::Runtime;
//!
//! let runtime = Runtime::new()?;
//! let scheduler = runtime.task_scheduler();
//! runtime.stop();
//! runtime.wait();
//! let mut rejected = pin!(scheduler.spawn(async |_| 42));
//! let mut context = Context::from_waker(Waker::noop());
//! assert!(rejected.as_mut().poll(&mut context).is_pending());
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
//! # Panics
//!
//! When unwinding is enabled, a panic in remote factory invocation or remote/local
//! future polling is transported to the join. Awaiting the join or calling its
//! `wait()` resumes the original payload. Blocking-task panics are transported
//! to their joins too. A local factory runs immediately, so a panic while
//! constructing its future propagates directly to its caller.
//!
//! This is not an application-state recovery mechanism. Runtime capabilities
//! are not universally `UnwindSafe` or `RefUnwindSafe`, and catching a panic
//! does not repair state or poisoned locks. With `panic = "abort"`, a panic
//! aborts the process instead of being transported. There is no configurable
//! runtime panic-handler API.
//!
//! # Construction errors
//!
//! [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build) returns
//! [`runtime::Error`](crate::runtime::Error) when processor selection cannot be
//! satisfied. Use its display text and error source for diagnostics, not as a
//! stable recovery classification.
//!
//! Not every startup failure is a returned error. Worker initialization and
//! OS thread creation can panic; the blocking-pool implementation still uses
//! infallible construction. A worker limit does not eliminate these failures.
