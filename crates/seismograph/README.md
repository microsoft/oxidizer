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

High-performance process telemetry with extensible snapshot sources.

Instrumented crates record bounded events while registered sources contribute
point-in-time state to a portable [`snapshot()`][__link0].

Snapshot capture defaults to retaining active-thread buffers and recording
policy. [`snapshot::EventBufferDisposition::Stop`][__link1] captures the retained
events, disables all six recording classes, and releases event-ring storage.
Unlike legacy `Release`, it does not restart recording. Later source/encoding
failures do not undo completed cleanup. [`recorder::clear_event_buffers()`][__link2]
independently empties event rings without copying a snapshot or invoking
sources, preserving recording policies and active-thread allocations.
These operations leave process-lifetime recorder metadata registered.

```rust
use seismograph::recorder::event::{EventClass, EventKind, ObjectId, Record};
use seismograph::recorder::{Configuration, RecordingPolicy};

seismograph::recorder(Configuration {
    arc_dereferences: RecordingPolicy::all(false),
    ..Default::default()
});
seismograph::record(EventClass::ArcDereference, || {
    Some(Record::object(EventKind::ArcDeref, ObjectId::new(42)))
});

let encoded = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
let decoded = seismograph::snapshot::decode(encoded.as_bytes()).unwrap();
assert!(
    decoded
        .events
        .events
        .iter()
        .any(|event| event.object_id() == Some(ObjectId::new(42)))
);
```

Applications built with the `monitor` feature can publish a localhost
endpoint for the `seismograph monitor` TUI. Keep the returned monitor alive
for as long as remote control should remain available:

```rust
let _monitor = seismograph::monitor::Monitor::builder()
    .name("worker")
    .instance("west-europe")
    .start()
    .unwrap();
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seismograph">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbnIjY07yDBCMb5dOQTY4yGVUbIqi6d2b0sygbtiUL_gUfWH9hZIGCa3NlaXNtb2dyYXBoZTAuMS4w
 [__link0]: https://docs.rs/seismograph/0.1.0/seismograph/fn.snapshot.html
 [__link1]: https://docs.rs/seismograph/0.1.0/seismograph/?search=snapshot::EventBufferDisposition::Stop
 [__link2]: https://docs.rs/seismograph/0.1.0/seismograph/?search=recorder::clear_event_buffers
