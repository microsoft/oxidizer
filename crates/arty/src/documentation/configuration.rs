// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Choosing worker counts, blocking pools, and runtime services.
//!
//! Use [`Runtime::builder`](crate::runtime::Runtime::builder) when the defaults
//! do not fit your application. Set your options before calling
//! [`build`](crate::runtime::RuntimeBuilder::build), which starts the workers.
//!
//! # Async workers
//!
//! [`CpuPolicy`](crate::runtime::CpuPolicy) selects processors, with
//! one async worker per selected processor:
//!
//! | Policy | Meaning |
//! | --- | --- |
//! | `auto()` (default) | Let Arty choose; currently uses all available processors, but that policy may evolve |
//! | `all()` | Use all available processors explicitly |
//! | `at_most(n)` | Use no more than `n`, clamping to available processors |
//! | `exactly(n)` | Require `n`; return `runtime::Error` if fewer are available |
//!
//! The runtime rejects a count of zero when it is built.
//!
//! ```
//! use arty::runtime::{BlockingPoolPolicy, CpuPolicy, Runtime, RuntimeBuilder};
//! use arty::task::Builtins;
//!
//! fn app_builder() -> RuntimeBuilder {
//!     Runtime::builder()
//!         .cpu_policy(CpuPolicy::at_most(2))
//!         .blocking_pool_policy(BlockingPoolPolicy::shared(4))
//! }
//!
//! #[arty::main(builder = app_builder())]
//! async fn main(cx: Builtins) -> Result<(), arty::task::JoinError> {
//!     assert_eq!(cx.scheduler().spawn(async |_| 42).await?, 42);
//!     Ok(())
//! }
//! ```
//!
//! This requests at most two async workers and a shared pool of up to four
//! blocking threads. The pool limit does not limit the whole process.
//!
//! Async worker stacks default to 2 MiB. Use
//! [`stack_size`](crate::runtime::RuntimeBuilder::stack_size) to change them;
//! a larger `RUST_MIN_STACK` takes precedence. Blocking-pool stacks are separate.
//!
//! # Blocking pools
//!
//! Blocking work runs off the async workers. By default, they share one pool.
//! Use [`BlockingPoolPolicy::shared`](crate::runtime::BlockingPoolPolicy::shared)
//! to set a runtime-wide thread limit, or
//! [`isolated`](crate::runtime::BlockingPoolPolicy::isolated) to give each
//! worker its own pool. Isolated pools can use more threads in total.
//! Choose based on your workload rather than assuming one policy is faster.
//!
//! # Entry-point attributes
//!
//! With `macros` enabled, `#[arty::main]` and `#[arty::test]` create and stop
//! the runtime for you. By default, `main` chooses workers automatically and
//! `test` uses one worker.
//!
//! Use `workers = N` to cap the worker count or `builder = expression` for
//! custom settings; they cannot be combined. The builder expression runs
//! before workers start.
//!
//! See [`main`](crate::main) and [`test`](crate::test) for all options. Build
//! the runtime yourself if you need to handle construction errors or own
//! its shutdown.
//!
//! # Clocks and telemetry
//!
//! The default clock follows real time. Use
//! [`RuntimeBuilder::clock`](crate::runtime::RuntimeBuilder::clock) for a
//! custom clock; see [time](super::time) for controlled-time tests.
//!
//! The default telemetry sink discards events. Configure
//! [`RuntimeBuilder::sink`](crate::runtime::RuntimeBuilder::sink) to receive
//! events; see [telemetry](super::telemetry).
//!
//! # Async I/O
//!
//! Arty does not provide an async I/O driver. Use
//! [`spawn_blocking`](crate::task::Scheduler::spawn_blocking) for synchronous
//! I/O; libraries requiring another runtime's I/O driver still need that driver.
//!
//! # Worker placement
//!
//! A task stays on its worker once started. For
//! [`RuntimeScheduler::spawn_anywhere`](crate::task::RuntimeScheduler::spawn_anywhere)
//! and [`Scheduler::spawn_anywhere`](crate::task::Scheduler::spawn_anywhere),
//! the runtime chooses where new work starts; it does not move running tasks
//! between workers. Measure your workload before choosing worker and pool limits.
