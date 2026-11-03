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

## Task scheduling

Choose where a new task starts with one of these methods:

* [`Scheduler::spawn`][__link4] uses the worker bound
  to the scheduler. A cloned scheduler keeps that binding, even when
  another thread calls it.
* [`Scheduler::spawn_anywhere`][__link5] lets
  the runtime select a worker. The runtime can select the same worker.
* [`Scheduler::spawn_at`][__link6] uses a worker
  you select from the same runtime with
  [`RuntimeControl::workers`][__link7].

After a task starts, it stays on its assigned worker. If it awaits an
operation that is not ready, that worker can poll another ready task
assigned to it.

For `spawn_anywhere` and `spawn_at`, pass task data separately from the
factory. The data must implement [`ThreadAware`][__link8]. Arty
calls its relocation method on the destination worker before the factory
runs. The implementation can update worker-specific state or refresh an
optimization, such as a cache. Relocation must preserve real data and
cannot affect correctness.

If Arty rejects a task or cancels it before the factory starts, it drops the
data without relocating it.

```rust
let answer = cx
    .scheduler()
    .spawn_anywhere(
        (cx.clock().clone(), 42_u32),
        |(clock, answer)| async move {
            clock.delay(std::time::Duration::from_millis(1)).await;
            answer
        },
    )
    .await?;
assert_eq!(answer, 42);
```

Tuples implement `ThreadAware` when every element does. Here, `Clock` and
`u32` implement it. Arty relocates the clock to the selected worker before
the factory runs. The factory waits for 1 ms, then returns the answer.

## API overview

* [`runtime`][__link9] explains how to create, configure, and stop a runtime.
* [`task`][__link10] explains how to start tasks, choose their workers, and receive results.
* [`time`][__link11] provides clocks, delays, timeouts, and control over time in tests.
* [`io`][__link12] connects I/O libraries to the runtime.
* [`core`][__link13] describes workers and how values update their state for a worker.
* [`main`][__link14] and [`test`][__link15] start and stop a runtime for an async function or test.

## Features

The default feature set enables `rt`, `macros`, and `monitor`.

* `rt`: Enables the runtime, task, and I/O APIs.
* `macros`: Enables [`main`][__link16] and [`test`][__link17]. It also enables `rt`.
* `monitor`: Enables the Seismograph monitor dependency. It forwards
  monitoring support for `performables` when `performables` is enabled. It does
  not start monitoring or emit new telemetry.
* `test-util`: Enables testing utilities, including
  [`ClockControl`][__link18].
  Enable it in dev-dependencies, not production dependencies.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQb0n095s03xAIbbpfC7FNGc2Ab7jYkYxugQoobKMJKeji9Ff1hZIKCZGFydHllMC42LjCCa2FydHlfbWFjcm9zZTAuNi4w
 [__link0]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link1]: https://docs.rs/arty/0.6.0/arty/?search=core::ThreadAware
 [__link10]: https://docs.rs/arty/0.6.0/arty/task/index.html
 [__link11]: https://docs.rs/arty/0.6.0/arty/time/index.html
 [__link12]: https://docs.rs/arty/0.6.0/arty/io/index.html
 [__link13]: https://docs.rs/arty/0.6.0/arty/core/index.html
 [__link14]: https://docs.rs/arty_macros/0.6.0/arty_macros/?search=main
 [__link15]: https://docs.rs/arty_macros/0.6.0/arty_macros/?search=test
 [__link16]: https://docs.rs/arty_macros/0.6.0/arty_macros/?search=main
 [__link17]: https://docs.rs/arty_macros/0.6.0/arty_macros/?search=test
 [__link18]: https://docs.rs/arty/0.6.0/arty/?search=time::ClockControl
 [__link2]: https://docs.rs/arty_macros/0.6.0/arty_macros/?search=main
 [__link3]: https://docs.rs/arty/0.6.0/arty/?search=task::JoinError
 [__link4]: https://docs.rs/arty/0.6.0/arty/?search=task::Scheduler::spawn
 [__link5]: https://docs.rs/arty/0.6.0/arty/?search=task::Scheduler::spawn_anywhere
 [__link6]: https://docs.rs/arty/0.6.0/arty/?search=task::Scheduler::spawn_at
 [__link7]: https://docs.rs/arty/0.6.0/arty/?search=runtime::RuntimeControl::workers
 [__link8]: https://docs.rs/arty/0.6.0/arty/?search=core::ThreadAware
 [__link9]: https://docs.rs/arty/0.6.0/arty/runtime/index.html
