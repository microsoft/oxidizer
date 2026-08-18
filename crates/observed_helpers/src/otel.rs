// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Conversions from `observed` types to their `OpenTelemetry` counterparts.

use observed::{Severity, Text, Value};
use opentelemetry::logs::AnyValue;

/// Converts a [`Text`] into an `OTel` [`StringValue`](opentelemetry::StringValue),
/// preserving the borrowed-versus-shared distinction so neither representation
/// copies.
// Excluded from the coverage gate for the reason given above `any_value_of`.
#[cfg_attr(coverage_nightly, coverage(off))]
fn string_value_of(text: Text) -> opentelemetry::StringValue {
    match text {
        Text::Static(s) => s.into(),
        Text::Shared(s) => s.into(),
        // Guards the `#[non_exhaustive]` enum; a future representation still
        // exports its contents, at the cost of a copy.
        ref other => other.as_str().to_owned().into(),
    }
}

/// Converts every element of `values` into an `OTel` list.
fn list_of<T>(values: Vec<T>, mut f: impl FnMut(T) -> AnyValue) -> AnyValue {
    AnyValue::ListAny(Box::new(values.into_iter().map(&mut f).collect()))
}

/// Converts a `u64` into an `OTel` [`AnyValue`].
///
/// `AnyValue` has no unsigned variant, so a value past `i64::MAX` is exported as
/// its decimal string rather than wrapping into a negative number.
fn unsigned_any_value(value: u64) -> AnyValue {
    i64::try_from(value).map_or_else(|_| AnyValue::String(value.to_string().into()), AnyValue::Int)
}

/// Converts a `u64` into an `OTel` [`opentelemetry::Value`].
///
/// See [`unsigned_any_value`]: `opentelemetry::Value` has no unsigned variant
/// either.
fn unsigned_otel_value(value: u64) -> opentelemetry::Value {
    i64::try_from(value).map_or_else(
        |_| opentelemetry::Value::String(value.to_string().into()),
        opentelemetry::Value::I64,
    )
}

/// Converts a [`Value`] into an `OTel` [`AnyValue`], as used for log record
/// attributes and bodies.
// The last arm guards the `#[non_exhaustive]` `Value`, so no variant that exists
// today can reach it, and coverage instrumentation counts an arm that is never
// taken as an uncovered line. The dispatch is therefore excluded from the
// coverage gate instead of the guard being deleted: without the guard, a variant
// added upstream would either fail to compile here or silently lose data. The
// conversions it delegates to stay measured, and mutation testing still applies.
#[cfg_attr(coverage_nightly, coverage(off))]
#[must_use]
pub fn any_value_of(value: Value) -> AnyValue {
    match value {
        Value::Bool(v) => AnyValue::Boolean(v),
        Value::I64(v) => AnyValue::Int(v),
        Value::U64(v) => unsigned_any_value(v),
        Value::F64(v) => AnyValue::Double(v),
        Value::String(v) => AnyValue::String(string_value_of(v)),
        Value::BoolArray(v) => list_of(v, AnyValue::Boolean),
        Value::I64Array(v) => list_of(v, AnyValue::Int),
        Value::F64Array(v) => list_of(v, AnyValue::Double),
        Value::StringArray(v) => list_of(v, |s| AnyValue::String(string_value_of(s))),
        // Guards the `#[non_exhaustive]` enum. Unreachable for the shapes that
        // exist today; a future shape degrades to its debug form rather than
        // being dropped.
        other => AnyValue::String(format!("{other:?}").into()),
    }
}

/// Converts a [`Value`] into an `OTel` [`opentelemetry::Value`], as used for
/// metric dimensions.
// Excluded from the coverage gate for the reason given above `any_value_of`.
#[cfg_attr(coverage_nightly, coverage(off))]
#[must_use]
pub fn otel_value_of(value: Value) -> opentelemetry::Value {
    use opentelemetry::Array;

    match value {
        Value::Bool(v) => opentelemetry::Value::Bool(v),
        Value::I64(v) => opentelemetry::Value::I64(v),
        Value::U64(v) => unsigned_otel_value(v),
        Value::F64(v) => opentelemetry::Value::F64(v),
        Value::String(v) => opentelemetry::Value::String(string_value_of(v)),
        Value::BoolArray(v) => opentelemetry::Value::Array(Array::Bool(v)),
        Value::I64Array(v) => opentelemetry::Value::Array(Array::I64(v)),
        Value::F64Array(v) => opentelemetry::Value::Array(Array::F64(v)),
        Value::StringArray(v) => opentelemetry::Value::Array(Array::String(v.into_iter().map(string_value_of).collect())),
        // Guards the `#[non_exhaustive]` enum; see `any_value_of`.
        other => opentelemetry::Value::String(format!("{other:?}").into()),
    }
}

