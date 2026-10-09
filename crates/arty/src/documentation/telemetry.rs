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
//! Arty also registers every runtime, async worker, and async task with
//! [`seismograph`]. Seismograph records worker/thread association, placement,
//! materialization, poll duration, and exactly one completed, panicked, or
//! cancelled terminal state. Enable its `runtime_tasks` recording policy for
//! high-frequency task events; runtime metadata and counters remain available
//! in snapshots when event recording is disabled.
//!
//! Applications configuring recording or capturing snapshots need a direct
//! dependency on `seismograph`:
//!
//! ```sh
//! cargo add seismograph
//! ```
//!
//! Enable runtime-task events before starting runtimes, then capture and decode
//! a process snapshot:
//!
//! ```
//! use arty::runtime::{Runtime, WorkersPolicy};
//! use seismograph::recorder::{Configuration, RecordingPolicy};
//! use seismograph::snapshot::SnapshotOptions;
//!
//! seismograph::recorder(Configuration {
//!     runtime_tasks: RecordingPolicy::all(false),
//!     ..Configuration::default()
//! });
//! let runtime = Runtime::builder()
//!     .workers(WorkersPolicy::exactly(1))
//!     .build()?;
//! runtime.scheduler().block_on(async |_| ())?;
//! runtime.stop()?;
//!
//! let snapshot = seismograph::snapshot(SnapshotOptions::default())?;
//! let decoded = seismograph::snapshot::decode(snapshot.as_bytes())?;
//! assert!(!decoded.sources.is_empty());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Routine classified runtime fields use the `arty` / `SystemMetadata`
//! identifier when configuring redaction. Panic diagnostics use the separate
//! `arty` / `PanicMessage` identifier, so configure both classes when panic
//! text should remain visible.
