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

A runtime for asynchronous tasks with stable thread placement.

Use Arty when tasks need to retain thread-local state across asynchronous waits.
Each task runs on one worker thread until it completes or is cancelled, so its
future can hold non-[`Send`][__link0] values such as [`Rc`][__link1].

Arty provides task scheduling, blocking-task pools, clocks, and telemetry.
It does not provide asynchronous I/O drivers or built-in memory pools, and it
does not move started tasks between workers to balance their load. Libraries
that require another runtime’s I/O drivers need that runtime’s integration.

## Quickstart

Enable `macros` to use `#[arty::main]`; it also enables `rt` and `time`.
These runtime APIs are not yet published. The following dependency selects a
revision that provides them:

```toml
[dependencies]
arty = { git = "https://github.com/microsoft/oxidizer", rev = "27c6370ea051ece01c519b9a3174413d41b14d64", features = ["macros"] }
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

This prints `42`. The attribute starts a runtime, runs the asynchronous body
on a worker, and shuts down when the body returns. `cx` provides that worker’s
scheduler and clock. Awaiting the child task receives its result without
blocking the worker; `?` propagates a task panic or shutdown cancellation.

Await any child work that must finish before returning from the entry point.
Shutdown cancels pending asynchronous tasks rather than draining them.
Use `runtime::Runtime` directly to integrate with synchronous code, borrow
caller-owned data, or control when the runtime stops.

## Task placement

A task’s `Builtins::scheduler()` creates child tasks on the same worker.
`Runtime::task_scheduler()` distributes submissions across workers.
`Builtins::local_scheduler()` also accepts non-`Send` captures and results
when called on the associated worker.

Cloning a scheduler or `Builtins` preserves its worker association.
`TaskScheduler::spawn_anywhere()` can distribute new work and explicitly
relocate its payload’s capabilities. It does not migrate an existing task
or relocate a task’s returned value.

Keep synchronous blocking calls off asynchronous workers: use
`TaskScheduler::spawn_blocking()` so other tasks and timers can make progress.

## Documentation

The `documentation` module contains guides to scheduling, thread awareness,
shutdown, configuration, time, and telemetry. To include all the guides when
building documentation from a source checkout, enable all features:

```text
cargo doc -p arty --all-features --no-deps --open
```

Application dependencies need only the features they use. Enabling all
features for documentation does not require using `test-util` in production.
Hosted documentation describes published releases, which may not yet contain
these runtime APIs.

## Features

No features are enabled by default.

* **`rt`** - Enables `arty::runtime` and `arty::task`, and implies `time`.
* **`macros`** - Enables `#[arty::main]` and `#[arty::test]` and implies `rt`.
* **`time`** - Exposes time primitives through `arty::time`.
* **`test-util`** - Enables testing utilities, including `arty::time::ClockControl`
  when `time` is enabled. Enable it in dev-dependencies, not production dependencies.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty">source code</a>.
</sub>

 [__link0]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link1]: https://doc.rust-lang.org/stable/std/?search=rc::Rc
