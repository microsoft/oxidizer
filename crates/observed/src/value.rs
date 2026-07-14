// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::sync::Arc;

use opentelemetry::logs::AnyValue;

/// An attribute value for telemetry events.
///
/// Thin wrapper over [`opentelemetry::Value`] that provides ergonomic
/// conversions from common Rust types.
#[derive(Debug, Clone, PartialEq)]
pub struct Value(opentelemetry::Value);

impl fmt::Display for Value {
    /// Delegates to [`opentelemetry::Value`]'s `Display` implementation.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Value {
    /// Constructs a `Value` from a raw [`opentelemetry::Value`].
    ///
    /// Useful for constructing array values or other complex types
    /// that don't have direct `From` impls.
    #[must_use]
    pub fn from_raw(value: opentelemetry::Value) -> Self {
        Self(value)
    }

    /// Returns a reference to the inner [`opentelemetry::Value`].
    #[must_use]
    pub fn as_inner(&self) -> &opentelemetry::Value {
        &self.0
    }

    /// Consumes this value and returns the inner [`opentelemetry::Value`].
    #[must_use]
    pub fn into_inner(self) -> opentelemetry::Value {
        self.0
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Self(opentelemetry::Value::Bool(v))
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Self(opentelemetry::Value::I64(v))
    }
}

impl From<f32> for Value {
    fn from(v: f32) -> Self {
        Self(opentelemetry::Value::F64(f64::from(v)))
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Self(opentelemetry::Value::F64(v))
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Self(opentelemetry::Value::String(v.into()))
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Self(opentelemetry::Value::String(v.to_owned().into()))
    }
}

impl From<Arc<str>> for Value {
    fn from(v: Arc<str>) -> Self {
        Self(opentelemetry::Value::String(v.into()))
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Self(opentelemetry::Value::I64(i64::from(v)))
    }
}

impl From<u32> for Value {
    fn from(v: u32) -> Self {
        Self(opentelemetry::Value::I64(i64::from(v)))
    }
}

impl From<opentelemetry::StringValue> for Value {
    fn from(v: opentelemetry::StringValue) -> Self {
        Self(opentelemetry::Value::String(v))
    }
}

// NOTE: There is intentionally NO `From<Sensitive<V>> for Value`.
// `Sensitive` must always go through a `RedactionEngine` before becoming a `Value`.
// For enrichments, classified types use `EnrichmentEntry::new` via `RedactedDisplay`.
// For event fields, classified types use `Value::from_redacted` via `RedactedDisplay`.

impl Value {
    /// Creates a `Value` by running a classified value through the redaction engine.
    ///
    /// This is the only way to create a string `Value` from a classified type -
    /// it must go through [`RedactedDisplay`](data_privacy::RedactedDisplay).
    /// The derive macro generates calls to this for non-primitive fields.
    pub fn from_redacted(value: &(impl data_privacy::RedactedDisplay + ?Sized), engine: &data_privacy::RedactionEngine) -> Self {
        Self(opentelemetry::Value::String(
            data_privacy::RedactedToString::to_redacted_string(value, engine).into(),
        ))
    }

    /// Returns the value as a string slice if it is a string type.
    ///
    /// Used internally for redaction - classified enrichment values that are
    /// strings get redacted through the [`data_privacy::RedactionEngine`].
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match &self.0 {
            opentelemetry::Value::String(s) => Some(s.as_ref()),
            _ => None,
        }
    }

    /// Returns the value as an `f64` if it is numeric. Returns `None` for
    /// non-numeric types (strings, bools, arrays).
    #[must_use]
    pub fn to_number(&self) -> Option<f64> {
        match &self.0 {
            opentelemetry::Value::I64(i) =>
            {
                #[expect(clippy::cast_precision_loss, reason = "metric recording precision loss is acceptable")]
                Some(*i as f64)
            }
            opentelemetry::Value::F64(f) => Some(*f),
            _ => None,
        }
    }
}

