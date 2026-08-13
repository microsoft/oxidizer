// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Interoperability with foreign telemetry sources.
//!
//! This module is the bridge between `observed` and events that originate from
//! other telemetry crates (such as `tracing` or `log`). Those events cannot
//! implement the statically-typed [`Event`](crate::Event) trait, so they are
//! adapted via the type-erased [`DynEvent`] trait and dispatched through
//! [`emit_dyn_event`].
//!
//! Unlike the [`emit!`](crate::emit!) macro - which lazily builds a concrete
//! `T: Event` and only evaluates it when a processor is interested -
//! [`emit_dyn_event`] takes an already-built `&dyn DynEvent` and dispatches it
//! directly through the sink's normal pipeline.
//!
//! # Privacy
//!
//! Field values still travel through each processor's redaction engine, exactly
//! as they do for typed events. The event *body* does not: see
//! [`DynEvent::body`] for the contract an adaptor has to uphold.

use std::borrow::Cow;
use std::ops::ControlFlow;

use crate::Sink;
use crate::metadata::EventDescription;
use crate::processing::{FieldVisitorFn, IntermediateEvent};

/// Adaptor for foreign event types that cannot implement [`Event`](crate::Event) directly.
pub trait DynEvent: Send + Sync {
    /// The event name.
    ///
    /// Compile-time events return `&'static str` (zero-copy).
    /// Dynamic events may return `&'static str` for runtime names.
    ///
    /// Limitation to `&'static str` comes from `LogRecord::set_event_name` in `OTel` SDK.
    fn name(&self) -> &'static str;

    /// An optional human-readable message body.
    ///
    /// # Trusted, pre-sanitized boundary
    ///
    /// Unlike field values, the body is **not** passed through a processor's
    /// [`RedactionEngine`](data_privacy::RedactionEngine): processors export it
    /// verbatim, so an erasing or replacing engine does not touch it. A typed
    /// event's body is a `&'static str` message template and is therefore safe
    /// by construction; this method widens that to `Cow<'static, str>` so a
    /// bridge can carry a formatted message.
    ///
    /// An adaptor must therefore return only text it has already sanitized -
    /// typically a message template or a value it produced itself. Untrusted
    /// runtime input, user-controlled text, PII, and secrets must instead be
    /// reported through [`visit_fields`](Self::visit_fields), where each value
    /// carries a data class and is redacted per processor.
    fn body(&self) -> Option<Cow<'static, str>>;

    /// The source file where the event originated, if available.
    fn source_file(&self) -> Option<Cow<'static, str>>;

    /// The source line where the event originated, if available.
    fn source_line(&self) -> Option<u32>;

    /// The name of the crate where the event originated, if available.
    fn source_crate(&self) -> Option<Cow<'static, str>>;

    /// Lazily visits all key-value fields on this event.
    ///
    /// For each field, the visitor receives a [`FieldDescriptor`](crate::metadata::FieldDescriptor) and a getter
    /// closure. The getter takes a `&RedactionEngine` and returns the redacted
    /// [`Value`](crate::Value). It is only invoked if the processor wants the value.
    ///
    /// The visitor returns [`ControlFlow::Continue`] to keep iterating or
    /// [`ControlFlow::Break`] to stop early.
    fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()>;

    /// Returns the event description (metadata) for processor prefiltering.
    ///
    /// Compile-time events return `T::DESCRIPTION` (static metadata).
    /// Dynamic events construct an `EventDescription` from runtime values.
    ///
    /// This must carry the event's complete signal metadata: a processor
    /// selects on it alone, so a signal omitted here means the event never
    /// reaches the processors that handle it. The severity a log destination
    /// exports is read from the
    /// [`LogDescription`](crate::metadata::LogDescription) returned here, so
    /// this one statement decides both routing and the exported level.
    fn description(&self) -> EventDescription;
}

/// Emits a pre-constructed [`DynEvent`] through the given [`Sink`].
///
/// This is the runtime, type-erased counterpart to [`emit!`](crate::emit!). Whereas
/// `emit!` lazily builds a statically-typed `T: Event` and only evaluates it when a processor is
/// interested, this function takes an already-built `&dyn DynEvent` and dispatches it directly.
///
/// It is intended for **interoperability with other telemetry crates** (such as `tracing` or
/// `log` bridges) where events originate from a foreign type that cannot implement the
/// [`Event`](crate::Event) trait, and are instead adapted via [`DynEvent`].
///
/// The event still passes through the sink's normal pipeline. If the sink is a no-op or no
/// processor is interested, the event is dropped without further work.
pub fn emit_dyn_event(sink: &Sink, event: &dyn DynEvent) {
    sink.emit_impl(IntermediateEvent::dynamic(event));
}
