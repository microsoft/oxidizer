<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Arty Io Host Logo" width="96">

# Arty Io Host

[![crate.io](https://img.shields.io/crates/v/arty_io_host.svg)](https://crates.io/crates/arty_io_host)
[![docs.rs](https://docs.rs/arty_io_host/badge.svg)](https://docs.rs/arty_io_host)
[![MSRV](https://img.shields.io/crates/msrv/arty_io_host)](https://crates.io/crates/arty_io_host)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Unstable I/O driver hosting utilities for the Arty runtime.

This crate is implementation infrastructure for [`arty`][__link0].
Applications and libraries should use `arty` instead of depending on this
crate directly.

The items are public because Arty consumes them across a crate boundary.
They do not form a standalone stability or compatibility promise.

## Quick start

Clone the runtime-wide registry for each worker, relocate the clone onto
that worker, and then create its driver host:

```rust
// One runtime-wide value owns the shared context registry and telemetry sink.
let runtime = IoRuntime::new(Sink::noop());
let worker = ThreadBuilder::default().build(std::thread::current().id());

// Each worker receives a clone that must be relocated on the destination
// thread before its worker-local host is created.
let mut worker_runtime = runtime.clone();
worker_runtime.relocate(None, &worker);
let host = worker_runtime.driver_host(Waker::noop().clone());

// The runtime calls this from its worker loop with the available wait time.
host.execute_cycle(Duration::ZERO)?;
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_io_host">source code</a>.
</sub>

 [__link0]: https://docs.rs/arty
