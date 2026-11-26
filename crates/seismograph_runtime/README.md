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

Runtime diagnostics and task lifecycle reporting for [`seismograph`][__link0].

[`RuntimeBuilder`][__link1] creates a [`Runtime`][__link2]. The runtime owns worker
registrations and creates dedicated [`asynchronous::Worker`][__link3] and
[`blocking::Worker`][__link4] handles.

An asynchronous worker can provide snapshots of tasks in its native task
storage. A blocking worker reports lifecycle events only. Blocking tasks do
not appear in task snapshots.

Both task types use the shared `Task*` event kinds. The enqueue methods
return lightweight tokens when runtime task recording is enabled. The caller
moves each token through queue, execution, poll, and outcome transitions.
Dropping an unfinished token records cancellation.

[`TaskIdentity`][__link5] lets the runtime assign an identity before it records the
task. Event recording retains only events emitted after recording starts.
It does not reconstruct earlier task history.

## Example

```rust
use seismograph_runtime::{Runtime, TaskIdentity, TaskOutcome, WorkerMetadata, WorkerRole};

seismograph::recorder(seismograph::recorder::Configuration {
    runtime_tasks: seismograph::recorder::Policy::all(false),
    ..seismograph::recorder::Configuration::default()
});
let runtime = Runtime::builder("primary").configured_workers(1).build();
let worker = runtime.async_worker(WorkerMetadata::new(WorkerRole::Core));
let identity = TaskIdentity::from_location(std::panic::Location::caller());
let queued = runtime.enqueue_async(None, identity, None, 0, 0).unwrap();
let task = queued.activated(&worker);
let poll = task.poll_started(&worker);
assert!(
    poll.finished(&worker, Some(TaskOutcome::Completed))
        .is_none()
);
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seismograph_runtime">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQb7vIJdZ0I5O8bU0HndxO0ld8b_naw_GxFrb8bcXdL1rgYTxxhZIKCa3NlaXNtb2dyYXBoZTAuNi4wgnNzZWlzbW9ncmFwaF9ydW50aW1lZTAuNi4w
 [__link0]: https://crates.io/crates/seismograph/0.6.0
 [__link1]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/struct.RuntimeBuilder.html
 [__link2]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/struct.Runtime.html
 [__link3]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=asynchronous::Worker
 [__link4]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=blocking::Worker
 [__link5]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=TaskIdentity
