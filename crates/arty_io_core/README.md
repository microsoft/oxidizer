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

Drivers provide I/O; runtimes provide scheduling. This crate defines
the interfaces between them, but provides neither a runtime nor an I/O implementation.

## How drivers work

Applications access I/O through an [`IoContext`][__link0]. The runtime creates one worker-local
[`Driver`][__link1] for that context on each async worker. The driver submits I/O and processes
completed operations in [`Driver::execute_cycle`][__link2].

```rust
use std::task::Waker;

use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, DriverRole, IoContext};
use thread_aware_core::{Thread, ThreadAware};

#[derive(Clone)]
struct Context;

struct ExampleDriver;

impl ThreadAware for Context {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl IoContext for Context {
    type ContextOptions = ();
    type Driver = ExampleDriver;

    fn context((): Self::ContextOptions) -> Self {
        Self
    }

    fn driver(&mut self, options: DriverOptions) -> Result<Self::Driver, DriverError> {
        let _role = options.role();
        Ok(ExampleDriver)
    }
}

impl Driver for ExampleDriver {
    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }

    fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
        Ok(())
    }

    fn shutdown(self) -> Result<(), DriverError> {
        Ok(())
    }
}

```

### Primary and secondary drivers

An async worker has at most one [`Primary`][__link3] driver. [`DriverOptions`][__link4]
provides the role assigned during creation. A primary may wait for I/O for up to
[`Cycle::max_wait`][__link5].

The runtime calls [`Secondary`][__link6] drivers with
`Cycle::max_wait() == Duration::ZERO`. A secondary processes completions that are already ready
or returns immediately if another thread handles its notifications. A secondary must wake its
async worker when another cycle is needed.

## Runtime responsibilities

The runtime registers each concrete context type once. When an async worker first retrieves a context,
the runtime clones it, calls [`ThreadAware::relocate`][__link7],
then calls [`IoContext::driver`][__link8]. The relocation destination and [`DriverOptions::thread`][__link9]
identify the same worker. [`DriverOptions`][__link10] also supplies the assigned role.

The runtime calls `driver` at most once per registration and async worker, not once per clone.
Application-facing operations require successful initialization. Implementations can reject
duplicate driver creation and operations on contexts without that initialization.
Clones of initialized contexts remain usable without another driver call.
Later relocation remains optional under [`ThreadAware`][__link11].

The runtime invokes secondaries before the primary. If there is no primary, the runtime waits
for work itself. The owner waker in [`DriverOptions`][__link12] wakes the async worker, while
[`Driver::waker`][__link13] makes a waiting driver return promptly.

A driver creation or cycle error is fatal. The runtime starts shutdown on every async worker
and calls `shutdown` on every driver that was created successfully, including the driver whose
cycle failed. Contexts already returned by another worker become unusable as shutdown closes
them to new operations.

## Shutdown

When an async worker shuts down, the runtime stops calling `execute_cycle` and synchronously
calls [`Driver::shutdown`][__link14] for every driver owned by that worker. Each driver rejects new
operations and uses a finite shutdown limit. A returned [`DriverError`][__link15] does not prevent the
runtime from attempting to shut down the remaining drivers.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_io_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbiH3-MZq7tAIbXrw40SxM9_Ubos3NchztrvsbxRLND6Vfa8RhZIKCbGFydHlfaW9fY29yZWUwLjQuMIJxdGhyZWFkX2F3YXJlX2NvcmVlMC4xLjM
 [__link0]: https://docs.rs/arty_io_core/0.4.0/arty_io_core/?search=IoContext
 [__link1]: https://docs.rs/arty_io_core/0.4.0/arty_io_core/?search=Driver
 [__link10]: https://docs.rs/arty_io_core/0.4.0/arty_io_core/?search=DriverOptions
 [__link11]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=ThreadAware
 [__link12]: https://docs.rs/arty_io_core/0.4.0/arty_io_core/?search=DriverOptions
 [__link13]: https://docs.rs/arty_io_core/0.4.0/arty_io_core/?search=Driver::waker
 [__link14]: https://docs.rs/arty_io_core/0.4.0/arty_io_core/?search=Driver::shutdown
 [__link15]: https://docs.rs/arty_io_core/0.4.0/arty_io_core/?search=DriverError
 [__link2]: https://docs.rs/arty_io_core/0.4.0/arty_io_core/?search=Driver::execute_cycle
 [__link3]: https://docs.rs/arty_io_core/0.4.0/arty_io_core/?search=DriverRole::Primary
 [__link4]: https://docs.rs/arty_io_core/0.4.0/arty_io_core/?search=DriverOptions
 [__link5]: https://docs.rs/arty_io_core/0.4.0/arty_io_core/?search=Cycle::max_wait
 [__link6]: https://docs.rs/arty_io_core/0.4.0/arty_io_core/?search=DriverRole::Secondary
 [__link7]: https://docs.rs/thread_aware_core/0.1.3/thread_aware_core/?search=ThreadAware::relocate
 [__link8]: https://docs.rs/arty_io_core/0.4.0/arty_io_core/?search=IoContext::driver
 [__link9]: https://docs.rs/arty_io_core/0.4.0/arty_io_core/?search=DriverOptions::thread
