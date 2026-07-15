// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Structured telemetry events with enrichment, redaction, and per-field routing.
//!
//! The `observed` crate provides a unified telemetry API that:
//!
//! - Emits **structured, typed events** via `#[derive(Event)]` and the [`emit!`] macro
//! - Supports **enrichment** - scoped, stackable, context-propagated entries
//!   attached to all events in scope (via RAII guards and `#[derive(Enrichment)]` structs)
//! - Enforces **redaction** - data-classification metadata on every field, redaction
//!   applied through a [`RedactionEngine`](data_privacy::RedactionEngine)
//! - Provides **per-field routing** - one event struct can produce logs and, metrics with
//!   independent field subsets per signal
//! - Integrates with **OpenTelemetry** through pluggable [`EventProcessor`](processing::EventProcessor) implementations
//!
//! # Quick Start
//!
//! ```
//! use data_privacy::{DataClass, Sensitive};
//! use observed::{Event, Sink, emit};
//!
//! const DC: DataClass = DataClass::new("example", "public");
//!
//! #[derive(Event)]
//! #[event(name = "my.event")]
//! #[log(severity = info, message = "Processing {my.event.field}")]
//! struct MyEvent {
//!     #[dimension(log = "my.event.field")]
//!     field: Sensitive<&'static str>,
//! }
//!
//! fn do_something(sink: &Sink) {
//!     emit!(
//!         sink,
//!         MyEvent {
//!             field: Sensitive::new("val", DC)
//!         }
//!     );
//!     // do something
//! }
//! ```
//!
//! # Enrichment
//!
//! Enrichment attaches key-value context to **every event** emitted within a scope.
//! Typical use cases include request IDs, user identifiers, or operation names that
//! should appear on all telemetry without being passed explicitly to each event.
//!
//! ## Scoped enrichment
//!
//! Use the [`EnrichFutureExt::enrich`](crate::enrichment::EnrichFutureExt::enrich) or
//! [`EnrichFnExt::enrich`](crate::enrichment::EnrichFnExt::enrich) extension
//! methods to attach entries to a future or closure. The entries are pushed onto
//! the thread-local slot on every poll (or call) and popped when the poll
//! completes:
//!
//! ```
//! # use observed::enrichment::EnrichFutureExt;
//! # use observed::{Enrichment, Event, Sink, emit};
//! # use data_privacy::{DataClass, Sensitive};
//! # const DC: DataClass = DataClass::new("example", "public");
//! # type RequestId = Sensitive<&'static str>;
//! # #[derive(Event)]
//! # #[event(name = "my.event")]
//! # #[log(severity = info, message = "body")]
//! # struct MyEvent;
//! # impl MyEvent { fn new(_: &str) -> Self { Self } }
//! #[derive(Enrichment)]
//! struct RequestCtx {
//!     #[dimension(log = "request.id")]
//!     request_id: RequestId,
//! }
//!
//! async fn fetch(request_id: RequestId, sink: &Sink) {
//!     async {
//!         emit!(sink, MyEvent::new("test")); // sees request.id
//!     }
//!     .enrich(sink, RequestCtx { request_id })
//!     .await;
//! }
//! ```
//!
//! ## Transferring enrichment across threads and tasks
//!
//! Enrichment is not automatically propagated to other threads or async tasks. It has to be
//! explicitly transferred via [`Sink::transfer_context`] and
//! [`Transfer::apply`](crate::context::Transfer::apply).
//!
//! How it works:
//!
//! - [`Sink::transfer_context`] captures the current enrichment state into a plain data struct
//!   ([`Transfer`](crate::context::Transfer)).
//! - [`Transfer::apply`](crate::context::Transfer::apply) restores the captured state on the
//!   target thread or in the spawned future's poll. The returned guard restores the previous
//!   state on drop.
//!
//! ```
//! # use observed::{Event, Sink, emit};
//! # use data_privacy::{DataClass, Sensitive};
//! # const DC: DataClass = DataClass::new("example", "public");
//! # #[derive(Event)]
//! # #[event(name = "my.event")]
//! # #[log(severity = info, message = "body")]
//! # struct MyEvent;
//! # let sink = Sink::noop();
//! let transfer = sink.transfer_context();
//!
//! let sink = sink.clone();
//! let handle = std::thread::spawn(move || {
//!     // Restore the captured enrichment on this thread for the guard's lifetime.
//!     let _guard = transfer.apply();
//!     emit!(sink, MyEvent); // sees parent enrichment
//! });
//! handle.join().unwrap();
//! ```
//!
//! ## Resolution at emission time
//!
//! When `emit!` fires, the sink walks its thread-local enrichment chain and
//! collects all visible entries and passes them to processors along with the event.

