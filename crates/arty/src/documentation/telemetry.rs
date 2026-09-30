// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime events, task enrichment, and data classification.
//!
//! Arty uses [`observed`] for runtime events. The default sink is a noop.
//! Pass an application's configured [`observed::Sink`] to
//! [`RuntimeBuilder::sink`](crate::runtime::RuntimeBuilder::sink) before
//! construction. The task's [`Builtins::sink`](crate::runtime::Builtins::sink)
//! returns that runtime's associated sink.
//!
//! Applications constructing a sink also need a direct dependency on
//! `observed`. This helper accepts that configured sink rather than prescribing
//! a particular processor or exporter, returning a builder for an entry point's
//! `builder` option:
//!
//! ```
//! fn app_builder(sink: observed::Sink) -> arty::runtime::RuntimeBuilder {
//!     arty::runtime::Runtime::builder().sink(sink)
//! }
//! ```
//!
//! # Enrichment and task outcomes
//!
//! Asynchronous submission captures the enrichment active at the submission
//! site. Remote and local tasks restore it while polling; their success and
//! panic events retain that context. This includes tasks submitted through
//! `spawn_anywhere`. Do not assume that an unrelated OS thread or a blocking
//! callback inherits the same asynchronous-task context.
//!
//! A dropped join does not prevent a task's panic event from being emitted.
//! However, an asynchronous task discarded during shutdown need not emit a
//! success or panic event. Do not treat spawn/outcome events as an exactly-once
//! completion accounting protocol. The runtime's `arty.rt.stopped` event
//! is emitted once when shutdown is observed complete, not for each waiter.
//!
//! # Classification and wire names
//!
//! Runtime-classified fields use
//! `data_privacy::DataClass::new("arty", "SystemMetadata")`. Configure a
//! processor's redaction policy using that identifier; Arty does not expose a
//! public `SYSTEM_METADATA` constant. Applications configuring that policy
//! also need `data_privacy`.
//!
//! Event names use the `arty.rt` prefix. Blocking-pool saturation is
//! reported by the `arty.rt.blocking_worker.pool_saturated` event and
//! counter. Pool configuration uses `blocking_worker_pool.mode` and
//! `blocking_worker_pool.max_threads`; blocking-pool OS threads are named
//! `arty-blocking`. Numeric metric values remain unredacted numbers.
//!
//! Opaque Rust thread identifiers are strings under `arty.thread.id`, not the
//! integer-valued OpenTelemetry `thread.id` attribute. Do not parse that string
//! as a portable OS thread identifier.
