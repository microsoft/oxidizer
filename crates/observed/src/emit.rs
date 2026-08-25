// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The `emit!` macro for dispatching telemetry events to sinks.

/// Emits a telemetry event through the given [`Sink`](crate::Sink).
///
/// The sink argument accepts a [`Sink`](crate::Sink), a `&Sink`, or any value
/// that implements `AsRef<Sink>`.
///
/// Invocation forms:
///
/// 1. **Struct literal**: `emit!(sink, MyEvent { field1: val1, field2: val2 })`
/// 2. **Expression**: `emit!(sink, my_event_variable)`
///
/// The event is dispatched through the supplied sink and nothing else - there is
/// no ambient registry of emitters. To fan one call out to several destinations,
/// pass a sink built with [`Sink::composite`](crate::Sink::composite).
///
/// The macro automatically captures [`SourceLocation`](crate::metadata::SourceLocation) at the call site.
///
/// The captured crate name comes from `CARGO_PKG_NAME` (the emitting crate's
/// package name), so it stays stable regardless of the module the `emit!` call
/// is nested in.
///
/// # What emitting costs
///
/// The event expression is a closure, so it is evaluated only if at least one of
/// the sink's processors declares interest in the event's compile-time
/// [`EventDescription`](crate::metadata::EventDescription). If every processor
/// declines, nothing in the expression runs.
///
/// Once *any* processor is interested the expression is evaluated **in full**.
/// Laziness past that point is per field, not per signal: a processor pulls only
/// the fields it wants, and a field nobody pulls costs no clone, no allocation,
/// and no redaction call. But the expressions that *initialize* the fields have
/// already run. An event with an event-level metric and a log-only field
/// initialized by `load_user_profile()` therefore still calls
/// `load_user_profile()` when the only interested processor is metric-only.
///
/// Keep expensive or sensitive work out of the event expression when it is only
/// needed for one signal.
#[macro_export]
macro_rules! emit {
    ($sink:expr, $event:expr $(,)?) => {{
        ::core::convert::AsRef::<$crate::Sink>::as_ref(&$sink).emit::<_, _>(
            || $event,
            $crate::metadata::SourceLocation::new(::core::env!("CARGO_PKG_NAME"), ::core::file!(), ::core::line!()),
        );
    }};
}
