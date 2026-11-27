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

Both task types use the shared `Task*` event kinds. Task methods return
lightweight handles when runtime task recording is enabled. The caller
records queue insertion and moves each token through execution, poll, and
outcome transitions. Dropping an unfinished queued token records cancellation.

[`TaskIdentity`][__link5] lets the runtime assign an identity before it records the
task. Event recording retains only events emitted after recording starts.
[`model::RuntimeModel`][__link6] reconstructs runtime, worker, and task state from the
retained event ring. A task that predates recording appears only after it
emits a retained event, such as a later poll.

## Runtime integration

Create one [`Runtime`][__link7] during runtime startup. Keep it until all runtime
workers stop. Create it even when the application does not start a
Seismograph monitor connection.

Create one [`asynchronous::Worker`][__link8] on each asynchronous worker thread.
Create one [`blocking::Worker`][__link9] on each blocking worker thread. Store each
worker in thread-local runtime state and borrow it for lifecycle callbacks.
Do not clone a worker for each callback.

When an asynchronous task is spawned, call [`Runtime::async_task`][__link10] with its
origin worker, identity, parent, and concrete body size. The method returns
`None` when runtime-task recording is disabled. If it returns a handle, move
the handle with the task and call [`asynchronous::TaskHandle::enqueued`][__link11] when
the task enters the executor queue.

Pass the resulting optional [`asynchronous::QueuedTask`][__link12] into the executor.
On the first poll, call [`asynchronous::QueuedTask::activated`][__link13] and then
[`asynchronous::Task::poll_started`][__link14]. After every poll, call
[`asynchronous::Poll::finished`][__link15] with `None` for a pending poll or with the
terminal outcome.

If a task has no queued handle, keep a dormant executor state. When a later
poll starts, create an active token with [`asynchronous::Task::unrecorded`][__link16].
This records the new poll without inventing queue or activation events.
Read [`asynchronous::Worker::is_recording`][__link17] once at the start of an executor
cycle. Select a recorded or silent poll loop for the full cycle. The silent
loop must transform task state without recorder checks or clock reads.

For blocking work, call [`Runtime::blocking_task`][__link18] at submission. If it
returns a handle, call [`blocking::TaskHandle::enqueued`][__link19] while the queue
lock supplies the queue length. Call [`blocking::QueuedTask::started`][__link20] when
a worker removes the job, and call [`blocking::Execution::finished`][__link21] after
the function returns or panics.

Call [`asynchronous::Worker::parked`][__link22] before an asynchronous worker waits,
and call [`asynchronous::Worker::unparked`][__link23] after it wakes. Apply the same
rule to blocking workers. Call [`Runtime::stopping`][__link24] when shutdown starts,
and call [`Runtime::stop`][__link25] after all workers stop.

Keep the disabled path short. Runtime and worker handles are always present,
but task handles are optional. Do not add a task registry, provider callback,
mutex, clock read, or shared-reference clone to the disabled task path.

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
let queued = runtime.async_task(identity, 0).unwrap().enqueued(0);
let task = queued.activated(&worker);
let poll = task.poll_started(&worker);
assert!(poll.finished(Some(TaskOutcome::Completed)).is_none());
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seismograph_runtime">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbcnmvNz2MIpkbgcguB6q4mzQbrNCHFUuFFlwbGY9L3pu-8cVhZIKCa3NlaXNtb2dyYXBoZTAuNi4wgnNzZWlzbW9ncmFwaF9ydW50aW1lZTAuNi4w
 [__link0]: https://crates.io/crates/seismograph/0.6.0
 [__link1]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=RuntimeBuilder
 [__link10]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=Runtime::async_task
 [__link11]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=asynchronous::TaskHandle::enqueued
 [__link12]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=asynchronous::QueuedTask
 [__link13]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=asynchronous::QueuedTask::activated
 [__link14]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=asynchronous::Task::poll_started
 [__link15]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=asynchronous::Poll::finished
 [__link16]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=asynchronous::Task::unrecorded
 [__link17]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=asynchronous::Worker::is_recording
 [__link18]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=Runtime::blocking_task
 [__link19]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=blocking::TaskHandle::enqueued
 [__link2]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=Runtime
 [__link20]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=blocking::QueuedTask::started
 [__link21]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=blocking::Execution::finished
 [__link22]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=asynchronous::Worker::parked
 [__link23]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=asynchronous::Worker::unparked
 [__link24]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=Runtime::stopping
 [__link25]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=Runtime::stop
 [__link3]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=asynchronous::Worker
 [__link4]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=blocking::Worker
 [__link5]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=TaskIdentity
 [__link6]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=model::RuntimeModel
 [__link7]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=Runtime
 [__link8]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=asynchronous::Worker
 [__link9]: https://docs.rs/seismograph_runtime/0.6.0/seismograph_runtime/?search=blocking::Worker
