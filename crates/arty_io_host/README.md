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

Hosts I/O drivers for the unstable Arty runtime.

This crate supports [`arty`][__link0]. For applications and
libraries, use `arty` instead of depending on this crate directly.

Arty uses these public items across a crate boundary. The items do not form
a standalone stable API.

## Quick start

Create one [`IoRuntime`][__link1] for the runtime. Clone it for each worker, then
create a [`DriverHost`][__link2] with the worker’s thread coordinate:

```rust
// One value owns the shared context registry and telemetry sink.
let runtime = IoRuntime::new(Sink::noop());
let worker = ThreadBuilder::default().build(std::thread::current().id());

// Each worker creates its host on the thread that its coordinate identifies.
let host = runtime.clone().driver_host(worker, Waker::noop().clone())?;

// The worker loop supplies the available wait time.
host.execute_cycle(Duration::ZERO)?;
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_io_host">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbJ2aQtYLoZPIbqsQr94rGs5QbLqqiU9bMpXEb2Rsmum1bD65hZIGCbGFydHlfaW9faG9zdGUwLjYuMQ
 [__link0]: https://docs.rs/arty
 [__link1]: https://docs.rs/arty_io_host/0.6.1/arty_io_host/?search=IoRuntime
 [__link2]: https://docs.rs/arty_io_host/0.6.1/arty_io_host/?search=DriverHost
