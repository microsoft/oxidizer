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

Contracts for integrating I/O drivers with an async runtime.

This crate defines the types shared by runtimes and drivers that evolve independently. It
provides neither a runtime nor an I/O implementation.

## Core types

* [`IoContext`][__link0] is the consumer-facing handle that selects a [`DriverProvider`][__link1].
* [`DriverProvider`][__link2] creates one [`Driver`][__link3] for each runtime worker.
* [`Driver`][__link4] is the thread-local adapter between a worker and an I/O subsystem.
* [`ProviderOptions`][__link5] and [`DriverOptions`][__link6] carry runtime facilities during registration.
* [`DriverHandle`][__link7] lets drivers discover peers registered on the same worker.
* [`SystemTaskSpawner`][__link8] runs blocking system work on runtime-owned threads.
* [`ShutdownError`][__link9] reports a failure to complete graceful shutdown.

## Registration

A runtime registers a driver when an [`IoContext`][__link10] type is first requested. It calls
[`IoContext::provider`][__link11] once, then clones and relocates the provider for each active worker.
Each relocated provider is consumed by [`DriverProvider::create`][__link12].

[`DriverOptions`][__link13] identifies the worker, supplies runtime facilities, and contains handles to
drivers registered earlier on that worker. After storing the new driver, the runtime calls
[`Driver::on_peer_registered`][__link14] on those earlier drivers. This gives both the new driver and its
peers an opportunity to exchange independently owned shared state.

The first context request completes after every active worker has created its driver. Later
requests obtain a context from the driver on the calling worker. Registration is infallible at
the type level: a provider or peer callback panics if registration cannot be completed.

## Driving I/O

A runtime owns each driver and invokes its methods only on the worker that created it. Drivers
are therefore not required to implement [`Send`][__link15] or [`Sync`][__link16].

[`Driver::process_completions`][__link17] processes pending work and optionally waits for more.
[`Driver::waker`][__link18] interrupts the current or next blocking wait. Contexts may move between
workers and may outlive the driver that created them.

State reachable from a context, waker, background thread, or operating-system callback must be
owned independently of the driver and synchronized as necessary. State used only by the owning
worker may remain directly in the driver.

## Shutdown

[`Driver::shutdown`][__link19] consumes the driver, closes admission to new operations, and waits for
active work to drain. It returns [`ShutdownError`][__link20] when graceful cleanup cannot be completed.
The runtime keeps [`SystemTaskSpawner`][__link21] available until shutdown returns.

A driver must be safe to drop at every point in its lifecycle, including during unwinding and
after a shutdown error. Contexts remain valid after shutdown but reject new operations.

## Example

The [single-thread runtime example][__link22]
demonstrates lazy registration of two context types and same-worker driver discovery.

## Project documents

* [Requirements][__link23]
* [Design][__link24]


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_io_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbsTA_t6PiRm0b7uZ7BHrrPdQb50KJPErsLWIbWI1bGa1i7dFhZIGCbGFydHlfaW9fY29yZWUwLjIuMA
 [__link0]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=IoContext
 [__link1]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider
 [__link10]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=IoContext
 [__link11]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=IoContext::provider
 [__link12]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider::create
 [__link13]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverOptions
 [__link14]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::on_peer_registered
 [__link15]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link16]: https://doc.rust-lang.org/stable/std/marker/trait.Sync.html
 [__link17]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::process_completions
 [__link18]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::waker
 [__link19]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::shutdown
 [__link2]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider
 [__link20]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=ShutdownError
 [__link21]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTaskSpawner
 [__link22]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/examples/single_thread_runtime/main.rs
 [__link23]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/REQUIREMENTS.md
 [__link24]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md
 [__link3]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver
 [__link4]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver
 [__link5]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=ProviderOptions
 [__link6]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverOptions
 [__link7]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverHandle
 [__link8]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTaskSpawner
 [__link9]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=ShutdownError
