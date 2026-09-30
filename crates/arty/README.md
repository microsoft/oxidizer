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

Thread-aware, thread-per-core application runtime.

Each worker has a single-threaded executor: a task remains on its original worker for its
entire lifetime. An `arty::runtime::Runtime` owns worker startup and shutdown. Its
`task_scheduler()` distributes work round-robin, while a task’s
`Builtins::scheduler` preserves worker affinity. Futures are constructed on the destination
worker and need not be [`Send`][__link0].

```rust
use arty::runtime::Runtime;

let runtime = Runtime::new().unwrap();
let scheduler = runtime.task_scheduler();
let answer = scheduler
    .spawn(async |cx| cx.scheduler().spawn(async |_| 42).await)
    .wait();
assert_eq!(answer, 42);
```

Arty provides scheduling, blocking tasks, clocks, and structured telemetry. It does
not provide asynchronous I/O drivers or memory pools. External I/O integration through
[`arty_io_core`][__link1] is planned separately.

## Features

No features are enabled by default.

* **`rt`** - Enables `arty::runtime` and `arty::task`, and implies `time`.
* **`macros`** - Enables `#[arty::main]` and `#[arty::test]` and implies `rt`.
* **`time`** - Exposes time primitives through `arty::time`.
* **`test-util`** - Enables test-only runtime utilities. With `time`, this includes
  `arty::time::ClockControl`. Under Miri, runtime tests use a simulated
  six-processor topology instead of native processor discovery and pinning.

## Project policies

* [Design][__link2]
* [I/O][__link3]
* [Panics][__link4]
* [Stabilization][__link5]


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbQtbu7K4PchsbOaL3fiio7zMbNOPIbE4I8VEb-HDBMMKNY6phZIGCbGFydHlfaW9fY29yZWUwLjIuMA
 [__link0]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link1]: https://crates.io/crates/arty_io_core/0.2.0
 [__link2]: https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/DESIGN.md
 [__link3]: https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/IO.md
 [__link4]: https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/PANICS.md
 [__link5]: https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/STABILIZATION.md
