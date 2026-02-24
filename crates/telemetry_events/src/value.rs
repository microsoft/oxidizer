use std::time::Duration;

use data_privacy::{RedactedDisplay, RedactedToString, RedactionEngine};
use opentelemetry::Value;
use opentelemetry::logs::AnyValue;

pub struct TelemetrySafeValue(TelemetrySafeValueInner);

impl TelemetrySafeValue {
    pub fn from_redacted<T: RedactedDisplay>(value: &T, redaction_engine: &RedactionEngine) -> Self {
        Self(TelemetrySafeValueInner::RedactedString(value.to_redacted_string(redaction_engine)))
    }

    pub fn to_number(&self) -> Option<f64> {
        match self.0 {
            TelemetrySafeValueInner::I64(i) => Some(i as f64),
            TelemetrySafeValueInner::F64(f) => Some(f),
            TelemetrySafeValueInner::RedactedString(_) => None,
        }
    }
}

impl From<i64> for TelemetrySafeValue {
    fn from(value: i64) -> Self {
        Self(TelemetrySafeValueInner::I64(value))
    }
}

impl From<f64> for TelemetrySafeValue {
    fn from(value: f64) -> Self {
        Self(TelemetrySafeValueInner::F64(value))
    }
}

impl From<Duration> for TelemetrySafeValue {
    fn from(value: Duration) -> Self {
        Self(TelemetrySafeValueInner::F64(value.as_secs_f64()))
    }
}

impl Into<Value> for TelemetrySafeValue {
    #[inline(always)]
    fn into(self) -> Value {
        match self.0 {
            TelemetrySafeValueInner::I64(i) => Value::I64(i),
            TelemetrySafeValueInner::F64(f) => Value::F64(f),
            TelemetrySafeValueInner::RedactedString(s) => Value::String(s.into()),
        }
    }
}

impl Into<AnyValue> for TelemetrySafeValue {
    #[inline(always)]
    fn into(self) -> AnyValue {
        match self.0 {
            TelemetrySafeValueInner::I64(i) => AnyValue::Int(i),
            TelemetrySafeValueInner::F64(f) => AnyValue::Double(f),
            TelemetrySafeValueInner::RedactedString(s) => AnyValue::String(s.into()),
        }
    }
}

enum TelemetrySafeValueInner {
    I64(i64),
    F64(f64),
    RedactedString(String),
}
