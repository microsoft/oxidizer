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
use seismograph::recorder::{Configuration, RecordingPolicy};

seismograph::recorder(Configuration {
    general_events: RecordingPolicy::all(false),
    ..Default::default()
});

// Run application code that uses instrumented libraries.
let snapshot = seismograph::snapshot(Default::default())?;
```

Inspect saved snapshots with `seismograph view` from the
[Seismograph CLI][__link0].

## Live monitoring

Applications built with the `monitor` feature can publish an OS-authorized local
endpoint for the `seismograph monitor` TUI. Keep the returned monitor alive
for as long as local control should remain available. The owning user and
root/elevated administrators can connect without a token. Startup creates
the user’s private registry automatically; no administrator setup is needed.
Unix prefers `$XDG_RUNTIME_DIR/seismograph`, with a private randomized `/tmp`
fallback. Windows uses the user’s local application-data directory.
Pass a `seismograph_protocol::io::MonitorIo` to `Builder::io` to select
the monitor’s identity, transport, discovery, persistence, and clock
capability. Omitting it preserves native I/O; `seismograph_protocol`’s
`test-util` feature provides an isolated in-memory implementation for
application tests without host registry setup.

```rust
let _monitor = seismograph::monitor::Monitor::builder()
    .name("worker")
    .instance("west-europe")
    .start()
    .unwrap();
```

## Features

* **`monitor`**: Enables the OS-authorized local monitor server.
* **`test-util`**: Enables `monitor`, the protocol’s mock I/O controls, and
  `monitor::Builder::isolated` for native tests using a temporary registry.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seismograph">source code</a>.
</sub>

 [__link0]: https://crates.io/crates/seismograph_cli
