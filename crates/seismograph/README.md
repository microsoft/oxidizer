<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Seismograph Logo" width="96">

# Seismograph

[![crate.io](https://img.shields.io/crates/v/seismograph.svg)](https://crates.io/crates/seismograph)
[![docs.rs](https://docs.rs/seismograph/badge.svg)](https://docs.rs/seismograph)
[![MSRV](https://img.shields.io/crates/msrv/seismograph)](https://crates.io/crates/seismograph)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

High-performance process telemetry with extensible snapshot sources.

Instrumented crates record bounded events while registered sources contribute
point-in-time state to a portable [`snapshot()`][__link0].

```rust
use seismograph::recorder::event::{EventClass, EventKind, ObjectId, Record};
use seismograph::recorder::{Configuration, RecordingPolicy};

seismograph::recorder(Configuration {
    arc_dereferences: RecordingPolicy::all(false),
    ..Default::default()
});
seismograph::record(EventClass::ArcDereference, || {
    Record::object(EventKind::ArcDeref, ObjectId::new(42))
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

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEG9dVcQv7gDzkG7VJ-FsdvgXwG4ndzbdWNuz6G6a5_GehYxcvYXKEG5GAxQmVXmOAGwbYoFgg97cqG3iIALZ4J988G2Y5o21O-lQ_YWSBgmtzZWlzbW9ncmFwaGUwLjEuMA
 [__link0]: https://docs.rs/seismograph/0.1.0/seismograph/fn.snapshot.html
