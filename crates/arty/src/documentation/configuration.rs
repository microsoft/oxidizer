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
//! ```
//! use std::num::NonZeroUsize;
//!
//! use arty::runtime::{BlockingPoolPolicy, Builtins, ProcessorCount, Runtime, RuntimeBuilder};
//!
//! fn app_builder() -> RuntimeBuilder {
//!     Runtime::builder()
//!         .processor_count(ProcessorCount::at_most(NonZeroUsize::new(2).unwrap()))
//!         .blocking_pool_policy(BlockingPoolPolicy::shared(4))
//! }
//!
//! #[arty::main(builder = app_builder())]
//! async fn main(cx: Builtins) {
//!     assert_eq!(cx.scheduler().spawn(async |_| 42).await.unwrap(), 42);
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
//! Telemetry uses a noop sink by default. Configure
//! [`RuntimeBuilder::sink`](crate::runtime::RuntimeBuilder::sink) to receive
//! runtime events and propagate enrichment; see [telemetry](super::telemetry).
//!
//! # Testing environment
//!
//! Enable `test-util` in dev-dependencies for testing utilities. Under Miri,
//! Arty uses simulated six-processor data instead of native processor discovery
//! and pinning. This lets tests use the scheduling model without claiming to
//! exercise OS affinity or real hardware discovery. Native tests remain
//! necessary for those platform operations.
//!
//! # Asynchronous I/O
//!
//! Arty currently has no asynchronous I/O driver, driver-injection API, or
//! built-in memory pool. Workers can process wakeups, timers, and shutdown
//! without an I/O driver. `spawn_blocking` can run synchronous I/O on a pool,
//! but that does not add asynchronous I/O integration.
//!
//! [`arty_io_core`] describes separate I/O contracts; its presence does not
//! supply drivers to this runtime. A library that requires Tokio's I/O or timer
//! drivers does not become compatible merely by submitting its future to Arty.
//! Task metadata, advanced spawn builders, fanout, explicit worker-placement
//! APIs, and a runtime yield operation are not provided either.
//!
//! # Performance evidence
//!
//! Worker affinity trades automatic load balancing for stable placement.
//! Measure the application's actual workload before choosing its worker and
//! blocking-pool configuration.
//!
//! The [historical benchmark report](https://github.com/microsoft/oxidizer/blob/d4dd28c31dd2bd45238dff4c92c440561dc71581/crates/arty/docs/benchmarks.md)
//! found Tokio faster for ordinary, local, and nested scheduling on its Windows
//! host, and Arty faster for the measured blocking-pool cases. It measures an
//! older implementation, not the current runtime. Placement differences,
//! blocking-pool policies, host timer granularity, and batch-level timing limit
//! what those results can establish; they are not universal speed claims.
