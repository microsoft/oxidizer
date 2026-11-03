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

Single-threaded, thread-aware runtime for async applications.

Each async task stays on one worker thread for its entire life, even when
Arty manages multiple workers. Tasks can keep thread-local and non-[`Send`][__link0]
state across awaits.

Arty can pass values between workers without moving running tasks.
The [`ThreadAware`][__link1] trait lets a value update its state
for the worker that receives it. This update is called relocation.

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
The `?` returns a [`JoinError`][__link3] if the child task fails.
Await child tasks before returning. Shutdown cancels pending async tasks.

## Execution model

Each worker is one operating-system thread with a cooperative executor.
The worker thread polls the tasks assigned to it. When one task awaits an
operation that is not ready, the worker can poll another assigned task.
After a task starts, it stays on that worker thread for its entire life.

Choose the initial worker for a new task:

* [`Scheduler::spawn`][__link4] uses the worker bound
  to the scheduler. A cloned scheduler keeps the same binding, even when
  another thread calls it.
* [`Scheduler::spawn_anywhere`][__link5] lets
  the runtime select a worker. This initial placement is similar to Tokio
  task placement.
* [`Scheduler::spawn_at`][__link6] uses a worker
  that you select from the same runtime.

Unlike a Tokio task, an Arty task does not move between workers after it
starts.

A task factory creates the task future on its destination worker. A payload
is a separate value passed to that factory. If the factory starts, Arty
relocates a [`ThreadAware`][__link7] payload before the factory
uses it. Rejected tasks and tasks cancelled before their factory starts can
drop the payload without relocation.

Cloned values can refer to the same resource while keeping separate state
on each worker. `ThreadAware` updates that state, but it does not share
ownership of a resource or store local state for you.

This example compares fixed placement with runtime-selected placement:

```rust
let parent_worker = cx.thread().id();
let child_worker = cx
    .scheduler()
    .spawn(async |child| child.thread().id())
    .await?;
assert_eq!(child_worker, parent_worker);

// `cx.clone()` is the payload. The closure is the task factory.
cx.scheduler()
    .spawn_anywhere(cx.clone(), |moved| async move {
        assert_eq!(moved.thread().id(), std::thread::current().id());
    })
    .await?;
```

`spawn_anywhere` can select the original worker. The relocated context
always reports the worker that runs the new task.

Each runtime owns its workers and services. Arty does not use a single
runtime shared by the whole process. Each worker tracks its own state.

Arty runs tasks, provides clocks, and runs blocking functions on separate threads.
It can also run I/O drivers, which handle input and output.
I/O libraries connect to Arty through `arty_io_core`.
Related I/O crates ship alongside Arty.

## API overview

* [`runtime`][__link8] explains how to create, configure, and stop a runtime.
* [`task`][__link9] explains how to start tasks, choose their workers, and receive results.
* [`time`][__link10] provides clocks, delays, timeouts, and control over time in tests.
* [`io`][__link11] connects I/O libraries to the runtime.
* [`core`][__link12] describes workers and how values update their state for a worker.
* [`main`][__link13] and [`test`][__link14] start and stop a runtime for an async function or test.

## Features

The default feature set enables `rt`, `macros`, and `monitor`.

* `rt`: Enables the runtime, task, and I/O APIs.
* `macros`: Enables [`main`][__link15] and [`test`][__link16]. It also enables `rt`.
* `monitor`: Enables the Seismograph monitor dependency. It forwards
  monitoring support for `performables` when `performables` is enabled. It does
  not start monitoring or emit new telemetry.
* `test-util`: Enables testing utilities, including
  [`ClockControl`][__link17].
  Enable it in dev-dependencies, not production dependencies.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbVOOkiwtK_84bGyYJldq1SSMbSs5R1-8kbtkbwouPZ79D401hZIKCZGFydHllMC42LjCCa2FydHlfbWFjcm9zZTAuNi4w
 [__link0]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link1]: https://docs.rs/arty/0.6.0/arty/?search=core::ThreadAware
 [__link10]: https://docs.rs/arty/0.6.0/arty/time/index.html
 [__link11]: https://docs.rs/arty/0.6.0/arty/io/index.html
 [__link12]: https://docs.rs/arty/0.6.0/arty/core/index.html
 [__link13]: https://docs.rs/arty_macros/0.6.0/arty_macros/?search=main
 [__link14]: https://docs.rs/arty_macros/0.6.0/arty_macros/?search=test
 [__link15]: https://docs.rs/arty_macros/0.6.0/arty_macros/?search=main
 [__link16]: https://docs.rs/arty_macros/0.6.0/arty_macros/?search=test
 [__link17]: https://docs.rs/arty/0.6.0/arty/?search=time::ClockControl
 [__link2]: https://docs.rs/arty_macros/0.6.0/arty_macros/?search=main
 [__link3]: https://docs.rs/arty/0.6.0/arty/?search=task::JoinError
 [__link4]: https://docs.rs/arty/0.6.0/arty/?search=task::Scheduler::spawn
 [__link5]: https://docs.rs/arty/0.6.0/arty/?search=task::Scheduler::spawn_anywhere
 [__link6]: https://docs.rs/arty/0.6.0/arty/?search=task::Scheduler::spawn_at
 [__link7]: https://docs.rs/arty/0.6.0/arty/?search=core::ThreadAware
 [__link8]: https://docs.rs/arty/0.6.0/arty/runtime/index.html
 [__link9]: https://docs.rs/arty/0.6.0/arty/task/index.html
