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

Each asynchronous task stays on one worker thread, so it can retain
thread-local and non-[`Send`][__link0] state across waits. A runtime can use several
workers; single-threaded execution applies to each task, not the whole process.
Thread-aware values carry a worker association that can be explicitly relocated.

Arty provides task scheduling, blocking-task pools, clocks, and telemetry.
It does not provide asynchronous I/O drivers or move running tasks between workers.

## Quickstart

Add Arty with the `macros` feature:

```sh
cargo add arty --features macros
```

```rust
use arty::runtime::Builtins;

#[arty::main]
async fn main(cx: Builtins) -> Result<(), arty::task::JoinError> {
    let answer = cx.scheduler().spawn(async |_| 6 * 7).await?;
    println!("{answer}");
    Ok(())
}
```

This prints `42`. [`main`][__link1] starts a runtime and stops it when the body returns.
Await child work that must finish first; shutdown cancels pending asynchronous tasks.

## Overview

* [`main`][__link2] and [`test`][__link3] start a runtime for an asynchronous entry point or test.
* [`Runtime`][__link4] provides explicit configuration, ownership, and shutdown.
* [`Builtins`][__link5] provides a task’s scheduler, clock, and worker coordinates.
* [`TaskScheduler`][__link6] submits asynchronous and blocking work.
  [`LocalTaskScheduler`][__link7] supports sharing non-`Send` state on one worker.
* [`Clock`][__link8] provides timers and timeouts. [`ClockControl`][__link9] provides controlled test time.
* [`Thread`][__link10] and [`ThreadAware`][__link11] describe
  worker coordinates and explicitly relocatable values.

The [guides][__link12] cover scheduling, thread awareness, shutdown, configuration, time, and telemetry.

## Features

No features are enabled by default.

* **`rt`** - Enables the runtime and task APIs, and implies `time`.
* **`macros`** - Enables [`main`][__link13] and [`test`][__link14], and implies `rt`.
* **`time`** - Enables clocks, timers, and timeouts.
* **`test-util`** - Enables testing utilities, including [`ClockControl`][__link15] with `time`.
  Enable it in dev-dependencies, not production dependencies.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQb2i7DQf3YWPIbnB4-qM4WlkkbFQ8i49jOjNMbScOrFlYjRqlhZIGCZGFydHllMC4zLjE
 [__link0]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link1]: https://docs.rs/arty/latest/arty/attr.main.html
 [__link10]: https://docs.rs/arty/0.3.1/arty/?search=core::Thread
 [__link11]: https://docs.rs/arty/0.3.1/arty/?search=core::ThreadAware
 [__link12]: https://docs.rs/arty/latest/arty/documentation/index.html
 [__link13]: https://docs.rs/arty/latest/arty/attr.main.html
 [__link14]: https://docs.rs/arty/latest/arty/attr.test.html
 [__link15]: https://docs.rs/arty/latest/arty/time/struct.ClockControl.html
 [__link2]: https://docs.rs/arty/latest/arty/attr.main.html
 [__link3]: https://docs.rs/arty/latest/arty/attr.test.html
 [__link4]: https://docs.rs/arty/latest/arty/runtime/struct.Runtime.html
 [__link5]: https://docs.rs/arty/latest/arty/runtime/struct.Builtins.html
 [__link6]: https://docs.rs/arty/latest/arty/task/struct.TaskScheduler.html
 [__link7]: https://docs.rs/arty/latest/arty/task/struct.LocalTaskScheduler.html
 [__link8]: https://docs.rs/arty/latest/arty/time/struct.Clock.html
 [__link9]: https://docs.rs/arty/latest/arty/time/struct.ClockControl.html
