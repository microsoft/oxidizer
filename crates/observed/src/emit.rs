// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Emits a telemetry event to **all** registered emitters (fan-out).
///
/// The sink argument accepts a [`Sink`](crate::Sink), a `&Sink`, or any value
/// that implements `AsRef<Sink>`.
///
/// Invocation forms:
///
/// 1. **Struct literal**: `emit!(sink, MyEvent { field1: val1, field2: val2 })`
/// 2. **Expression**: `emit!(sink, my_event_variable)`
///
/// The macro automatically captures [`SourceLocation`](crate::metadata::SourceLocation) at the call site and
/// dispatches through the explicitly-passed sink plus any scoped emitters.
///
/// The captured crate name comes from `CARGO_PKG_NAME` (the emitting crate's
/// package name), so it stays stable regardless of the module the `emit!` call
/// is nested in.
#[macro_export]
macro_rules! emit {
    ($sink:expr, $event:expr $(,)?) => {{
        ::core::convert::AsRef::<$crate::Sink>::as_ref(&$sink).emit::<_, _>(
            || $event,
            $crate::metadata::SourceLocation::new(::core::env!("CARGO_PKG_NAME"), ::core::file!(), ::core::line!()),
        );
    }};
}