// Allow `::observed::…` paths emitted by derive macros to resolve inside this crate.
extern crate self as observed;

#[macro_use]
mod emit;

pub mod context;
pub mod enrichment;
pub(crate) mod event;
pub mod interop;
pub(crate) mod key;
pub mod metadata;
pub mod processing;
pub(crate) mod severity;
pub(crate) mod sink;
pub(crate) mod value;

// Re-export the derive macro and proc macros.

// Re-export core types at the crate root for convenience.
pub use event::Event;
pub use key::Key;
/// Derives the [`Enrichment`](enrichment::Enrichment) trait for a struct.
///
/// Enrichment structs produce key-value entries that are attached to **every event**
/// emitted within a scope - without being passed explicitly to each event.
/// Unlike [`Event`], enrichment structs have no severity, body, or metrics.
///
/// The derive also generates an [`IntoIterator`] implementation so the struct
/// can be passed directly to [`.enrich()`](enrichment::EnrichFutureExt::enrich) /
/// [`.enrich()`](enrichment::EnrichFnExt::enrich).
///
/// # Full syntax
///
/// ```text
/// #[derive(Enrichment)]
/// struct MyContext {
///     // ── field with default redaction ─────────────────────────
///     field: T,                             // T: RedactedDisplay
///
///     // ── routing modifiers ────────────────────────────────────
///     #[dimension]                          // log under field name; not a metric dimension
///     #[dimension(log = "...")]             // rename enrichment key
///     #[dimension(log = exclude)]           // exclude from logs
///     #[dimension(metric)]                  // metric dimension keyed by the field name
///     #[dimension(metric = "...")]          // metric dimension with an explicit key
///
///     // ── redaction modifiers (mutually exclusive) ─────────────
///     #[unredacted]                         // bypass redaction; T: Into<Value>
///     #[data_class(<expr>)]                 // wrap in Sensitive::new(value, <expr>)
///
///     field: T,
///
///     // ── optional fields ──────────────────────────────────────
///     opt: Option<T>,                       // `None` → filled with "n/a" (default)
/// }
/// ```
///
/// # Field-level attributes
///
/// | Attribute | Description |
/// |-----------|-------------|
/// | `#[dimension]` | Log under the field's own name; not a metric dimension (the explicit default). |
/// | `#[dimension(log = "...")]` | Rename the enrichment key. |
/// | `#[dimension(log = exclude)]` | Exclude the field from log records. |
/// | `#[dimension(metric)]` | Opt the field in as a metric dimension keyed by the field's own name. |
/// | `#[dimension(metric = "...")]` | Opt the field in as a metric dimension under the given key. |
/// | `#[dimension(log = "...", metric = "...")]` | Route both signals with independent keys. Either side may be omitted (but not both); `log = exclude` omits the field from logs, and a bare `metric` uses the field name. |
/// | `#[unredacted]` | Bypass redaction; the type must implement `Into<Value>`. |
/// | `#[data_class(<expr>)]` | Wrap the value in `Sensitive::new(value, <expr>)` for classification. |
/// | `#[if_none(drop)]` / `#[if_none("...")]` | Control how a `None` `Option<T>` is recorded. The default is `#[if_none("n/a")]`. |
///
/// `#[unredacted]` and `#[data_class(...)]` are mutually exclusive.
///
/// ## Optional fields
///
/// A field of type `Option<T>` is captured like a `T` when it is `Some(_)`. When
/// it is `None`, `#[if_none(...)]` decides the outcome (default
/// `#[if_none("n/a")]`, or `drop` to omit it) - the same behavior as in
/// [`Event`].
///
/// # Redaction paths
///
/// Every enrichment field follows one of three redaction paths:
///
/// 1. **Default** - the type must implement `RedactedDisplay`. The value is stored
///    as a trait object and redacted at emission time.
/// 2. **`#[data_class(<expr>)]`** - wraps the value in `Sensitive::new(value, <expr>)`
///    before storing, for types without built-in classification.
/// 3. **`#[unredacted]`** - bypasses redaction entirely; the type must implement
///    `Into<Value>`.
///
/// # Example
///
/// ```
/// use data_privacy::{DataClass, Sensitive};
/// use observed::enrichment::EnrichFnExt;
/// use observed::{Enrichment, Event, Sink, emit};
///
/// const DC: DataClass = DataClass::new("example", "public");
///
/// #[derive(Event)]
/// #[event(name = "my.event")]
/// #[log(severity = info, message = "body")]
/// struct MyEvent {
///     #[unredacted]
///     status: i64,
/// }
///
/// #[derive(Enrichment)]
/// struct RequestContext {
///     #[dimension(log = "request.id")]
///     #[unredacted]
///     request_id: i64,
///     user_agent: Sensitive<&'static str>,
/// }
///
/// let sink = Sink::noop();
/// (|| {
///     emit!(sink, MyEvent { status: 200 }); // sees request.id & user_agent
/// })
/// .enrich(
///     &sink,
///     RequestContext {
///         request_id: 42,
///         user_agent: Sensitive::new("curl/8.0", DC),
///     },
/// )();
/// ```
pub use observed_macros::Enrichment;
/// Derives the [`Event`] trait for a struct.
///
/// # Full syntax
///
/// ```text
/// #[derive(Event)]
/// #[event(name = "<event_name>")]                              // REQUIRED
/// #[log(severity = <SEVERITY> [, name = "..."] [, message = "..."])]  // optional
/// #[metric(kind = <KIND> [, field = <field>] [, name = "..."] [, description = "..."] [, unit = "..."])]  // optional
/// // <KIND> is one of: counter, updown_counter, gauge, histogram
/// #[disabled]                                                      // optional
/// struct MyEvent {
///     // ── field with default redaction (log-only) ──────────────
///     field: T,                             // T: RedactedDisplay
///
///     // ── field routing modifiers ──────────────────────────────
///     #[dimension]                          // log under field name; not a metric dimension
///     #[dimension(log = "...")]             // rename log key
///     #[dimension(log = exclude)]           // exclude from logs
///     #[dimension(metric)]                  // metric dimension keyed by the field name
///     #[dimension(metric = "...")]          // metric dimension with an explicit key
///
///     // ── redaction modifiers (mutually exclusive) ─────────────
///     #[unredacted]                         // bypass redaction; T: Into<Value>
///     #[data_class(<expr>)]                 // wrap in Sensitive::new(value, <expr>)
///
///     field: T,
///
///     // ── optional fields ──────────────────────────────────────
///     opt: Option<T>,                       // `None` → filled with "n/a" (default)
///     #[if_none(drop)]                      // ...or omit `opt` entirely when `None`
///     opt2: Option<T>,
/// }
/// ```
///
/// **Placeholders:**
///
/// | Placeholder | Allowed values |
/// |-------------|---------------|
/// | `<SEVERITY>` | `trace`, `debug`, `info`, `warn`, `error`, `fatal` |
///
/// # Struct-level attributes
///
/// | Attribute | Required | Description |
/// |-----------|----------|-------------|
/// | `#[event(name = "...")]` | **yes** | Canonical event name used for routing and identification. |
/// | `#[log(severity = <S>)]` | no | Opt into log emission. `severity` is required; `name` defaults to the event name; `message` is optional. |
/// | `#[metric(kind = <KIND>, ...)]` | no | Declare a metric instrument (`<KIND>` = `counter`, `updown_counter`, `gauge`, or `histogram`). See [Metric instruments](#metric-instruments). |
/// | `#[disabled]` | no | Mark the event as opt-in only; processors must explicitly enable it. |
///
/// # Metric instruments
///
/// Metrics are declared with a **struct-level** `#[metric(kind = <KIND>)]`
/// attribute, where `<KIND>` selects the OpenTelemetry instrument kind:
///
/// | `kind` | Records | `field` |
/// |--------|---------|---------|
/// | `counter` | Monotonic sum | **optional** — omit to record `1` per emission |
/// | `updown_counter` | Bidirectional sum | **required** |
/// | `gauge` | Last value | **required** |
/// | `histogram` | Value distribution | **required** |
///
/// The `field = <name>` argument names the struct field whose value is recorded
/// by the instrument. The referenced field **must exist**, otherwise compilation
/// fails. A fieldless `#[metric(kind = counter)]` records `1` for every emission.
///
/// Value-type constraints are enforced at compile time:
///
/// - `#[metric(kind = counter, field = x)]` requires `x` to be an **unsigned**
///   integer type (`u8`..`u128`, `usize`).
/// - `#[metric(kind = updown_counter, field = x)]` requires `x` to be a
///   **signed** integer type (`i8`..`i128`, `isize`).
/// - `kind = gauge` / `kind = histogram` impose no value-type constraint.
///
/// Each instrument also accepts optional `name` (defaults to the event
/// name), `description`, and `unit` arguments:
///
/// ```text
/// #[event(name = "http.request")]
/// #[metric(kind = histogram, field = duration, name = "http.server.duration", unit = "ms")]
/// struct HttpRequest {
///     #[unredacted]
///     duration: f64,
/// }
/// ```
///
/// ## Name resolution
///
/// `event(name)` is the canonical identity of the event. `log(name)` and the
/// instrument `name` **default to the event name** when omitted:
///
/// ```text
/// #[event(name = "http.request")]
/// #[log(severity = info)]                          // log name  = "http.request"
/// #[metric(kind = updown_counter, field = in_flight)]  // metric name = "http.request"
/// ```
///
/// ### Mapping to runtime types
///
/// | Attribute | Stored in |
/// |-----------|----------|
/// | `event(name)` | [`EventDescription::name()`](crate::metadata::EventDescription::name) |
/// | `log(name)` | [`LogDescription::name()`](crate::metadata::LogDescription::name) |
/// | `log(severity)` | [`LogDescription::severity()`](crate::metadata::LogDescription::severity) |
/// | `log(message)` | [`LogDescription::body()`](crate::metadata::LogDescription::body) |
/// | instrument `name` | [`MetricDescription::instrument_name()`](crate::metadata::MetricDescription::instrument_name) |
/// | instrument kind | [`MetricDescription::kind()`](crate::metadata::MetricDescription::kind) |
///
/// # Field-level attributes
///
/// By default every field participates in the log signal (when `#[log(...)]`
/// is present) and is **excluded** from metric dimensions.
///
/// | Attribute | Description |
/// |-----------|-------------|
/// | `#[dimension]` | Log under the field's own name; not a metric dimension (the explicit default). |
/// | `#[dimension(log = "...")]` | Rename the log key. |
/// | `#[dimension(log = exclude)]` | Exclude the field from log records. |
/// | `#[dimension(metric)]` | Register the field as a metric dimension keyed by the field's own name. |
/// | `#[dimension(metric = "...")]` | Register the field as a metric dimension with the given key. |
/// | `#[dimension(log = "...", metric = "...")]` | Route both signals with independent keys. Either side may be omitted (but not both); `log = exclude` omits the field from logs, and a bare `metric` uses the field name. |
/// | `#[unredacted]` | Bypass redaction; the type must implement `Into<Value>`. |
/// | `#[data_class(<expr>)]` | Wrap the value in `Sensitive::new(value, <expr>)` for classification. |
/// | `#[if_none(drop)]` / `#[if_none("...")]` | Control how a `None` `Option<T>` is recorded. The default is `#[if_none("n/a")]`. |
///
/// `#[unredacted]` and `#[data_class(...)]` are mutually exclusive.
///
/// ## Optional fields
///
/// A field of type `Option<T>` is captured like a `T` when it is `Some(_)`. When
/// it is `None`, `#[if_none(...)]` decides the outcome: by default
/// (`#[if_none("n/a")]`) a `"n/a"` placeholder is recorded for the log attribute
/// and/or metric dimension, while `#[if_none(drop)]` omits the field
/// entirely for that emission.
///
/// `Option<T>` is detected syntactically, so a type aliased to `Option` is not
/// recognized.
///
/// `#[dimension(metric = "...")]` may be declared even when the event has no
/// instrument attribute — useful for custom processors that define dynamic
/// metrics and need pre-declared dimension keys on the field descriptor.
pub use observed_macros::Event;
pub use severity::Severity;
pub use sink::{Sink, SinkId};
pub use value::Value;

/// Hidden module re-exporting types that the `emit!` and `#[derive(Event)]` macros reference.
///
/// This is **not public API** - it exists solely for macro-generated code.
#[doc(hidden)]
pub mod __private {
    // data_privacy re-exports (no public path in this crate).
    pub use ::data_privacy::{RedactionEngine, Sensitive};

    pub use crate::enrichment::EnrichmentEntry;
}
