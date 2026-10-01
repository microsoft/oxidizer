// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Choosing worker counts, blocking pools, and runtime services.
//!
//! Use [`Runtime::builder`](crate::runtime::Runtime::builder) when the defaults
//! do not fit the application. Pass the builder to an entry-point attribute or
//! call [`build`](crate::runtime::RuntimeBuilder::build) yourself. Construction
//! starts the workers, so configure the builder first.
//!
//! # Asynchronous workers
//!
//! [`ProcessorCount`](crate::runtime::ProcessorCount) selects processors, with
//! one asynchronous worker per selected processor:
//!
//! | Policy | Meaning |
//! | --- | --- |
//! | `auto()` (default) | Let Arty choose; currently uses all available processors, but that policy may evolve |
//! | `all()` | Use all available processors explicitly |
//! | `at_most(n)` | Use no more than `n`, clamping to available processors |
//! | `exactly(n)` | Require `n`; return `runtime::Error` if fewer are available |
//!
//! `n` is a `usize`. Both count-based policies reject zero when the runtime
//! is built; creating a policy or setting it on the builder does not validate it.
//!
//! ```
//! use arty::runtime::{BlockingPoolPolicy, ProcessorCount, Runtime, RuntimeBuilder};
//! use arty::task::Builtins;
//!
//! fn app_builder() -> RuntimeBuilder {
//!     Runtime::builder()
//!         .processor_count(ProcessorCount::at_most(2))
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
//! This requests at most two asynchronous workers and a separate shared blocking
//! pool capped at four threads. It does not request exactly two processors or
//! limit the whole process to four threads.
//!
//! Asynchronous worker stacks default to 2 MiB. [`stack_size`](crate::runtime::RuntimeBuilder::stack_size)
//! changes that size; a larger `RUST_MIN_STACK` takes precedence. This setting
//! does not configure blocking-pool stacks.
//!
//! # Blocking pools
//!
//! [`BlockingPoolPolicy`](crate::runtime::BlockingPoolPolicy) is independent of the
//! asynchronous worker count. The default `isolated()` policy gives each
//! asynchronous worker its own blocking pool, separating contention but allowing
//! thread and stack costs to grow with the number of workers.
//!
//! `shared(n)` uses one runtime-wide pool with a common thread limit. It bounds
//! that pool's threads across all asynchronous workers, but combines their
//! blocking work. Neither policy promises higher throughput for every workload.
//!
//! # Entry-point attributes
//!
//! With the `macros` feature, `#[arty::main]` and
//! `#[arty::test]` own construction and shutdown for an asynchronous
//! function taking owned `Builtins`. They use the automatic processor policy
//! by default, including in tests.
//!
//! `workers = N` is an upper bound, using the same `at_most` policy as the
//! explicit builder. Use `builder = expression` for computed settings or a
//! different processor policy. These options cannot be combined. The builder
//! expression runs once on the synchronous caller, before workers start.
//!
//! The [`main`](crate::main) and [`test`](crate::test) attribute reference describes
//! the complete syntax, including `runtime_path = ::renamed_arty::runtime` for
//! renamed dependencies. Prefer the explicit builder when construction errors
//! need to be returned rather than panicked, or when lifecycle ownership needs
//! to remain outside the entry point.
//!
//! # Clocks and telemetry
//!
//! The default clock follows real time. [`RuntimeBuilder::clock`](crate::runtime::RuntimeBuilder::clock)
//! accepts an inactive clock or a test `ClockControl`; the runtime activates
//! worker clocks and advances their timers. The [time guide](super::time)
//! explains controlled time without changing task scheduling semantics.
//!
//! Telemetry uses a no-op sink by default. Configure
//! [`RuntimeBuilder::sink`](crate::runtime::RuntimeBuilder::sink) to receive
//! runtime events and propagate enrichment; see [telemetry](super::telemetry).
//!
//! # Asynchronous I/O
//!
//! Arty drives task wakeups and timers, but does not provide an asynchronous
//! I/O driver. Use [`spawn_blocking`](crate::task::TaskScheduler::spawn_blocking)
//! for synchronous I/O, or a library that supplies its own compatible driver.
//! Submitting a future does not supply another runtime's services: libraries
//! requiring Tokio's I/O or timer drivers still need those drivers.
//!
//! # Worker placement
//!
//! A future stays on its worker once started. Stable placement supports local
//! state and processor locality, but does not automatically balance running
//! tasks across workers. Distribute independent tasks with
//! [`Runtime::scheduler`](crate::runtime::Runtime::scheduler) or
//! [`spawn_anywhere`](crate::task::TaskScheduler::spawn_anywhere), and measure the
//! application's workload before choosing worker and pool limits.
