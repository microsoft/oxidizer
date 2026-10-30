<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Seismograph Runtime Logo" width="96">

# Seismograph Runtime

[![crate.io](https://img.shields.io/crates/v/seismograph_runtime.svg)](https://crates.io/crates/seismograph_runtime)
[![docs.rs](https://docs.rs/seismograph_runtime/badge.svg)](https://docs.rs/seismograph_runtime)
[![MSRV](https://img.shields.io/crates/msrv/seismograph_runtime)](https://crates.io/crates/seismograph_runtime)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

On-demand runtime diagnostics for [`seismograph`][__link0].

Register runtime and worker metadata only when diagnostics are requested by
the application. Each worker registers a task provider that routes bounded
requests to its native task storage. The provider returns owned descriptions
between execution cycles. There is no separate diagnostic task registry.

Workers check [`worker::WorkerHandle::is_observed`][__link1] once per execution cycle.
Without collection or runtime-event recording, the normal poll loop needs no
diagnostic callbacks, clocks, locks, counters, or reference-count operations.
Allocate task identities and type descriptions only when first observed.

Ordinary Seismograph snapshots request current task state. To measure polls,
call [`RuntimeHandle::capture`][__link2] with a nonzero
[`snapshot::CaptureOptions::measure_for`][__link3]. The diagnostic caller waits for that
interval, not the workers. Private observation windows prevent a poll from a
previous request from entering a new measurement window.

Runtime-event recording is separate from task inspection. It retains only
events emitted after recording starts. Missing creation times, wake ages,
earlier polls, and completed task history are not reconstructed.
Stopping event recording does not freeze the actual runtime’s current state.
[`worker::WorkerHandle::task_events`][__link4] supplies a [`Copy`][__link5] context for task
creation and real outcomes, without per-task registration or shared ownership.

## Example

```rust
use seismograph_runtime::RuntimeMetadata;
use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};

let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("primary", 1));
let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
worker.handle().register_task_provider(|request| {
    // A real runtime moves this request to its worker before inspecting tasks.
    request.respond(
        Vec::new(),
        seismograph_runtime::snapshot::TaskCollection::Complete,
    );
})?;
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seismograph_runtime">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbe_rSi_eciMkbZ9WZwc9JDqMbs5twE1aCioIb5KjnCXEtXnlhZIKCa3NlaXNtb2dyYXBoZTAuMy4wgnNzZWlzbW9ncmFwaF9ydW50aW1lZTAuMy4w
 [__link0]: https://crates.io/crates/seismograph/0.3.0
 [__link1]: https://docs.rs/seismograph_runtime/0.3.0/seismograph_runtime/?search=worker::WorkerHandle::is_observed
 [__link2]: https://docs.rs/seismograph_runtime/0.3.0/seismograph_runtime/?search=RuntimeHandle::capture
 [__link3]: https://docs.rs/seismograph_runtime/0.3.0/seismograph_runtime/?search=snapshot::CaptureOptions::measure_for
 [__link4]: https://docs.rs/seismograph_runtime/0.3.0/seismograph_runtime/?search=worker::WorkerHandle::task_events
 [__link5]: https://doc.rust-lang.org/stable/std/marker/trait.Copy.html
