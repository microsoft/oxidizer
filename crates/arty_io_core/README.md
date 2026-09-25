<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Arty Io Core Logo" width="96">

# Arty Io Core

[![crate.io](https://img.shields.io/crates/v/arty_io_core.svg)](https://crates.io/crates/arty_io_core)
[![docs.rs](https://docs.rs/arty_io_core/badge.svg)](https://docs.rs/arty_io_core)
[![MSRV](https://img.shields.io/crates/msrv/arty_io_core)](https://crates.io/crates/arty_io_core)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Contracts for integrating I/O drivers with an async runtime.

This crate defines the types shared by runtimes and independently versioned drivers. It
provides neither a runtime nor an I/O implementation.

## Core types

* [`IoContext`][__link0] selects a [`DriverProvider`][__link1].
* [`DriverProvider`][__link2] creates one [`Driver`][__link3] and context per runtime worker.
* [`DriverRole`][__link4] identifies an optional worker-blocking primary and non-blocking secondaries.
* [`Cycle`][__link5] supplies a shared time snapshot, wait bound, and [`Coordinator`][__link6].
* [`DriverOptions`][__link7] supplies per-worker construction facilities and peer handles.
* [`SystemTaskSpawner`][__link8] runs blocking system work outside async workers.
* [`DriverError`][__link9] and [`ShutdownError`][__link10] report infrastructure and cleanup failures.

## Registration

The first request for an [`IoContext`][__link11] creates its provider and initializes a driver/context
pair on every active worker. Before publishing a context, the runtime assigns the driver’s
role and completes a separate zero-wait cycle. The new driver sees earlier drivers through
[`DriverOptions::drivers`][__link12]; earlier drivers receive the new driver’s handle through
[`Driver::on_peer_registered`][__link13]. Later requests reuse the registration.

## Driving I/O

A runtime invokes secondary drivers first and the primary last. Every driver receives the same
[`Cycle::max_wait`][__link14]. A primary may block its worker for that duration. A secondary must return
promptly and may use the duration only for a wait scheduled on a background thread.

Drivers create non-cloneable [`PendingWork`][__link15] values with [`Cycle::start_work`][__link16] and attach
native-wait callbacks with [`PendingWork::on_interrupt`][__link17]. The runtime waits for every
pending-work value after the primary returns and before starting the next cycle. A driver calls
[`PendingWork::complete`][__link18] after publishing work, or drops the value if its wait ended
without work.

## Shutdown

[`Driver::shutdown`][__link19] consumes the driver, closes admission, and blocks until cleanup completes
or fails. Contexts remain valid as closed handles. A driver must not depend on work that can
run only after its shutdown returns.

## Project documents

* [Requirements][__link20]
* [Design][__link21]
* [Completion coordination (exploratory)][__link22]


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_io_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbJkbjaOrMtWIbKjmpdkOQbwgbxq-3zUpics8bsxFlV09vjkxhZIGCbGFydHlfaW9fY29yZWUwLjIuMA
 [__link0]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=IoContext
 [__link1]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider
 [__link10]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=ShutdownError
 [__link11]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=IoContext
 [__link12]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverOptions::drivers
 [__link13]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::on_peer_registered
 [__link14]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Cycle::max_wait
 [__link15]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=PendingWork
 [__link16]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Cycle::start_work
 [__link17]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=PendingWork::on_interrupt
 [__link18]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=PendingWork::complete
 [__link19]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::shutdown
 [__link2]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider
 [__link20]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/REQUIREMENTS.md
 [__link21]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md
 [__link22]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/COMPLETION_COORDINATION.md
 [__link3]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver
 [__link4]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverRole
 [__link5]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Cycle
 [__link6]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Coordinator
 [__link7]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverOptions
 [__link8]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTaskSpawner
 [__link9]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverError
