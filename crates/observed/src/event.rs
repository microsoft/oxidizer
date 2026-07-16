// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ops::ControlFlow;

use crate::metadata::EventDescription;
use crate::processing::FieldVisitorFn;

/// A structured telemetry event.
///
/// Every event type implements this trait - typically via `#[derive(Event)]`.
/// All events are processed through a single, redaction-safe pipeline:
/// field values pass through a [`data_privacy::RedactionEngine`] for
/// privacy-safe extraction.
///
/// This ensures that all emitted telemetry - whether routed to logs, metrics,
/// or both - has consistent privacy-compliant behavior.
///
/// # Derive macro
///
/// ```
/// use data_privacy::{DataClass, Sensitive};
/// use observed::Event;
///
/// const DC: DataClass = DataClass::new("example", "public");
///
/// #[derive(Event)]
/// #[event(name = "http.outgoing_request")]
/// #[log(severity = info, message = "HTTP request completed")]
/// #[metric(kind = histogram, field = duration_ms, name = "request_duration", unit = "ms")]
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
    /// closure. The getter takes a `&RedactionEngine` and returns the redacted
    /// [`Value`](crate::Value). It is only invoked if the processor wants the value.
    ///
    /// Fields follow one of three redaction paths:
    /// - **Default**: the type must implement [`data_privacy::RedactedDisplay`].
    /// - **`data_class = <expr>`**: wraps the value in [`data_privacy::Sensitive`] before redaction.
    /// - **`unredacted`**: bypasses redaction; the type must implement `Into<Value>`.
    fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()>;
}
