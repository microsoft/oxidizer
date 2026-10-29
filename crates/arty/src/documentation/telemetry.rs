// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime telemetry through [`observed`].
//!
//! Arty emits runtime events through an [`observed::Sink`]. The default is
//! [`Sink::noop`](observed::Sink::noop), which disables events and enrichment.
//! Supply a regular sink with [`RuntimeBuilder::sink`](crate::runtime::RuntimeBuilder::sink)
//! to enable enrichment; configure its processors to receive events.
//! Tasks can access it through
//! [`Context::sink`](crate::task::Context::sink).
//! Configure processors that do not panic: event delivery is synchronous and
//! Arty does not recover from telemetry-processor panics.
//!
//! Applications configuring a sink or emitting their own events need a direct
//! dependency on [`observed`]. See its documentation for event definitions,
//! enrichment, processing, and redaction.
//!
//! With a regular sink configured, async tasks inherit the enrichment active
//! when submitted and restore it while polling. Blocking callbacks and unrelated
//! threads do not inherit that context automatically. A task cancelled at shutdown
//! need not emit an outcome event, so task events are not an exactly-once completion
//! record.
//!
//! Routine classified runtime fields use the `arty` / `SystemMetadata`
//! identifier when configuring redaction. Panic diagnostics use the separate
//! `arty` / `PanicMessage` identifier, so configure both classes when panic
//! text should remain visible.
