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

With runtime recording disabled, the [`task::TaskHandle`][__link3] wake and poll hooks
add one relaxed policy check for recording-only activity. That check skips all
activity-state mutation, additional clock reads, task locking, and ring lookup;
the existing always-on timestamps, atomics, counters, and event gates remain.
This hot-path bound does not cover registration or terminal callbacks.
Registration initializes inline activity storage in the existing task control
allocation even when disabled, increasing its footprint. Terminal callbacks
perform one unconditional terminal-marker store per retired task: retained
wakers must remain terminal if recording is enabled again later. Identity-only
registration initializes activity as Unknown without an invalidation operation.
Enabled activity uses a non-blocking task-state lock and session checks before
writing the caller’s bounded ring. The recorder may allocate its thread ring
on first use; steady-state hooks do not format or allocate.

## Recording

Linking this crate does not instrument a runtime automatically. The runtime
must register itself and its workers and call the task instrumentation APIs.
Registration makes runtime metadata and counters available in snapshots even
when event recording is disabled. Event recording additionally requires
[`seismograph::recorder::Configuration::runtime_tasks`][__link4] to be enabled; enabling
general events alone does not enable runtime events.

Recording can be enabled after runtimes and tasks have started. Subsequent
events are recorded, but earlier lifecycle events are not replayed. Runtime
metadata in the snapshot still describes those pre-existing registrations.

[`task::TaskHandle::woken`][__link5] emits one [`EventKind::TaskReady`][__link6] for the first
outstanding notification. Its event thread is the notifier, not a worker
assignment. Initial scheduling must call `woken`; the legacy `task_enqueued`
event is separate and does not emit another readiness notification.

[`snapshot::TaskActivity`][__link7] reports coherent outstanding Ready/Running ages.
Its `poll_worker_id` belongs to that exact Running poll, even when the poll’s
start event was overwritten or independently sampled worker slots are stale.
A wake while Running requests another poll: its raw time is `ready_since`,
while `queued_since` starts when the current poll finishes. Use the activity’s
`observed_at` as the age boundary; Stop freezes it. New recording generations
(including Clear/reset) invalidate old evidence. Existing tasks start Unknown
until observed, and contention conservatively returns Unknown instead of
blocking wakers or exposing torn state. Lifetime counters remain independent
of recording and must not be interpreted as generation-local observations.
Recorded [`EventKind::TaskPollStarted`][__link8] events carry completed scheduler queue
waits in `value_0` (nanoseconds) with `value_1 == 2`. These coherent samples
exclude time in a preceding poll even if its finish event was overwritten.
`value_1 == 0` means no sample; historical `value_1 == 1` remains raw
wake-to-poll latency and must not be interpreted as a scheduler queue sample.

Task sizes describe the concrete future (or synchronous task body) before
runtime wrapping, excluding separately allocated buffers and executor storage.
Sized registration retains this metadata even without recording. Spawn events
preserve it for completed tasks: `value_1 == 0` means unknown, otherwise
`value_1 - 1` is the size in bytes, including zero-byte futures. Older source
schemas and unsized registrations have no size metadata.

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

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbzxsp_ugqZ1EbMoZJ-qfjPcMbNYlF4Li19U8bnB1ozm4LohZhZIKCa3NlaXNtb2dyYXBoZTAuMi4wgnNzZWlzbW9ncmFwaF9ydW50aW1lZTAuMi4w
 [__link0]: https://crates.io/crates/seismograph/0.2.0
 [__link1]: https://docs.rs/seismograph_runtime/0.2.0/seismograph_runtime/?search=snapshot::source::ID
 [__link2]: https://docs.rs/seismograph_runtime/0.2.0/seismograph_runtime/?search=snapshot::decode
 [__link3]: https://docs.rs/seismograph_runtime/0.2.0/seismograph_runtime/?search=task::TaskHandle
 [__link4]: https://docs.rs/seismograph/0.2.0/seismograph/?search=recorder::Configuration::runtime_tasks
 [__link5]: https://docs.rs/seismograph_runtime/0.2.0/seismograph_runtime/?search=task::TaskHandle::woken
 [__link6]: https://docs.rs/seismograph/0.2.0/seismograph/?search=recorder::event::EventKind::TaskReady
 [__link7]: https://docs.rs/seismograph_runtime/0.2.0/seismograph_runtime/?search=snapshot::TaskActivity
 [__link8]: https://docs.rs/seismograph/0.2.0/seismograph/?search=recorder::event::EventKind::TaskPollStarted
