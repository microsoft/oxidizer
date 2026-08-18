// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

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
/// use observed_helpers::format_any_value;
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

impl fmt::Display for DisplayAnyValue<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            AnyValue::Int(v) => write!(f, "{v}"),
            AnyValue::Double(v) => write!(f, "{v}"),
            AnyValue::String(v) => write!(f, "{v}"),
            AnyValue::Boolean(v) => write!(f, "{v}"),
            AnyValue::Bytes(v) => f.write_str(&const_hex::encode(v.as_slice())),
            AnyValue::ListAny(v) => {
                f.write_str("[")?;
                for (i, item) in v.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    DisplayAnyValue(item).fmt(f)?;
                }
                f.write_str("]")
            }
            AnyValue::Map(v) => {
                f.write_str("{")?;
                for (i, (key, val)) in v.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{key}: ")?;
                    DisplayAnyValue(val).fmt(f)?;
                }
                f.write_str("}")
            }
            other => write!(f, "{other:?}"),
        }
    }
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
