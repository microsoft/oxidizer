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

Applications access a driver’s I/O operations through an [`IoContext`][__link0]. It creates a
worker-local [`Driver`][__link1] for each runtime worker. The driver processes submissions and
completions in bounded calls to [`Driver::execute_cycle`][__link2].

```rust
use std::task::Waker;

use arty_io_core::{
    Cycle, Driver, DriverError, DriverInstance, DriverOptions, DriverRole, IoContext,
};
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

    fn driver(
        &mut self,
        options: DriverOptions,
    ) -> Result<DriverInstance<Self::Driver>, DriverError> {
        let role = options
            .allowed_roles()
            .first()
            .copied()
            .ok_or_else(|| DriverError::from_message("no supported driver role"))?;
        Ok(DriverInstance::new(ExampleDriver, role))
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

A worker has at most one [`Primary`][__link3] driver. [`DriverOptions`][__link4] lists the
roles available during creation, and [`DriverInstance`][__link5] returns the selected role. A primary
may wait on the worker for up to [`Cycle::max_wait`][__link6]; a zero wait bound means no waiting.

[`Secondary`][__link7] drivers must not block the worker. They may schedule
background waits on independent execution capacity.

## Runtime responsibilities

The runtime clones and relocates contexts to its workers and supplies the acceptable roles
through [`DriverOptions`][__link8]. The context selects one of those roles and returns it in a
[`DriverInstance`][__link9]. The runtime completes a non-blocking, zero-wait initialization cycle before
publishing a context.
It also supplies an owner waker for requesting another cycle. Each driver supplies a waker that
interrupts its pending completion wait.

Each logical cycle uses a shared wait bound. The runtime invokes secondaries before the
primary. If there is no primary, the runtime retains responsibility for parking the worker.

## Shutdown

The runtime stops normal cycles and calls [`Driver::shutdown`][__link10] for every driver, continuing
after a [`DriverError`][__link11]. Each driver closes admission and drains its resources within a
bounded wait, independently of other drivers on the same worker. Contexts remain valid as
closed handles.

See the [design notes][__link12] for the lifecycle and ownership model and the [contract decisions][__link13] for
alternatives that were considered and rejected.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_io_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbPmY1dEKTf0obb9lz5Fgy9PUb76OLmX93BysbeR7rFlJXqJlhZIGCbGFydHlfaW9fY29yZWUwLjIuMA
 [__link0]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=IoContext
 [__link1]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver
 [__link10]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::shutdown
 [__link11]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverError
 [__link12]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md
 [__link13]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DECISIONS.md
 [__link2]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::execute_cycle
 [__link3]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverRole::Primary
 [__link4]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverOptions
 [__link5]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverInstance
 [__link6]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Cycle::max_wait
 [__link7]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverRole::Secondary
 [__link8]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverOptions
 [__link9]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverInstance
