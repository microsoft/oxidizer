<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Arty Logo" width="96">

# Arty

[![crate.io](https://img.shields.io/crates/v/arty.svg)](https://crates.io/crates/arty)
[![docs.rs](https://docs.rs/arty/badge.svg)](https://docs.rs/arty)
[![MSRV](https://img.shields.io/crates/msrv/arty)](https://crates.io/crates/arty)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Single-threaded, thread-aware application runtime.

An Arty runtime can have several workers, but each async task stays on one
worker throughout its life. This lets it use thread-local and non-[`Send`][__link0]
state across awaits. Thread-aware values can be explicitly relocated when
starting new work on another worker.

There is no process-global runtime: each instance owns its workers and
services. Workers use thread-local bookkeeping rather than a global
runtime singleton.

Arty provides task scheduling, clocks, telemetry, and pools for offloading
blocking callbacks. It does not provide async I/O drivers or move running
tasks between workers.

## Quickstart

Add Arty with its default runtime and macro features:

```sh
cargo add arty
```

```rust
use arty::task::{Builtins, JoinError};

#[arty::main]
async fn main(cx: Builtins) -> Result<(), JoinError> {
    let answer = cx.scheduler().spawn(async |_| 6 * 7).await?;
    println!("{answer}");
    Ok(())
}
```

This prints `42`. [`main`][__link1] starts and stops the runtime.
The `?` returns a [`JoinError`][__link2] if the child fails.
Await child work before returning; shutdown cancels pending async tasks.

## Overview

* [`main`][__link3] and [`test`][__link4] manage a runtime for an
  async entry point or test; [`Runtime`][__link5] gives you
  direct control over its lifetime and settings.
* [`Builtins`][__link6] gives each task its scheduler, clock,
  and worker. [`RuntimeScheduler`][__link7] lets the
  runtime place new work.
* [`Scheduler`][__link8] keeps child work on its worker.
* [`Clock`][__link9] provides timers and timeouts;
  [`ClockControl`][__link10] controls time in tests.
* [`Thread`][__link11] describes a worker, and
  [`ThreadAware`][__link12] values can relocate between workers.

## Why Arty?

Unlike Tokio’s multi-thread runtime, Arty keeps each task on one worker.
This supports thread-local and non-`Send` state, keeps worker-local data
nearby, and can reduce contention for that data. Shared data may still contend.
Choose Tokio for automatic task distribution or its async I/O ecosystem;
Arty does not provide Tokio’s I/O or timer drivers.

## Blocking work

Join handles are futures: await them. Arty deliberately does not expose a
synchronous join operation because async workers must stay free to poll
tasks and advance timers. Blocking a worker stalls every task assigned to
it. If the blocked code waits for work on that worker, or blocking callbacks
wait on each other in an exhausted pool, the stall can become a deadlock.

This deadlocks: the worker waits synchronously for a child that only the
same worker can poll.

```rust
#[arty::main]
async fn main(cx: arty::task::Builtins) {
    let child = cx.scheduler().spawn(async |_| 42);
    let _ = futures::executor::block_on(child);
}
```

[`RuntimeScheduler::block_on`][__link13] is
the narrow blocking entry point for running async work from synchronous
code. It rejects calls from async workers. For synchronous I/O or library
calls, use
[`Scheduler::spawn_blocking`][__link14] or
[`RuntimeScheduler::spawn_blocking`][__link15]
and await the join. The callback runs in a blocking pool instead of on an
async worker.

## Detailed documentation

The [guides][__link16] explain Arty’s capabilities in more detail:

* [Scheduling][__link17] explains where tasks run,
  local non-`Send` work, and blocking pools.
* [Configuration][__link18] explains worker counts,
  blocking-pool policies, clocks, and telemetry sinks.
* [Shutdown][__link19] explains runtime ownership,
  task cancellation, and what stopping the workers waits for.
* [Thread awareness][__link20] explains
  stable task placement and explicit relocation of values.
* [Time][__link21] explains worker-driven timers, timeouts,
  and controlled time in tests.
* [Telemetry][__link22] explains runtime events
  and links to [`observed`][__link23] for further details.

The `documentation` module is included only for docs.rs builds and
doc-test collection with all documentation features, including `test-util`;
it is not part of the public API available to applications.

## Features

The default feature set enables `rt` and `macros`.

* **`rt`** - Enables the runtime and task APIs, and implies `time`.
* **`macros`** - Enables [`main`][__link24] and [`test`][__link25], and implies `rt`.
* **`time`** - Enables clocks, timers, and timeouts.
* **`test-util`** - Enables testing utilities, including [`ClockControl`][__link26] with `time`.
  Enable it in dev-dependencies, not production dependencies.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbGTvrvo-fV-8bX71dk0Y2kEQby4ruQUuQ1Kgb3ChN6fCifzRhZIOCZGFydHllMC40LjCCa2FydHlfbWFjcm9zZTAuNC4wgmhvYnNlcnZlZGYwLjI3LjA
 [__link0]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link1]: https://docs.rs/arty_macros/0.4.0/arty_macros/?search=main
 [__link10]: https://docs.rs/arty/0.4.0/arty/?search=time::ClockControl
 [__link11]: https://docs.rs/arty/0.4.0/arty/?search=core::Thread
 [__link12]: https://docs.rs/arty/0.4.0/arty/?search=core::ThreadAware
 [__link13]: https://docs.rs/arty/0.4.0/arty/?search=task::RuntimeScheduler::block_on
 [__link14]: https://docs.rs/arty/0.4.0/arty/?search=task::Scheduler::spawn_blocking
 [__link15]: https://docs.rs/arty/0.4.0/arty/?search=task::RuntimeScheduler::spawn_blocking
 [__link16]: https://docs.rs/arty/0.4.0/arty/?search=documentation
 [__link17]: https://docs.rs/arty/0.4.0/arty/?search=documentation::scheduling
 [__link18]: https://docs.rs/arty/0.4.0/arty/?search=documentation::configuration
 [__link19]: https://docs.rs/arty/0.4.0/arty/?search=documentation::shutdown
 [__link2]: https://docs.rs/arty/0.4.0/arty/?search=task::JoinError
 [__link20]: https://docs.rs/arty/0.4.0/arty/?search=documentation::thread_awareness
 [__link21]: https://docs.rs/arty/0.4.0/arty/?search=documentation::time
 [__link22]: https://docs.rs/arty/0.4.0/arty/?search=documentation::telemetry
 [__link23]: https://crates.io/crates/observed/0.27.0
 [__link24]: https://docs.rs/arty_macros/0.4.0/arty_macros/?search=main
 [__link25]: https://docs.rs/arty_macros/0.4.0/arty_macros/?search=test
 [__link26]: https://docs.rs/arty/0.4.0/arty/?search=time::ClockControl
 [__link3]: https://docs.rs/arty_macros/0.4.0/arty_macros/?search=main
 [__link4]: https://docs.rs/arty_macros/0.4.0/arty_macros/?search=test
 [__link5]: https://docs.rs/arty/0.4.0/arty/?search=runtime::Runtime
 [__link6]: https://docs.rs/arty/0.4.0/arty/?search=task::Builtins
 [__link7]: https://docs.rs/arty/0.4.0/arty/?search=task::RuntimeScheduler
 [__link8]: https://docs.rs/arty/0.4.0/arty/?search=task::Scheduler
 [__link9]: https://docs.rs/arty/0.4.0/arty/?search=time::Clock
