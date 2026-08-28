// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/observed/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/observed/favicon.ico")]
// TODO(doc-coverage): remove once `missing_docs` is promoted to [workspace.lints.rust].
#![deny(missing_docs)]
// Public enums must opt in to growth explicitly via `#[non_exhaustive]`;
// otherwise adding a variant is a breaking change for downstream crates.
#![warn(clippy::exhaustive_enums)]

//! Structured telemetry events with enrichment, redaction, and per-field routing.
//!
//! The `observed` crate provides a unified telemetry API that:
//!
//! - Emits **structured, typed events** via `#[event(...)]` and the [`emit!`] macro
//! - Supports **enrichment** - scoped, stackable, context-propagated entries
//!   attached to all events in scope (via RAII guards and `#[derive(Enrichment)]` structs)
//! - Supports **redaction** - classified fields are extracted through a
//!   [`RedactionEngine`](data_privacy::RedactionEngine), while explicit
//!   unredacted paths remain caller-controlled
//! - Provides **per-field routing** - one event struct can produce logs and metrics with
//!   independent field subsets per signal
//! - Integrates with **OpenTelemetry** through pluggable [`EventProcessor`](processing::EventProcessor) implementations
//!
//! # Quick Start
//!
//! ```
//! use data_privacy::{DataClass, Sensitive};
//! use observed::{Sink, emit, event};
//!
//! const DC: DataClass = DataClass::new("example", "public");
//!
//! #[event("my.event")]
//! #[info("Processing {my.event.field}")]
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
//! # use observed::{Enrichment, Sink, emit, event};
//! # use data_privacy::{DataClass, Sensitive};
//! # const DC: DataClass = DataClass::new("example", "public");
//! # type RequestId = Sensitive<&'static str>;
//! # #[event("my.event")]
//! # #[info("body")]
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
//! Enrichment lives in a thread-local slot, so it is **not** automatically
//! propagated to other threads or async tasks.
//!
//! **Most code should not do this by hand.** The runtime integrations
//! (`observed_rt`, which layers over `anyspawn`, and `oxidizer_rt`) propagate
//! enrichment to every spawned task for you: enrich at the spawn site, spawn
//! through the runtime, and the context follows. The rest of this section is
//! the plumbing underneath, and is aimed at people writing such an integration -
//! a spawner, a `tower` layer, or similar middleware.
//!
//! Integrators transfer it explicitly:
//!
//! - [`Sink::transfer_context`] snapshots the current thread's enrichment into a
//!   plain, sendable [`Transfer`](crate::context::Transfer) value.
//! - [`EnrichFutureExt::attach`](crate::enrichment::EnrichFutureExt::attach) wraps a
//!   future so the captured enrichment is restored **on every poll** and removed
//!   again before the future yields.
//!
//! Applying a transfer mutates the enrichment of the current thread for the
//! lifetime of the returned guard, so any emission made through the original sink
//! on that thread also sees the transferred entries.
//!
//! To add entries of your own, put them **in the transfer** with
//! [`Transfer::with_enrichment`](crate::context::Transfer::with_enrichment) or
//! [`Transfer::with_enrichment_for`](crate::context::Transfer::with_enrichment_for).
//! That is what an integration wants: it is independent of wrapper order, so it
//! keeps working once the future is boxed or wrapped again further out - both of
//! which happen in real integrations.
//!
//! Wrapping [`enrich`](crate::enrichment::EnrichFutureExt::enrich) around
//! `attach` also works for a single `attach` on a plain, non-boxed future, since
//! [`Transferred::enrich`](crate::context::Transferred::enrich) re-orders the two.
//! That is a convenience for hand-written code, not a general guarantee - see its
//! docs for the shapes it does not cover.
//!
//! ```
//! # use observed::enrichment::EnrichFutureExt;
//! # use observed::{Sink, emit, event};
//! # #[event("my.event")]
//! # #[info("body")]
//! # struct MyEvent;
//! # fn spawn_child(sink: &Sink) {
//! // Capture the current thread's enrichment as a plain, sendable value...
//! let transfer = sink.transfer_context();
//!
//! // ...and attach it to the future that will run on another task/thread.
//! // `attach` restores the enrichment on every poll and drops it before the
//! // future yields, so unrelated tasks on the same worker thread never see it.
//! let sink = sink.clone();
//! let task = async move {
//!     emit!(sink, MyEvent); // sees the transferred enrichment
//! }
//! .attach(transfer);
//! // Hand `task` to your executor, e.g. `tokio::spawn(task)`.
//! let _ = task;
//! # }
//! ```
//!
//! For synchronous work you can instead apply the low-level guard directly via
//! [`Transfer::apply_current_thread`](crate::context::Transfer::apply_current_thread).
//! **Never hold that guard across an `.await`**: because it mutates a thread-local,
//! the enrichment would stay active while the task is suspended and leak into
//! unrelated tasks that the runtime schedules on the same thread. Use
//! [`attach`](crate::enrichment::EnrichFutureExt::attach) for async code so the
//! guard is scoped to a single poll.
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
pub(crate) mod error;
pub(crate) mod event;
pub mod interop;
pub(crate) mod key;
pub mod metadata;
pub mod processing;
pub(crate) mod severity;
pub(crate) mod sink;
pub(crate) mod text;
pub(crate) mod value;