/// Converts a [`Severity`] into its `OTel` counterpart.
#[must_use]
pub fn otel_severity_of(severity: Severity) -> opentelemetry::logs::Severity {
    use opentelemetry::logs::Severity as Otel;

    match severity {
        Severity::Trace => Otel::Trace,
        Severity::Debug => Otel::Debug,
        Severity::Warn => Otel::Warn,
        Severity::Error => Otel::Error,
        Severity::Fatal => Otel::Fatal,
        // `Severity::Info`, plus the guard for the `#[non_exhaustive]` enum. A
        // severity this crate does not know about maps to `Info` rather than to
        // an extreme: alerting keys off `severity_number`, so defaulting to
        // `Fatal` would page on an unknown variant and `Trace` would hide it.
        _ => Otel::Info,
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_convert_to_any_value() {
        assert_eq!(any_value_of(Value::from(true)), AnyValue::Boolean(true));
        assert_eq!(any_value_of(Value::from(7_i64)), AnyValue::Int(7));
        assert_eq!(any_value_of(Value::from(1.5_f64)), AnyValue::Double(1.5));
        assert_eq!(any_value_of(Value::from("hi")), AnyValue::String("hi".into()));
    }

    #[test]
    fn arrays_convert_to_any_value_lists() {
        assert_eq!(
            any_value_of(Value::from(vec![true, false])),
            AnyValue::ListAny(Box::new(vec![AnyValue::Boolean(true), AnyValue::Boolean(false)]))
        );
        assert_eq!(
            any_value_of(Value::from(vec![1_i64, 2])),
            AnyValue::ListAny(Box::new(vec![AnyValue::Int(1), AnyValue::Int(2)]))
        );
        assert_eq!(
            any_value_of(Value::from(vec![1.0_f64, 2.0])),
            AnyValue::ListAny(Box::new(vec![AnyValue::Double(1.0), AnyValue::Double(2.0)]))
        );
        assert_eq!(
            any_value_of(Value::from(vec![String::from("a"), String::from("b")])),
            AnyValue::ListAny(Box::new(vec![AnyValue::String("a".into()), AnyValue::String("b".into())]))
        );
    }

    #[test]
    fn scalars_convert_to_otel_value() {
        assert_eq!(otel_value_of(Value::from(true)), opentelemetry::Value::Bool(true));
        assert_eq!(otel_value_of(Value::from(7_i64)), opentelemetry::Value::I64(7));
        assert_eq!(otel_value_of(Value::from(1.5_f64)), opentelemetry::Value::F64(1.5));
        assert_eq!(otel_value_of(Value::from("hi")), opentelemetry::Value::String("hi".into()));
    }

    #[test]
    fn shared_text_converts_without_copying_representation() {
        assert_eq!(any_value_of(Value::from(String::from("owned"))), AnyValue::String("owned".into()));
        assert_eq!(
            otel_value_of(Value::from(String::from("owned"))),
            opentelemetry::Value::String("owned".into())
        );
    }

    #[test]
    fn arrays_convert_to_otel_arrays() {
        assert_eq!(
            otel_value_of(Value::from(vec![true, false])),
            opentelemetry::Value::Array(opentelemetry::Array::Bool(vec![true, false]))
        );
        assert_eq!(
            otel_value_of(Value::from(vec![1_i64, 2])),
            opentelemetry::Value::Array(opentelemetry::Array::I64(vec![1, 2]))
        );
        assert_eq!(
            otel_value_of(Value::from(vec![1.0_f64, 2.0])),
            opentelemetry::Value::Array(opentelemetry::Array::F64(vec![1.0, 2.0]))
        );
        assert_eq!(
            otel_value_of(Value::from(vec![String::from("a")])),
            opentelemetry::Value::Array(opentelemetry::Array::String(vec!["a".into()]))
        );
    }

    #[test]
    fn unsigned_values_convert() {
        assert_eq!(any_value_of(Value::from(7_u64)), AnyValue::Int(7));
        assert_eq!(otel_value_of(Value::from(7_u64)), opentelemetry::Value::I64(7));
    }

    #[test]
    fn unsigned_values_past_i64_max_convert_to_strings() {
        let big = u64::MAX;
        assert_eq!(any_value_of(Value::from(big)), AnyValue::String(big.to_string().into()));
        assert_eq!(
            otel_value_of(Value::from(big)),
            opentelemetry::Value::String(big.to_string().into())
        );
    }

    #[test]
    fn severities_convert() {
        use opentelemetry::logs::Severity as Otel;

        assert_eq!(otel_severity_of(Severity::Trace), Otel::Trace);
        assert_eq!(otel_severity_of(Severity::Debug), Otel::Debug);
        assert_eq!(otel_severity_of(Severity::Info), Otel::Info);
        assert_eq!(otel_severity_of(Severity::Warn), Otel::Warn);
        assert_eq!(otel_severity_of(Severity::Error), Otel::Error);
        assert_eq!(otel_severity_of(Severity::Fatal), Otel::Fatal);
    }
}
