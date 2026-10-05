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

Stable contracts for integrating I/O drivers into thread-aware runtimes.

Drivers provide I/O; runtimes provide scheduling and work coordination. This crate defines
the interfaces between them, but provides neither a runtime nor an I/O implementation.

## How drivers work

Applications access a driver’s I/O operations through an [`IoContext`][__link0]. Its [`DriverProvider`][__link1]
creates a context and a worker-local [`Driver`][__link2] for each runtime worker. The driver processes
submissions and completions in bounded calls to [`Driver::execute_cycle`][__link3].

Drivers expose a [`Driver::waker`][__link4] for interrupting pending completion waits. Wakes are
latched until the wait observes them, and the waker remains safe after the driver is dropped.

### Primary and secondary drivers

A worker has at most one [`Primary`][__link5] driver. The runtime grants the
primary permission through [`DriverOptions::role`][__link6], and the provider returns the role selected
by the driver in [`DriverInstance`][__link7]. A primary may wait on the worker for up to
[`Cycle::max_wait`][__link8]; a zero wait bound means no waiting.

[`Secondary`][__link9] drivers must not block the worker. They may coordinate
with other drivers or continuously process completions on a driver-owned background thread.

## Runtime responsibilities

The runtime clones and relocates providers to their workers, grants role permission through
[`DriverOptions`][__link10], and receives a [`DriverInstance`][__link11] containing the selected role. It
completes a non-blocking, zero-wait initialization cycle before publishing a context.
It also supplies a [`SystemTaskSpawner`][__link12] for blocking system work.

Each logical cycle supplies a wait bound. The runtime invokes secondaries before the primary.
Drivers own coordination for work that continues beyond their worker-local call. If there is
no primary, the runtime retains responsibility for parking the worker.

## Shutdown

The runtime stops normal cycles and calls [`Driver::shutdown`][__link13] for every driver, continuing
after a [`ShutdownError`][__link14]. Each driver closes admission and drains its resources within a
bounded wait, independently of other drivers on the same worker. Contexts remain valid as
closed handles.

## Example and reference

The [single-thread runtime example][__link15] demonstrates registration and driver roles. Its sample
drivers perform no I/O and use no-op wakers; a runtime serving native I/O must implement the
coordination described above.

* [Requirements][__link16]
* [Design][__link17]


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_io_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbo80RgLIZH90bZ-TYlntTmXgbl_i1fO4XZqMbBUHTijkZSzJhZIGCbGFydHlfaW9fY29yZWUwLjIuMA
 [__link0]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=IoContext
 [__link1]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider
 [__link10]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverOptions
 [__link11]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverInstance
 [__link12]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTaskSpawner
 [__link13]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::shutdown
 [__link14]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=ShutdownError
 [__link15]: https://github.com/microsoft/oxidizer/tree/main/crates/arty_io_core/examples/single_thread_runtime
 [__link16]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/REQUIREMENTS.md
 [__link17]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md
 [__link2]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver
 [__link3]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::execute_cycle
 [__link4]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::waker
 [__link5]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverRole::Primary
 [__link6]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverOptions::role
 [__link7]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverInstance
 [__link8]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Cycle::max_wait
 [__link9]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverRole::Secondary
