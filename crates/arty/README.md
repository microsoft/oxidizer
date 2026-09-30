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

A runtime for worker-local asynchronous tasks.

Arty makes worker-local execution and explicit relocation of runtime capabilities
the core programming model. It is intended for applications that organize work
around state owned by individual workers, rather than treating locality as an
addition to a general-purpose scheduler.

Unlike [Tokio’s multithreaded scheduler][__link0],
Arty does not move started tasks between workers to balance their load. Each task
stays on its worker, where it can create and retain non-[`Send`][__link1] state.
Tokio also supports [local tasks][__link2];
the distinction is Arty’s worker-local model, not exclusive support for non-`Send` futures.
The trade-off is that a busy worker’s tasks are not redistributed. Arty provides
scheduling, blocking tasks, clocks, and telemetry, but no asynchronous I/O drivers
or built-in memory pools. It is not a drop-in replacement for Tokio.

**Thread awareness** means that runtime capabilities have an explicit worker
association. Each task receives an owned `Builtins` value containing its worker’s
scheduler and clock. Cloning preserves that association; explicit relocation can
rebind capabilities to another worker in the same runtime. Relocation does not
migrate a running task or automatically relocate ordinary task results.

## Quickstart

Enable `macros` to use the runtime entry points; it also enables `rt` and
`time`. To try the runtime from this repository, use a Git revision that
contains these APIs:

```toml
[dependencies]
arty = { git = "https://github.com/microsoft/oxidizer", rev = "65f7f337b14b59259ad500484439a77f1f7f4f23", features = ["macros"] }
```

```rust
use arty::runtime::Builtins;

#[arty::main]
async fn main(cx: Builtins) {
    let answer = cx
        .scheduler()
        .spawn(async |_| 6 * 7)
        .await
        .expect("the child task completes before the entry point returns");
    println!("{answer}");
}
```

This prints `42`. The attribute creates the runtime and runs the entry point
on a worker, passing owned `Builtins`. Its scheduler creates the child task
on that worker; `.await` observes a `Result` without blocking the worker.
Task panics and shutdown cancellation are reported as `arty::task::JoinError`.
The runtime shuts down when the entry point finishes, so await any required
child work before returning. Use `arty::runtime::Runtime` directly when
integrating with synchronous code or controlling ownership and shutdown.

## Documentation

`arty::documentation` contains longer guides to scheduling,
thread awareness, lifecycle, configuration, time, and telemetry. It is included
in documentation and test builds only when all Arty features are enabled.
The dependency revision above selects the runtime API and predates these
guides. From a source checkout containing the documentation module, run:

```text
cargo doc -p arty --all-features --no-deps --open
```

Select `documentation` in the generated API reference’s module index.
Hosted docs describe published releases and may not yet include guides from
unreleased source. Application dependencies need only the features they use;
building all-feature documentation does not require a production `test-util`
dependency.

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

 [__link0]: https://docs.rs/tokio/latest/tokio/runtime/#multi-thread-scheduler
 [__link1]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link2]: https://docs.rs/tokio/latest/tokio/task/struct.LocalSet.html
