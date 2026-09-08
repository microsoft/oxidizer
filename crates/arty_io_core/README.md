<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Arty Io Core Logo" width="96">

# Arty Io Core

[![crate.io](https://img.shields.io/crates/v/arty_io_core.svg)](https://crates.io/crates/arty_io_core)
[![docs.rs](https://docs.rs/arty_io_core/badge.svg)](https://docs.rs/arty_io_core)
[![MSRV](https://img.shields.io/crates/msrv/arty_io_core)](https://crates.io/crates/arty_io_core)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Stable contracts for integrating external I/O drivers with the Arty runtime.

The runtime hosts drivers supplied by libraries and applications rather than depending on one
I/O implementation. This crate contains the small vocabulary both sides share:

* [`Driver`][__link0] is the adapter between one worker and an I/O subsystem.
* [`DriverContext`][__link1] associates a requested context type with its provider.
* [`DriverProvider`][__link2] creates and connects the per-worker adapters for a driver.
* [`DriverInit`][__link3] describes the worker and runtime facilities available during creation.
* [`SystemTasks`][__link4] lets a driver delegate blocking system work to the runtime.

Registration and driver placement are runtime behavior, not part of this crate. Keeping those
policies outside the contract allows the runtime and drivers to evolve independently.

## Example

The [fixed two-thread runtime example][__link5]
starts both worker threads before `get_context::<SampleContext>()` uses the context type to
inject its associated driver.

## Project documents

* [Requirements][__link6]
* [Design][__link7]


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_io_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQb36KB2NObzDEb6W_o1opCjNwbOnRX5ag2HJ4bErB6SbO8DiFhZIGCbGFydHlfaW9fY29yZWUwLjIuMA
 [__link0]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver
 [__link1]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverContext
 [__link2]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider
 [__link3]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverInit
 [__link4]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTasks
 [__link5]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/examples/two_thread_runtime/main.rs
 [__link6]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/REQUIREMENTS.md
 [__link7]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md
