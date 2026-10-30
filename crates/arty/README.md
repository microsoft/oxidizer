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

Cooperative single-threaded runtime for thread-aware applications.

Each task stays on one worker, which supports thread-local and non-[`Send`][__link0]
state across awaits. Shared handles can use explicit [`ThreadAware`][__link1]
relocation to expose one logical instance across workers while keeping
worker-local state separate.

## Quickstart

Add Arty with its default runtime and macro features:

```sh
cargo add arty
```

```rust
use arty::task::{Context, JoinError};

#[arty::main]
async fn main(cx: Context) -> Result<(), JoinError> {
    let answer = cx.scheduler().spawn(async |_| 6 * 7).await?;
    println!("{answer}");
    Ok(())
}
```

This prints `42`. [`main`][__link2] starts and stops the runtime.
The `?` returns a [`JoinError`][__link3] if the child fails.
Await child work before returning. Shutdown cancels pending async tasks.

## Execution model

Each Arty worker runs a cooperative single-threaded executor. A task returns
control when an awaited operation is pending, and it stays on one worker
throughout its life. Arty does not move running tasks between workers.

A task factory creates the task future on its destination worker. A payload
is a separate value passed to that factory.

[`ThreadAware`][__link4] gives a type a callback for explicit
relocation. For [`Scheduler::spawn_anywhere`][__link5],
Arty selects the destination. For
[`Scheduler::spawn_at`][__link6], Arty validates the
requested destination. If the task factory starts, Arty relocates the payload
before the factory uses it. Rejected work and work cancelled before the
factory starts can drop the payload without relocation.

Shared handles can use this callback to expose one logical instance across
workers while each clone uses worker-local state. `ThreadAware` does not
provide shared ownership or local storage by itself.

This example passes `cx.clone()` as the thread-aware payload. The closure is
the task factory. Arty relocates the payload before the factory uses it:

```rust
cx.scheduler()
    .spawn_anywhere(cx.clone(), |moved| async move {
        assert_eq!(moved.thread().id(), std::thread::current().id());
    })
    .await?;
```

The runtime can select the original worker. The invariant is that the
relocated context reports the worker that runs the new task.

There is no process-global runtime. Each instance owns its workers and
services. Workers use thread-local bookkeeping rather than a global runtime
singleton.

Arty provides task scheduling, clocks, blocking pools, and a host for
external I/O drivers. I/O libraries can integrate through `arty_io_core`.
Companion I/O crates ship alongside Arty.

## API overview

* [`runtime`][__link7] covers construction, configuration, ownership, and shutdown.
* [`task`][__link8] covers task placement, worker-local futures, blocking callbacks,
  and result handles.
* [`time`][__link9] covers clocks, delays, timeouts, and controlled test time.
* [`io`][__link10] covers external I/O contexts and worker-local drivers.
* [`core`][__link11] contains worker coordinates and the thread-awareness contract.
* [`main`][__link12] and [`test`][__link13] manage a runtime for an async entry point or test.

## Features

The default feature set enables `rt`, `macros`, and `monitor`.

* `rt`: Enables the runtime, task, and I/O APIs, and implies `time`.
* `macros`: Enables [`main`][__link14] and [`test`][__link15], and implies `rt`.
* `monitor`: Enables the Seismograph monitor dependency. It forwards
  `performables` telemetry support when `performables` is enabled. It does
  not start monitoring.
* `time`: Enables clocks, timers, and timeouts.
* `test-util`: Enables testing utilities, including
  [`ClockControl`][__link16] with `time`.
  Enable it in dev-dependencies, not production dependencies.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbC91TYBHbUbsb4l-Dg_TFUAcbgSFnB9MJnW8bj3mfqZrQEZlhZIKCZGFydHllMC41LjCCa2FydHlfbWFjcm9zZTAuNS4w
 [__link0]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link1]: https://docs.rs/arty/0.5.0/arty/?search=core::ThreadAware
 [__link10]: https://docs.rs/arty/0.5.0/arty/io/index.html
 [__link11]: https://docs.rs/arty/0.5.0/arty/core/index.html
 [__link12]: https://docs.rs/arty_macros/0.5.0/arty_macros/?search=main
 [__link13]: https://docs.rs/arty_macros/0.5.0/arty_macros/?search=test
 [__link14]: https://docs.rs/arty_macros/0.5.0/arty_macros/?search=main
 [__link15]: https://docs.rs/arty_macros/0.5.0/arty_macros/?search=test
 [__link16]: https://docs.rs/arty/0.5.0/arty/?search=time::ClockControl
 [__link2]: https://docs.rs/arty_macros/0.5.0/arty_macros/?search=main
 [__link3]: https://docs.rs/arty/0.5.0/arty/?search=task::JoinError
 [__link4]: https://docs.rs/arty/0.5.0/arty/?search=core::ThreadAware
 [__link5]: https://docs.rs/arty/0.5.0/arty/?search=task::Scheduler::spawn_anywhere
 [__link6]: https://docs.rs/arty/0.5.0/arty/?search=task::Scheduler::spawn_at
 [__link7]: https://docs.rs/arty/0.5.0/arty/runtime/index.html
 [__link8]: https://docs.rs/arty/0.5.0/arty/task/index.html
 [__link9]: https://docs.rs/arty/0.5.0/arty/time/index.html