// Re-export the derive macro and proc macros.

// Re-export core types at the crate root for convenience.
pub use error::{FlushError, SinkFlushError};
pub use event::Event;
pub use key::Key;
/// Derives the [`Enrichment`](enrichment::Enrichment) trait for a struct.
///
/// Enrichment structs produce key-value entries that are attached to **every event**
/// emitted within a scope - without being passed explicitly to each event.
/// Unlike [`Event`], enrichment structs have no severity, body, or metrics.
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
/// | `#[unredacted]` | Bypass redaction; the type must implement `Into<Value>`, or be a directly spelled borrowed `&str`. |
/// | `#[data_class(<expr>)]` | Wrap the value in `Sensitive::new(value, <expr>)` for classification. |
/// | `#[if_none(drop)]` / `#[if_none("...")]` | Control how a `None` `Option<T>` is recorded. The default is `#[if_none("n/a")]`. |
///
/// `#[unredacted]` and `#[data_class(...)]` are mutually exclusive.
/// An enrichment struct cannot declare an instrument.
///
/// ## Optional fields
///
/// A field of type `Option<T>` is captured like a `T` when it is `Some(_)`. When
/// it is `None`, `#[if_none(...)]` decides the outcome (default
/// `#[if_none("n/a")]`, or `drop` to omit it) - the same behavior as in the
/// [`event`](macro@crate::event) macro.
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
///    `Into<Value>`. A borrowed `&str` is the one exception: it has no
///    `Into<Value>` and is copied into an `Arc<str>` instead. Like `Option<T>`,
///    it is detected from the field's spelling, so an aliased `&str` is not
///    recognized and needs an explicit `Arc::<str>::from(s)`.
///
/// # Example
///
/// ```
/// use data_privacy::{DataClass, Sensitive};
/// use observed::enrichment::EnrichFnExt;
/// use observed::{Enrichment, Sink, emit, event};
///
/// const DC: DataClass = DataClass::new("example", "public");
///
/// #[event("my.event")]
/// #[info("body")]
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
/// Declares a struct as an [`Event`] and generates its trait impl.
///
/// # Full syntax
///
/// ```text
/// #[event("<event_name>" [, disabled])]                              // REQUIRED (`disabled` optional)
/// #[<severity>[("<message>")] [, name = "..."]]                      // optional log signal
/// #[<kind>([<field>] [, name = "..."] [, desc = "..."] [, unit = "..."])]  // optional; repeatable
/// // <severity> is one of: trace, debug, info, warning, error, fatal
/// // <kind>     is one of: counter, updown_counter, gauge, histogram
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
/// # Struct-level attributes
///
/// | Attribute | Required | Description |
/// |-----------|----------|-------------|
/// | `#[event("...")]` | **yes** | Canonical event name used for identification and processor interest checks. Add `disabled` (`#[event("...", disabled)]`) to surface disabled metadata to processors; the sink does not enforce it. |
/// | `#[<severity>]` | no | Opt into log emission, where `<severity>` is one of `trace`, `debug`, `info`, `warning`, `error`, `fatal`. At most one may be present. (The `warn` level is spelled `warning` because `warn` is a built-in attribute.) The optional positional string is the log body; `{key}` placeholders name effective log keys after raw-identifier normalization, `#[dimension(log = "...")]` renames, and `#[dimension(log = exclude)]` exclusions. `{{` escapes an opening brace; empty `{}` and unmatched `{` are accepted as literal text. An optional `name = "..."` overrides the log name (defaults to the event name). |
/// | `#[<kind>(...)]` | no | Declare a metric instrument (`<kind>` = `counter`, `updown_counter`, `gauge`, or `histogram`). See [Metric instruments](#metric-instruments). |
///
/// # Metric instruments
///
/// Metrics are declared with a **struct-level** `#[<kind>(...)]` attribute, where
/// `<kind>` selects the OpenTelemetry instrument kind:
///
/// | `<kind>` | Records | field |
/// |--------|---------|---------|
/// | `counter` | Monotonic sum | **optional** — omit to record `1` per emission |
/// | `updown_counter` | Bidirectional sum | **required** |
/// | `gauge` | Last value | **required** |
/// | `histogram` | Value distribution | **required** |
///
/// The leading positional `<field>` names the struct field whose value is
/// recorded by the instrument. The referenced field **must exist**, otherwise
/// compilation fails. A fieldless `#[counter]` records `1` for every emission.
///
/// Value-type constraints are enforced at compile time:
///
/// - The value field must be `#[unredacted]`. A classified value is rendered
///   through the redaction engine as a *string*, which carries no measurement
///   for the instrument to record.
/// - The value field must be a numeric primitive [`Value`] can carry: `i8`,
///   `i16`, `i32`, `i64`, `isize`, `u8`, `u16`, `u32`, `u64`, `usize`, `f32`,
///   `f64`. `u128` and `i128` are **not** supported - no telemetry backend
///   represents them - and neither is a newtype wrapper.
/// - `#[counter(x)]` requires `x` to be an **unsigned** integer type.
/// - `#[updown_counter(x)]` requires `x` to be a **signed** integer type.
/// - `gauge` / `histogram` accept any supported numeric type, floats included.
/// - No instrument accepts an `Option<T>` value field: an instrument records a
///   measurement on every emission, and the `#[if_none(...)]` placeholder for a
///   `None` is a *string*, which is not a valid measurement. Record an optional
///   field as a metric dimension (`#[dimension(metric = "...")]`) instead.
///
/// Each instrument also accepts optional `name` (defaults to the event
/// name), `desc`, and `unit` arguments:
///
/// ```text
/// #[event("http.request")]
/// #[histogram(duration, name = "http.server.duration", unit = "ms")]
/// struct HttpRequest {
///     #[unredacted]
///     duration: f64,
/// }
/// ```
///
/// ## Name resolution
///
/// The `#[event("...")]` name is the canonical identity of the event. The log
/// `name = "..."` and the instrument `name` **default to the event name** when
/// omitted:
///
/// ```text
/// #[event("http.request")]
/// #[info]                             // log name    = "http.request"
/// #[updown_counter(in_flight)]        // metric name = "http.request"
/// ```
///
/// ### Mapping to runtime types
///
/// | Attribute | Stored in |
/// |-----------|----------|
/// | `event("...")` | [`EventDescription::name()`](crate::metadata::EventDescription::name) |
/// | log `name` | [`LogDescription::name()`](crate::metadata::LogDescription::name) |
/// | severity | [`LogDescription::severity()`](crate::metadata::LogDescription::severity) |
/// | log message | [`LogDescription::body()`](crate::metadata::LogDescription::body) |
/// | instrument `name` | [`MetricDescription::instrument_name()`](crate::metadata::MetricDescription::instrument_name) |
/// | instrument kind | [`MetricDescription::kind()`](crate::metadata::MetricDescription::kind) |
///
/// # Field-level attributes
///
/// By default every field participates in the log signal (when a severity
/// attribute is present) and is **excluded** from metric dimensions.
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
/// A borrowed `&str` field is likewise detected syntactically: it is copied into
/// the event with an explicit [`Arc<str>`](std::sync::Arc) allocation, because
/// [`Value`] owns its data and only `&'static str` is stored without copying. A
/// type aliased to `&str` is not recognized and fails to compile; give such a
/// field an explicit `Arc::<str>::from(s)` getter instead.
///
/// `#[dimension(metric = "...")]` may be declared even when the event has no
/// instrument attribute — useful for custom processors that define dynamic
/// metrics and need pre-declared dimension keys on the field descriptor.
pub use observed_macros::event;
pub use severity::Severity;
pub use sink::{Sink, SinkId};
pub use text::Text;
pub use value::Value;

/// Hidden module re-exporting types that the `emit!` and `#[event(...)]` macros reference.
///
/// This is **not public API** - it exists solely for macro-generated code.
#[doc(hidden)]
pub mod __private {
    // data_privacy re-exports (no public path in this crate).
    pub use ::data_privacy::{RedactedDisplay, RedactedToString, Sensitive};

    pub use crate::enrichment::EnrichmentEntry;
}
