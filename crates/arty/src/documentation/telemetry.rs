// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime telemetry through [`observed`] and [`seismograph`].
//!
//! Arty emits runtime events through an [`observed::Sink`]. The default sink is
//! a no-op; supply one with [`RuntimeBuilder::sink`](crate::runtime::RuntimeBuilder::sink)
//! before starting the runtime. Tasks can access it through
//! [`Builtins::sink`](crate::task::Builtins::sink).
//! Configure processors that do not panic: event delivery is synchronous and
//! Arty does not recover from telemetry-processor panics.
//!
//! Applications configuring a sink or emitting their own events need a direct
//! dependency on [`observed`]. See its documentation for event definitions,
//! enrichment, processing, and redaction.
//!
//! Async tasks inherit the enrichment active when submitted and restore it
//! while polling. Blocking callbacks and unrelated threads do not inherit that
//! context automatically. A task cancelled at shutdown need not emit an outcome
//! event, so task events are not an exactly-once completion record.
//!
//! Enable Arty's `seismograph` Cargo feature to register runtimes and workers and
//! enable `performables` primitive hooks together. This feature is off by default;
//! the plain `rt` build has no Seismograph integration.
//! Arty's enabled integration does not maintain
//! a second diagnostic task list. Snapshots request descriptions from each worker's
//! actual executor storage between polls. Inspection is bounded; unavailable or
//! truncated results are reported explicitly. While observation is requested, a
//! worker's current-task identity remains readable even if it gets stuck inside a
//! poll. A worker that was already stuck before monitoring started cannot report
//! a retroactive current-task identity.
//!
//! With no monitoring or recording, tasks use zero-sized instrumentation. Submission
//! does not allocate diagnostic identities, resolve task types, update task counters,
//! or retain diagnostic runtime/worker handles. Task poll callbacks compile away.
//! Runtime registration remains available; one recording-policy check on submission
//! and one observer-demand check per worker cycle support dynamic activation.
//! Identities and type descriptions are constructed only when a task is inspected
//! or observed. There is no complete history of task outcomes or execution time:
//! inspect `counters_scope` before interpreting source counters.
//!
//! Enable Seismograph's `runtime_tasks` policy for requested task-event recording,
//! or request poll timing through a live subscription.
//! Measurements cover the observation window, not execution before collection.
//! Live descriptions do not retain creation times; recorded spawn events have
//! their own timestamps. Live inspection does not collect wake-to-poll timing
//! or spawn backtraces retroactively.
//! Primitive Arc/lock/channel recording uses this same `seismograph` feature
//! and the corresponding recording policy; there is no separate primitive feature.
//! Other dependencies can also enable those primitive hooks through Cargo
//! feature unification.
//! Existing tasks remain available for inspection when a monitor connects after they were
//! submitted. Polls observed after activation can be measured, but earlier spawn
//! events and parent identities are not reconstructed.
//!
//! Applications configuring recording and decoding runtime diagnostics need
//! direct dependencies on `seismograph` and `seismograph_runtime`:
//!
//! ```sh
//! cargo add seismograph seismograph_runtime
//! ```
//!
//! To try live inspection from this repository, run
//! `cargo run -p arty --features seismograph --example arty_seismograph -- --seconds 300`.
//! In another terminal on the same operating system, run
//! `cargo run -p seismograph_cli -- monitor`, select `arty-live-tasks`, then
//! press `s` to capture and `6` for Runtime. Recording starts disabled; current
//! tasks come from callback-based inspection. Press `s` again to refresh the
//! task list. The TUI uses explicit captures, not a continuous task subscription.
//!
//! Subscribe to current task descriptions without enabling event recording:
//!
//! ```
//! # fn main() {
//! # #[cfg(feature = "seismograph")] {
//! # (|| {
//! use arty::runtime::{Runtime, WorkersPolicy};
//! use seismograph::recorder::runtime::RuntimeId;
//! use seismograph_runtime::live::{SubscriptionOptions, subscribe};
//!
//! let runtime = Runtime::builder()
//!     .workers(WorkersPolicy::exactly(1))
//!     .build()?;
//! let id = RuntimeId::from_raw(runtime.id().get())
//!     .expect("Arty runtime identities are nonzero Seismograph identities");
//! let subscription = subscribe(id, SubscriptionOptions::default(), |batch| {
//!     println!(
//!         "worker {:?}: {:?}, {} tasks",
//!         batch.worker_id,
//!         batch.collection,
//!         batch.tasks.len()
//!     );
//! })?;
//! runtime.scheduler().block_on(async |_| ())?;
//! drop(subscription);
//! runtime.stop()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! # })().unwrap();
//! # }
//! # }
//! ```
//!
//! Callbacks run on a diagnostic thread, not on workers. Dropping the subscription
//! stops new delivery; an already-running callback may finish afterward. Set
//! `SubscriptionOptions::measurements` to `Measurements::PollTiming` to collect
//! execution measurements while subscribed. Inspect each task's `metrics_scope`
//! before interpreting its numeric fields. Poll durations are elapsed time inside
//! completed executor task polls, including result delivery and task cleanup.
//! They are not CPU time; operating-system preemption is included.
//!
//! Enable runtime-task events before starting runtimes, then capture and decode
//! a process snapshot:
//!
//! ```
//! # fn main() {
//! # #[cfg(feature = "seismograph")] {
//! # (|| {
//! use arty::runtime::{Runtime, WorkersPolicy};
//! use seismograph::recorder::{Configuration, RecordingPolicy};
//! use seismograph::snapshot::SnapshotOptions;
//! use seismograph_runtime::snapshot::source;
//!
//! seismograph::recorder(Configuration {
//!     runtime_tasks: RecordingPolicy::all(false),
//!     ..Configuration::default()
//! });
//! let runtime = Runtime::builder()
//!     .workers(WorkersPolicy::exactly(1))
//!     .build()?;
//! let runtime_id = runtime.id();
//! runtime.scheduler().block_on(async |_| ())?;
//! runtime.stop()?;
//!
//! let snapshot = seismograph::snapshot(SnapshotOptions::default())?;
//! let decoded = seismograph::snapshot::decode(snapshot.as_bytes())?;
//! let runtime_source = decoded
//!     .sources
//!     .iter()
//!     .find(|entry| entry.id == source::ID)
//!     .expect("Arty registers the runtime diagnostics source");
//! let diagnostics = seismograph_runtime::snapshot::decode(&runtime_source.data)?;
//! let runtime_diagnostics = diagnostics
//!     .runtimes
//!     .iter()
//!     .find(|entry| entry.id.get() == runtime_id.get())
//!     .expect("the public Arty identity matches its retained diagnostics");
//! assert_eq!(runtime_diagnostics.name, "arty");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! # })().unwrap();
//! # }
//! # }
//! ```
//!
//! Routine classified runtime fields use the `arty` / `SystemMetadata`
//! identifier when configuring redaction. Panic diagnostics use the separate
//! `arty` / `PanicMessage` identifier, so configure both classes when panic
//! text should remain visible.
