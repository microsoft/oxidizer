// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Human-readable rendering of an `OpenTelemetry` [`AnyValue`].
//!
//! `AnyValue` only derives `Debug`, so printing one produces wrapper noise such
//! as `String(Owned("hello"))`. [`format_any_value`] returns a [`fmt::Display`]
//! adapter that renders the value itself, recursing through lists and maps.
//! Every arm writes straight to the formatter, so rendering allocates nothing
//! and has no side effects.

use std::fmt;

use opentelemetry::logs::AnyValue;

/// Formats an [`AnyValue`] in a human-readable way.
///
/// `AnyValue` only derives `Debug`, which produces noisy output like
/// `String(Owned("hello"))`. This helper renders values cleanly:
///
/// | Variant | Output |
/// |---------|--------|
/// | `Boolean` | `true` / `false` |
/// | `Int` | `42` |
/// | `Double` | `3.14` |
/// | `String` | `hello` (no quotes, no wrapper) |
/// | `Bytes` | `01abff` (raw hex string) |
/// | `ListAny` | `[elem1, elem2, ...]` (recursive) |
/// | `Map` | `{key1: val1, key2: val2, ...}` (recursive) |
///
/// # Examples
///
/// ```
/// use observed_utils::format_any_value;
/// use opentelemetry::logs::AnyValue;
///
/// let v = AnyValue::String("hello world".into());
/// assert_eq!(format_any_value(&v).to_string(), "hello world");
///
/// let v = AnyValue::Int(42);
/// assert_eq!(format_any_value(&v).to_string(), "42");
///
/// let v = AnyValue::Boolean(true);
/// assert_eq!(format_any_value(&v).to_string(), "true");
/// ```
#[must_use]
pub fn format_any_value(value: &AnyValue) -> impl fmt::Display + '_ {
    DisplayAnyValue(value)
}

struct DisplayAnyValue<'a>(&'a AnyValue);

impl DisplayAnyValue<'_> {
    // The last arm guards the `#[non_exhaustive]` `AnyValue`, so it cannot be
    // reached by any variant that exists today, and coverage instrumentation
    // counts an arm that is never taken as an uncovered line. The whole match
    // is therefore excluded from the coverage gate rather than the guard being
    // deleted: without it, a variant added upstream would either fail to
    // compile here or silently lose data.
    //
    // The exclusion covers the scalar and `Bytes` arms too, since they render
    // inline: those are pinned by `format_int` / `format_bool` / `format_string`
    // / `format_bytes` and by mutation testing, which still applies here, but
    // not by the coverage gate. Only the list and map rendering, in
    // `write_list` / `write_map` below, stays measured - and each nested element
    // re-enters this excluded dispatch.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt_value(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            AnyValue::Int(v) => write!(f, "{v}"),
            AnyValue::Double(v) => write!(f, "{v}"),
            AnyValue::String(v) => write!(f, "{v}"),
            AnyValue::Boolean(v) => write!(f, "{v}"),
            AnyValue::Bytes(v) => write!(f, "{}", const_hex::display(v.as_slice())),
            AnyValue::ListAny(v) => write_list(f, v.as_slice()),
            AnyValue::Map(v) => write_map(f, v.iter()),
            other => write!(f, "{other:?}"),
        }
    }
}

impl fmt::Display for DisplayAnyValue<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_value(f)
    }
}

/// Renders a list as `[elem1, elem2, ...]`.
fn write_list(f: &mut fmt::Formatter<'_>, items: &[AnyValue]) -> fmt::Result {
    f.write_str("[")?;
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        DisplayAnyValue(item).fmt_value(f)?;
    }
    f.write_str("]")
}

/// Renders a map as `{key1: val1, key2: val2, ...}`.
fn write_map<'a, K: fmt::Display + 'a>(f: &mut fmt::Formatter<'_>, entries: impl Iterator<Item = (&'a K, &'a AnyValue)>) -> fmt::Result {
    f.write_str("{")?;
    for (i, (key, val)) in entries.enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{key}: ")?;
        DisplayAnyValue(val).fmt_value(f)?;
    }
    f.write_str("}")
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use opentelemetry::Key;

    use super::*;

    #[test]
    fn format_string() {
        let v = AnyValue::String("hello world".into());
        assert_eq!(format_any_value(&v).to_string(), "hello world");
    }

    #[test]
    fn format_int() {
        let v = AnyValue::Int(42);
        assert_eq!(format_any_value(&v).to_string(), "42");
    }

    #[test]
    fn format_double() {
        let v = AnyValue::Double(2.72);
        assert_eq!(format_any_value(&v).to_string(), "2.72");
    }

    #[test]
    fn format_bool() {
        let v = AnyValue::Boolean(true);
        assert_eq!(format_any_value(&v).to_string(), "true");
    }

    #[test]
    fn format_bytes() {
        let v = AnyValue::Bytes(Box::new(vec![0x01, 0xab, 0xff]));
        assert_eq!(format_any_value(&v).to_string(), "01abff");
    }

    #[test]
    fn format_list() {
        let v = AnyValue::ListAny(Box::new(vec![
            AnyValue::Int(1),
            AnyValue::String("two".into()),
            AnyValue::Boolean(false),
        ]));
        assert_eq!(format_any_value(&v).to_string(), "[1, two, false]");
    }

    #[test]
    fn format_empty_list() {
        let v = AnyValue::ListAny(Box::default());
        assert_eq!(format_any_value(&v).to_string(), "[]");
    }

    #[test]
    fn format_map() {
        let mut map = HashMap::new();
        map.insert(Key::from("a"), AnyValue::Int(1));
        map.insert(Key::from("b"), AnyValue::String("two".into()));
        let v = AnyValue::Map(Box::new(map));

        // `HashMap` iteration order is unspecified, so compare the rendered
        // entries as a set while still asserting on the delimiter shape.
        let rendered = format_any_value(&v).to_string();
        let inner = rendered
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
            .expect("map renders inside braces");
        let mut entries: Vec<&str> = inner.split(", ").collect();
        entries.sort_unstable();
        assert_eq!(entries, ["a: 1", "b: two"]);
    }

    #[test]
    fn format_empty_map() {
        let v = AnyValue::Map(Box::default());
        assert_eq!(format_any_value(&v).to_string(), "{}");
    }

    #[test]
    fn format_nested_map_in_list() {
        let mut map = HashMap::new();
        map.insert(Key::from("k"), AnyValue::Boolean(true));
        let v = AnyValue::ListAny(Box::new(vec![AnyValue::Map(Box::new(map))]));
        assert_eq!(format_any_value(&v).to_string(), "[{k: true}]");
    }
}