// Required at the OTel log boundary: `LogRecord::add_attribute` takes `AnyValue`,
// while the metric path uses `opentelemetry::Value`. Both conversions are needed.
impl From<Value> for AnyValue {
    fn from(v: Value) -> Self {
        match v.0 {
            opentelemetry::Value::Bool(b) => Self::Boolean(b),
            opentelemetry::Value::I64(i) => Self::Int(i),
            opentelemetry::Value::F64(f) => Self::Double(f),
            opentelemetry::Value::String(s) => Self::String(s),
            opentelemetry::Value::Array(a) => match a {
                opentelemetry::Array::Bool(v) => Self::ListAny(Box::new(v.into_iter().map(AnyValue::Boolean).collect())),
                opentelemetry::Array::I64(v) => Self::ListAny(Box::new(v.into_iter().map(AnyValue::Int).collect())),
                opentelemetry::Array::F64(v) => Self::ListAny(Box::new(v.into_iter().map(AnyValue::Double).collect())),
                opentelemetry::Array::String(v) => Self::ListAny(Box::new(v.into_iter().map(AnyValue::String).collect())),
                _ => Self::String("<unsupported array type>".into()),
            },
            _ => Self::String("<unsupported value type>".into()),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_from_bool() {
        let v = Value::from(true);
        assert_eq!(*v.as_inner(), opentelemetry::Value::Bool(true));
    }

    #[test]
    fn value_from_i64() {
        let v = Value::from(42_i64);
        assert_eq!(*v.as_inner(), opentelemetry::Value::I64(42));
    }

    #[test]
    fn value_from_i32() {
        let v = Value::from(7_i32);
        assert_eq!(*v.as_inner(), opentelemetry::Value::I64(7));
    }

    #[test]
    fn value_from_u32() {
        let v = Value::from(99_u32);
        assert_eq!(*v.as_inner(), opentelemetry::Value::I64(99));
    }

    #[test]
    fn value_from_f32() {
        let v = Value::from(1.5_f32);
        assert_eq!(*v.as_inner(), opentelemetry::Value::F64(1.5));
    }

    #[test]
    fn value_from_f64() {
        let v = Value::from(2.72_f64);
        assert_eq!(*v.as_inner(), opentelemetry::Value::F64(2.72));
    }

    #[test]
    fn value_from_str() {
        let v = Value::from("hello");
        assert_eq!(v.as_str(), Some("hello"));
    }

    #[test]
    fn value_from_string() {
        let v = Value::from(String::from("world"));
        assert_eq!(v.as_str(), Some("world"));
    }

    #[test]
    fn value_from_arc_str_preserves_pointer() {
        let arc: Arc<str> = Arc::from("shared");
        let ptr = Arc::as_ptr(&arc);
        let v = Value::from(arc);
        // The Arc's data pointer must survive into the Value without cloning.
        assert_eq!(v.as_str().unwrap().as_ptr(), ptr.cast::<u8>());
    }

    #[test]
    fn value_to_number() {
        let v = Value::from(42_i64);
        assert_eq!(v.to_number(), Some(42.0));

        let v = Value::from(2.72_f64);
        assert_eq!(v.to_number(), Some(2.72));

        let v = Value::from("hello");
        assert_eq!(v.to_number(), None);
    }

    #[test]
    fn value_display_delegates() {
        assert_eq!(Value::from(42_i64).to_string(), "42");
        assert_eq!(Value::from(true).to_string(), "true");
        assert_eq!(Value::from("hi").to_string(), "hi");
    }

    #[test]
    fn value_into_any_value_scalars() {
        assert_eq!(AnyValue::from(Value::from(true)), AnyValue::Boolean(true));
        assert_eq!(AnyValue::from(Value::from(7_i64)), AnyValue::Int(7));
        assert_eq!(AnyValue::from(Value::from(1.5_f64)), AnyValue::Double(1.5));
        assert_eq!(AnyValue::from(Value::from("hi")), AnyValue::String("hi".into()));
    }

    #[test]
    fn value_into_any_value_arrays() {
        let bools = Value::from_raw(opentelemetry::Value::Array(opentelemetry::Array::Bool(vec![true, false])));
        assert_eq!(
            AnyValue::from(bools),
            AnyValue::ListAny(Box::new(vec![AnyValue::Boolean(true), AnyValue::Boolean(false)]))
        );

        let ints = Value::from_raw(opentelemetry::Value::Array(opentelemetry::Array::I64(vec![1, 2])));
        assert_eq!(
            AnyValue::from(ints),
            AnyValue::ListAny(Box::new(vec![AnyValue::Int(1), AnyValue::Int(2)]))
        );

        let floats = Value::from_raw(opentelemetry::Value::Array(opentelemetry::Array::F64(vec![1.0, 2.0])));
        assert_eq!(
            AnyValue::from(floats),
            AnyValue::ListAny(Box::new(vec![AnyValue::Double(1.0), AnyValue::Double(2.0)]))
        );

        let strings = Value::from_raw(opentelemetry::Value::Array(opentelemetry::Array::String(vec![
            "a".into(),
            "b".into(),
        ])));
        assert_eq!(
            AnyValue::from(strings),
            AnyValue::ListAny(Box::new(vec![AnyValue::String("a".into()), AnyValue::String("b".into())]))
        );
    }
}
