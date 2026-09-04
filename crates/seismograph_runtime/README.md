<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Seismograph Runtime Logo" width="96">

# Seismograph Runtime

[![crate.io](https://img.shields.io/crates/v/seismograph_runtime.svg)](https://crates.io/crates/seismograph_runtime)
[![docs.rs](https://docs.rs/seismograph_runtime/badge.svg)](https://docs.rs/seismograph_runtime)
[![MSRV](https://img.shields.io/crates/msrv/seismograph_runtime)](https://crates.io/crates/seismograph_runtime)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Process-wide multi-runtime telemetry for [`seismograph`][__link0].

One static source describes every logical runtime in the process. Runtime
and worker registrations own stable control blocks and retire them
logically, so a concurrent snapshot never dereferences reclaimed metadata.
Retired records are intentionally retained for process lifetime in this
first schema, preserving metadata for every event still present in any
recording session.

## Compatibility

The registered source has stable ID [`snapshot::source::ID`][__link1] and the name
`runtime`. Its private framing and public schema are independently
versioned. [`snapshot::decode`][__link2] rejects unknown future versions rather than
silently interpreting them as the current layout.

Hot-path task, poll, transfer, and I/O methods update atomics and write the
calling thread’s bounded Seismograph ring without formatting or allocation.

```rust
use seismograph_runtime::RuntimeMetadata;
use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};

let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("primary", 1));
let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
worker.attach_current_thread();
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seismograph_runtime">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEG9dVcQv7gDzkG7VJ-FsdvgXwG4ndzbdWNuz6G6a5_GehYxcvYXKEGxrgq-FeaBG5G1OvMD96rlnWG05j90Pu2h-JG7XJzqoihHN_YWSCgmtzZWlzbW9ncmFwaGUwLjEuMIJzc2Vpc21vZ3JhcGhfcnVudGltZWUwLjEuMA
 [__link0]: https://crates.io/crates/seismograph/0.1.0
 [__link1]: https://docs.rs/seismograph_runtime/0.1.0/seismograph_runtime/?search=snapshot::source::ID
 [__link2]: https://docs.rs/seismograph_runtime/0.1.0/seismograph_runtime/?search=snapshot::decode
