// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime telemetry and opt-in local diagnostics.
//!
//! # Runtime events and task enrichment
//!
//! [`RuntimeBuilder::sink`](crate::runtime::RuntimeBuilder::sink) configures
//! [`observed`] events and enrichment independently of local monitoring.
//! Tasks capture the configured sink's current enrichment at submission and
//! restore it while executing. Runtime-owned event processors must not panic.
//! Classified runtime fields use `arty` / `SystemMetadata`; panic text uses
//! the separate `arty` / `PanicMessage` class.
//!
//! # Locally accessible diagnostics
//!
//! Enable the independent `seismograph` Cargo feature and opt a runtime in with
//! `MonitorOptions` and the builder's
//! `monitor` setting. `seismograph` does not enable `rt` itself. The default
//! runtime configuration has no diagnostic endpoint or runtime/task registration,
//! even when support is compiled.
//!
//! Arty owns the configuration and endpoint lifecycle. Backend types,
//! diagnostic identities and server handles are not exposed through Arty's
//! public Rust API. The endpoint protocol belongs to diagnostic tooling rather
//! than the Rust API compatibility contract.
//!
//! ```
//! # fn main() {
//! # #[cfg(all(feature = "rt", feature = "seismograph"))] {
//! # (|| {
//! use arty::runtime::{MonitorOptions, Runtime, WorkersPolicy};
//!
//! let runtime = Runtime::builder()
//!     .workers(WorkersPolicy::at_most(2))
//!     .monitor(MonitorOptions::default().name("my-runtime"))
//!     .build()?;
//! runtime.scheduler().block_on(async |_| ())?;
//! runtime.stop()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! # })().unwrap();
//! # }
//! # }
//! ```
//!
//! Each opted-in runtime has a separate authenticated localhost endpoint, with
//! discovery metadata in the current user's diagnostic directory. Building,
//! stopping and dropping one runtime does not start or close another runtime's
//! endpoint. `build` returns endpoint-startup failures; background connection
//! and listener failures are reported to the configured runtime telemetry sink
//! as `arty.rt.monitor.failed`.
//!
//! Monitoring does not install an allocator, start event recording or reset
//! existing process recording policies. The diagnostic tool controls recording.
//! Recorder policies, event buffers and allocator observations remain
//! process-wide: separate endpoints are not isolated recorders, and snapshots
//! can include other registered process sources and runtimes. Clear or Stop
//! commands through one endpoint affect the shared process recorder.
//!
//! The same feature enables primitive hooks for the optional `performables`
//! dependency when that dependency is selected. Actual Arc/lock/channel event
//! collection still requires the corresponding recording policy. Other
//! dependencies can enable these hooks through Cargo feature unification.
//!
//! # On-demand native task inspection
//!
//! Arty does not maintain a second diagnostic live-task registry. Queries
//! request descriptions from each worker's actual executor storage between
//! polls. Collection is bounded and explicitly distinguishes complete,
//! truncated, busy, timed-out, stopped and failed workers.
//!
//! There is no complete history from before observation was requested: outcomes, creation times,
//! parentage, wake-to-poll delays or execution time. Identities and type
//! descriptions are created lazily during observation. Inspect source coverage
//! and measurement scope before interpreting counters; unavailable is not zero.
//! Requested poll timing covers only its observation window. Poll duration is
//! elapsed time, including preemption, not CPU time.
//!
//! While observation is requested, current-task identity remains independently
//! readable even when a worker gets stuck in a poll. A worker already blocked
//! before observation starts cannot reconstruct a retroactive identity.
//! Recorded event streams retain only their configured bounded rings.
//!
//! Callback processing occurs on diagnostic threads, not runtime workers.
//! User callback failures do not become task failures. Dropping a subscription
//! stops new delivery while an already-running callback can finish afterward.
//!
//! # Try the connectable example
//!
//! From this repository, in two terminals on the same operating system:
//!
//! ```text
//! cargo run -p arty --features seismograph --example arty_seismograph -- --seconds 300
//! cargo run -p seismograph_cli -- monitor
//! ```
//!
//! Select `arty-live-tasks`, press Enter, `s` to capture, then `6` for Runtime.
//! Press `s` again to refresh current tasks. The TUI uses explicit captures,
//! not a continuous remote task subscription. Recording starts disabled; use
//! `c` to enable runtime or primitive event collection.
