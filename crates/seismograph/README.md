<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Seismograph Logo" width="96">

# Seismograph

[![crate.io](https://img.shields.io/crates/v/seismograph.svg)](https://crates.io/crates/seismograph)
[![docs.rs](https://docs.rs/seismograph/badge.svg)](https://docs.rs/seismograph)
[![MSRV](https://img.shields.io/crates/msrv/seismograph)](https://crates.io/crates/seismograph)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Runtime telemetry for Rust applications.

`seismograph` collects allocation, synchronization, I/O, cache, and task events
to help investigate memory use, contention, and scheduling. Crates such as
`rallocator` and `performables` provide instrumentation.

Recording is off by default, with sampling and optional backtraces to control
its cost. Events are kept in bounded per-thread buffers. Snapshots combine
those events with state supplied by registered sources, such as allocators
and runtimes.

## Usage

Enable the event categories you need, run your workload, then capture a
snapshot. For ownership and synchronization events:

```rust
use seismograph::recorder::event::{EventClass, EventKind, ObjectId, Record};
use seismograph::recorder::{Configuration, Policy};

seismograph::recorder(Configuration {
    general_events: Policy::all(false),
    ..Default::default()
});

// Run application code that uses instrumented libraries.
// To emit an event directly:
seismograph::record(EventClass::General, || {
    Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
});
let snapshot = seismograph::snapshot(Default::default())?;
```

Inspect saved snapshots with `seismograph view` from the
[Seismograph CLI][__link0].

## Live monitoring

Enable `monitor`, call `seismograph::monitor::Monitor::builder().start()?`, and keep the returned handle alive.
Run `seismograph monitor` to connect.

## Features

* **`monitor`**: Enables the OS-authorized local monitor server.
* **`test-util`**: Enables `monitor`, the protocol’s mock I/O controls, and
  `monitor::Builder::isolated` for native tests using a temporary registry.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seismograph">source code</a>.
</sub>

 [__link0]: https://crates.io/crates/seismograph_cli
