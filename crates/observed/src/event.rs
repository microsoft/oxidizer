// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The core `Event` trait implemented by all telemetry event types.

use std::ops::ControlFlow;

use crate::metadata::EventDescription;
use crate::processing::FieldVisitorFn;

/// A structured telemetry event.
///
/// Every event type implements this trait - typically via the `#[event(...)]`
/// attribute macro. Default and `#[data_class(<expr>)]` fields are extracted
/// through the processor-supplied redactor. Fields marked `#[unredacted]` and
/// unclassified enrichment values bypass it and must already be appropriate to
/// emit. Fields supplied by a [`DynEvent`](crate::interop::DynEvent) adaptor or a
/// hand-written [`VisitFields`] implementation are not a bypass: their getters receive the same
/// per-processor redactor, but the adaptor or the implementation writes those
/// getters, so applying redaction is its responsibility rather than something
/// this crate enforces. Only the dynamic event body skips the redactor outright.
///
/// # Attribute macro
///
/// ```
/// use data_privacy::{DataClass, Sensitive};
/// use observed::event;
///
/// const DC: DataClass = DataClass::new("example", "public");
///
/// #[event("http.outgoing_request")]
/// #[info("HTTP request completed")]
/// #[histogram(duration_ms, name = "request_duration", unit = "ms")]
/// struct OutgoingRequest {
///     method: Sensitive<&'static str>,
///
///     #[unredacted]
///     duration_ms: f64,
/// }
/// ```
pub trait Event: VisitFields + Send + Sync {
    /// Static metadata describing this event's shape, severity, and fields.
    const DESCRIPTION: EventDescription;
}

/// Visits the fields of an [`Event`], or of a value in a field marked `#[flatten]`.
///
/// An [`event`](macro@crate::event) can get fields whose names are known only at run time from a field marked
/// `#[flatten]`. A field name is a `&'static str`, so a name that is built at run time must stay in memory until the
/// process ends, for example through [`String::leak`].
pub trait VisitFields {
    /// Whether the visited fields can include metric measurements.
    ///
    /// For an [`event`](macro@crate::event) with a `#[flatten]` field of this type,
    /// [`EventDescription::contains_metrics`] is `true` when this value is `true`.
    const CONTAINS_METRICS: bool;

    /// Lazily visits all fields.
    ///
    /// For each field, the visitor receives a [`FieldDescriptor`](crate::metadata::FieldDescriptor) and a getter
    /// closure. The getter takes a `&dyn Redactor` and returns the redacted
    /// [`Value`](crate::Value). It is only invoked if the processor wants the value.
    ///
    /// If the visitor returns [`ControlFlow::Break`], stop and return `ControlFlow::Break`.
    ///
    /// Fields follow one of three redaction paths:
    /// - **Default**: the type must implement [`data_privacy::RedactedDisplay`].
    /// - **`data_class = <expr>`**: wraps the value in [`data_privacy::Sensitive`] before redaction.
    /// - **`unredacted`**: bypasses redaction; the type must implement `Into<Value>`.
    ///
    /// Typed metric bindings use the static field inventory in [`Event::DESCRIPTION`].
    /// Visit its descriptors in declaration order, keeping their indices unchanged
    /// when optional fields are omitted. Instrument-bearing fields must supply
    /// unredacted numeric values with the declared representation.
    /// Flattened fields retain their signal routing but have no index in the
    /// outer event. Static metric bindings do not record those fields.
    fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()>;
}
