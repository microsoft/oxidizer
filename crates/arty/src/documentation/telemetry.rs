// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime telemetry through [`observed`].
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
//! Routine classified runtime fields use the `arty` / `SystemMetadata`
//! identifier when configuring redaction. Panic diagnostics use the separate
//! `arty` / `PanicMessage` identifier, so configure both classes when panic
//! text should remain visible.
