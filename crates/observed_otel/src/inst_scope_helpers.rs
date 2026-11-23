// Copyright (c) Microsoft Corporation.

//! Conversions into `OTel` instrumentation scopes.
//!
//! [`IntoInstrumentationScope`] lets destination extension methods accept
//! either a fully-built [`opentelemetry::InstrumentationScope`] or a plain
//! name, keeping the common name-only case.

/// Anything that can be turned into an `OTel`
/// [`opentelemetry::InstrumentationScope`] — accepted by
/// [`crate::MetricDestinationExt::with_scope`].
///
/// Implemented for [`opentelemetry::InstrumentationScope`] itself (identity)
/// and for the string types accepted by `InstrumentationScope::builder`
/// (`&'static str`, `String`, `Cow<'static, str>`), each producing a scope
/// with just the given name and default version / `schema_url` / attributes.
pub trait IntoInstrumentationScope {
    /// Converts `self` into an [`opentelemetry::InstrumentationScope`].
    fn into_instrumentation_scope(self) -> opentelemetry::InstrumentationScope;
}

impl IntoInstrumentationScope for opentelemetry::InstrumentationScope {
    fn into_instrumentation_scope(self) -> opentelemetry::InstrumentationScope {
        self
    }
}

impl IntoInstrumentationScope for &'static str {
    fn into_instrumentation_scope(self) -> opentelemetry::InstrumentationScope {
        opentelemetry::InstrumentationScope::builder(self).build()
    }
}

impl IntoInstrumentationScope for String {
    fn into_instrumentation_scope(self) -> opentelemetry::InstrumentationScope {
        opentelemetry::InstrumentationScope::builder(self).build()
    }
}

impl IntoInstrumentationScope for std::borrow::Cow<'static, str> {
    fn into_instrumentation_scope(self) -> opentelemetry::InstrumentationScope {
        opentelemetry::InstrumentationScope::builder(self).build()
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use crate::IntoInstrumentationScope;

    #[test]
    fn string_conversions_preserve_the_scope_name() {
        let scopes = [
            "telemetry.requests".into_instrumentation_scope(),
            String::from("telemetry.requests").into_instrumentation_scope(),
            Cow::Borrowed("telemetry.requests").into_instrumentation_scope(),
        ];

        for scope in scopes {
            assert_eq!(scope.name(), "telemetry.requests");
        }
    }
}
