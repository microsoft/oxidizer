// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Numeric extraction for metric recording.

use observed::Value;

/// Returns the value as an `f64` suitable for recording against a metric
/// instrument, or `None` when the value is not numeric (strings, booleans,
/// arrays).
///
/// [`Value::U64`] is included: `u64` is the natural type for byte and request
/// counters, so omitting it here would make those instruments record nothing.
#[must_use]
pub fn metric_number_of(value: &Value) -> Option<f64> {
    match value {
        Value::I64(i) =>
        {
            #[expect(clippy::cast_precision_loss, reason = "metric recording precision loss is acceptable")]
            Some(*i as f64)
        }
        Value::U64(u) =>
        {
            #[expect(clippy::cast_precision_loss, reason = "metric recording precision loss is acceptable")]
            Some(*u as f64)
        }
        Value::F64(f) => Some(*f),
        _ => None,
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_values_convert() {
        assert_eq!(metric_number_of(&Value::from(42_i64)), Some(42.0));
        assert_eq!(metric_number_of(&Value::from(7_u64)), Some(7.0));
        assert_eq!(metric_number_of(&Value::from(2.72_f64)), Some(2.72));
    }

    #[test]
    fn non_numeric_values_are_none() {
        assert_eq!(metric_number_of(&Value::from("hello")), None);
        assert_eq!(metric_number_of(&Value::from(true)), None);
        assert_eq!(metric_number_of(&Value::from(vec![1_i64])), None);
    }
}
