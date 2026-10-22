// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Choosing worker counts, blocking pools, and runtime services.
//!
//! Use [`Runtime::builder`](crate::runtime::Runtime::builder) when the defaults
//! do not fit your application. Set your options before calling
//! [`build`](crate::runtime::RuntimeBuilder::build), which starts the workers.
//! The monitoring policy is enabled by default. Use
//! [`RuntimeBuilder::monitor`](crate::runtime::RuntimeBuilder::monitor) with
//! [`MonitorPolicy::disabled`](crate::runtime::MonitorPolicy::disabled) to
//! disable it for one runtime.
//!
//! The `monitor` Cargo feature enables the Seismograph dependencies for
//! `performables` and runtime monitoring. This stage only provides configuration
//! plumbing. It does not start a monitor, instrument tasks, or emit new telemetry.
//!
//! # Async workers
//!
//! [`WorkersPolicy`](crate::runtime::WorkersPolicy) selects the worker count
//! and assigns each worker to an available processor:
//!
//! | Policy | Meaning |
//! | --- | --- |
//! | `default()` | Use one worker |
//! | `all()` | Use all available processors explicitly |
//! | `at_most(n)` | Use no more than `n`, clamping to available processors |
//! | `exactly(n)` | Start `n` workers; share processors when `n` exceeds the available count |
//!
//! The runtime rejects a count of zero when it is built.
//! Each worker is an operating-system thread with its own stack. Large exact
//! counts can exceed operating-system thread or memory limits. For example,
//! `exactly(1000)` requests 1000 worker threads even on a smaller machine.
//!
//! ```
//! use arty::runtime::{BlockingPoolPolicy, Runtime, WorkersPolicy};
//!
//! fn main() -> Result<(), ohno::AppError> {
//!     let runtime = Runtime::builder()
//!         .workers(WorkersPolicy::at_most(2))
//!         .blocking_pool(BlockingPoolPolicy::shared().max(4))
//!         .build()?;
//!     assert_eq!(runtime.scheduler().block_on(async |_| 42)?, 42);
//!     runtime.stop()?;
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
//! for one runtime-wide pool, or
//! [`per_worker`](crate::runtime::BlockingPoolPolicy::per_worker) to give each
//! worker its own pool. Chain [`max`](crate::runtime::BlockingPoolPolicy::max)
//! to set the thread limit. Per-worker pools can use more threads in total.
//! Choose based on your workload rather than assuming one policy is faster.
//!
//! Each blocking callback occupies one pool thread until it returns. Do not
//! create a cycle where a callback waits for async work that then awaits
//! another callback from the same saturated pool. Shared pools expose every
//! worker to that cycle; per-worker pools limit it to one worker's pool. Arty
//! rejects direct polling of a same-pool blocking handle, but it does not track
//! transitive dependencies through spawned async tasks.
//!
//! # Entry-point attributes
//!
//! With `macros` enabled, `#[arty::main]` and `#[arty::test]` create and stop
//! the runtime for you. Both use one worker by default. `#[arty::main]` uses
//! the enabled monitoring policy. `#[arty::test]` uses the disabled monitoring
//! policy so future monitor activation stays out of ordinary tests. A test that
//! exercises monitoring must use `monitor = enabled`.
//!
//! Use `workers = N` to set the exact worker count, or `workers = all` to use
//! all available processors. Use `monitor = enabled` or `monitor = disabled`
//! to select the monitoring policy. Build the runtime explicitly for custom
//! clocks, telemetry, blocking pools, stack sizes, or an
//! [`at_most`](crate::runtime::WorkersPolicy::at_most) worker policy.
//!
//! See [`main`](crate::main) and [`test`](crate::test) for the supported
//! signature and configuration. Build the runtime yourself if you need to
//! handle construction errors or own its shutdown.
//!
//! # Clocks and telemetry
//!
//! The default clock follows real time. Use
//! [`RuntimeBuilder::clock`](crate::runtime::RuntimeBuilder::clock) for a
//! custom clock; see [time](super::time) for controlled-time tests.
//!
//! The default telemetry sink disables events and task enrichment. Configure
//! [`RuntimeBuilder::sink`](crate::runtime::RuntimeBuilder::sink) to enable
//! them; see [telemetry](super::telemetry).
//!
//! # Async I/O
//!
//! Arty hosts drivers supplied by I/O libraries built on `arty_io_core`, but it
//! does not include a built-in network or file I/O implementation. Access
//! compatible contexts through
//! [`Builtins::io`](crate::task::Builtins::io); see the
//! [I/O guide](https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/IO.md).
//! Use [`spawn_blocking`](crate::task::Scheduler::spawn_blocking) for synchronous
//! I/O. Libraries requiring another runtime's I/O driver still need that runtime.
//!
//! # Worker placement
//!
//! A task stays on its worker once started. For
//! [`RuntimeScheduler::spawn_anywhere`](crate::task::RuntimeScheduler::spawn_anywhere)
//! and [`Scheduler::spawn_anywhere`](crate::task::Scheduler::spawn_anywhere),
//! the runtime chooses where new work starts; it does not move running tasks
//! between workers. Measure your workload before choosing worker and pool limits.
