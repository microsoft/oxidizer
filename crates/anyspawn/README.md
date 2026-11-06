<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Anyspawn Logo" width="96">

# Anyspawn

[![crate.io](https://img.shields.io/crates/v/anyspawn.svg)](https://crates.io/crates/anyspawn)
[![docs.rs](https://docs.rs/anyspawn/badge.svg)](https://docs.rs/anyspawn)
[![MSRV](https://img.shields.io/crates/msrv/anyspawn)](https://crates.io/crates/anyspawn)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

A task spawner for Arty and Tokio.

[`Spawner`][__link0] provides the same task-spawning API for both runtimes without
runtime-specific type parameters. Enable `arty` or `tokio` to construct a
spawner. Both features can be enabled together.
Without either feature, the public types remain available for adapter crates,
but no spawner can be constructed.

## Quick Start

### Using Arty

```rust
use anyspawn::Spawner;
use arty::runtime::Runtime;

let runtime = Runtime::new()?;
let spawner = Spawner::new_arty(cx.scheduler().clone());
let result = spawner.spawn(async { 1 + 1 }).await?;
assert_eq!(result, 2);
runtime.stop()?;
```

### Using Tokio

```rust
use anyspawn::Spawner;

let spawner = Spawner::new_tokio(None);
let result = spawner.spawn(async { 1 + 1 }).await?;
assert_eq!(result, 2);
```

`Spawner::new_tokio` accepts a runtime handle, `Some(handle)`, or `None`.
`None` selects the runtime active at each spawn call.
`Spawner::new_arty` takes ownership of a scheduler.
These constructors do not enable enrichment transfer.
Use `Spawner::builder_tokio` or `Spawner::builder_arty` to configure
a spawner through [`SpawnerBuilder`][__link1].

Awaiting a [`JoinHandle`][__link2] returns `Result<T, JoinError>`. A task panic,
cancellation, or shutdown rejection becomes an error rather than another panic
in the joining task. The runtime’s panic policy still applies.
A task returning `Result<T, E>` produces `Result<Result<T, E>, JoinError>`.

## Thread-Aware Support

`Spawner` implements [`ThreadAware`][__link3].
Arty spawners follow their scheduler’s worker association during relocation.
Tokio spawners ignore relocation and keep the same runtime selection.

## Observed Enrichment

Enrichment is context attached to events emitted through an `observed::Sink`.
With the `observed` feature and a runtime feature, call `observed(sink)`
on the [`SpawnerBuilder`][__link4] to transfer context to spawned work.
Each spawn call captures the supplied sink’s context. Async tasks restore it
on each poll, and blocking tasks restore it for the closure call.

`spawn_anywhere` also restores context while its factory creates the future.
Arty still relocates the task data before calling the factory.
Spawners without a sink do not transfer context, even when `observed` is enabled.

```rust
use anyspawn::Spawner;
use observed::enrichment::EnrichFutureExt;
use observed::{Enrichment, Sink, event};

#[derive(Clone, Debug, Enrichment)]
struct Request {
    #[unredacted]
    request_id: u64,
}

#[event("task.started")]
#[info]
struct TaskStarted;

let sink = Sink::builder("app").build();
let spawner = Spawner::builder_tokio(None).observed(sink.clone()).build();
async {
    // Events emitted through this sink in the task include request_id.
    let task_sink = sink.clone();
    spawner
        .spawn(async move {
            observed::emit!(&task_sink, TaskStarted);
        })
        .await?;
    Ok::<(), anyspawn::JoinError>(())
}
.enrich(&sink, Request { request_id: 42 })
.await?;
```

## Features

* `arty`: Enables the Arty constructors on `Spawner`.
* `tokio`: Enables the Tokio constructors on `Spawner`.
* `observed`: Enables `SpawnerBuilder::observed`, which accepts an `observed::Sink`.
  Combine it with `arty` or `tokio`. It does not select a runtime.
* `seismograph`: Enables instrumentation in the optional `observed` dependency.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/anyspawn">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbV11enGy8VMMblwkYI5s6sAkbWzjDqALeRxQbRxHPnBrMLOphZIKCaGFueXNwYXduZjAuMTAuMIJsdGhyZWFkX2F3YXJlZjAuMTIuMg
 [__link0]: https://docs.rs/anyspawn/0.10.0/anyspawn/?search=Spawner
 [__link1]: https://docs.rs/anyspawn/0.10.0/anyspawn/?search=SpawnerBuilder
 [__link2]: https://docs.rs/anyspawn/0.10.0/anyspawn/?search=JoinHandle
 [__link3]: https://docs.rs/thread_aware/0.12.2/thread_aware/?search=ThreadAware
 [__link4]: https://docs.rs/anyspawn/0.10.0/anyspawn/?search=SpawnerBuilder
