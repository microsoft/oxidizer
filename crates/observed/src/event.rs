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
/// through the processor-supplied redactor. Fields marked `#[unredacted]`,
/// unclassified enrichment values, and values supplied by dynamic adaptors are
/// caller-controlled and must already be appropriate to emit.
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
pub trait Event: Send + Sync {
    /// Static metadata describing this event's shape, severity, and fields.
    const DESCRIPTION: EventDescription;

    /// Lazily visits all fields on this event.
    ///
    /// For each field, the visitor receives a [`FieldDescriptor`](crate::metadata::FieldDescriptor) and a getter
    /// closure. The getter takes a `&dyn Redactor` and returns the redacted
    /// [`Value`](crate::Value). It is only invoked if the processor wants the value.
    ///
    /// Fields follow one of three redaction paths:
    /// - **Default**: the type must implement [`data_privacy::RedactedDisplay`].
    /// - **`data_class = <expr>`**: wraps the value in [`data_privacy::Sensitive`] before redaction.
    /// - **`unredacted`**: bypasses redaction; the type must implement `Into<Value>`.
    fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()>;
}
