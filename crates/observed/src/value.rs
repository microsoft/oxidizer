// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Attribute values for telemetry events.

use std::borrow::Cow;
use std::cell::RefCell;
use std::fmt;
use std::fmt::Write as _;
use std::sync::Arc;

use crate::text::Text;

/// An attribute value for telemetry events.
///
/// Construct one with the `From` impls (`Value::from(42)`), and read it back by
/// matching on the variants. Exporters match to translate a value into whatever
/// representation their wire format uses.
///
/// String values are stored as [`Text`], which borrows a `&'static str` or
/// shares an [`Arc<str>`] rather than copying. A non-`'static` `&str` has no
/// conversion on purpose - copying it must be spelled out at the call site as
/// `Arc::from(s)`.
///
/// # Integer conversions
///
/// An unredacted value keeps its own type: nothing is saturated, wrapped or
/// stringified on the way in, so a counter never reports a number the caller
/// did not pass.
///
/// | Source | Variant | Conversion |
/// |--------|---------|------------|
/// | `i8`, `i16`, `i32`, `i64` | [`I64`](Self::I64) | widening, lossless |
/// | `u8`, `u16`, `u32` | [`I64`](Self::I64) | widening, lossless |
/// | `u64` | [`U64`](Self::U64) | exact |
/// | `isize` | [`I64`](Self::I64) | exact on every supported target |
/// | `usize` | [`U64`](Self::U64) | exact on every supported target |
/// | `f32`, `f64` | [`F64`](Self::F64) | widening, lossless |
///
/// `u128` and `i128` have **no** conversion. No telemetry backend represents
/// them, so the only options would be truncating or stringifying a number the
/// caller believes was recorded; `#[event(...)]` rejects them at compile time
/// instead.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Value {
    /// A boolean.
    Bool(bool),
    /// A signed 64-bit integer.
    I64(i64),
    /// An unsigned 64-bit integer.
    U64(u64),
    /// A 64-bit float.
    F64(f64),
    /// A string.
    String(Text),
    /// A homogeneous array of booleans.
    BoolArray(Vec<bool>),
    /// A homogeneous array of signed 64-bit integers.
    I64Array(Vec<i64>),
    /// A homogeneous array of 64-bit floats.
    F64Array(Vec<f64>),
    /// A homogeneous array of strings.
    StringArray(Vec<Text>),
}

/// Capacity above which the redaction scratch buffer is released instead of
/// retained: one outsized field would otherwise pin that much memory per
/// thread for the rest of the process's life.
const MAX_RETAINED_BUFFER: usize = 4096;

thread_local! {
    /// Scratch buffer for rendering redacted values.
    ///
    /// Redaction emits its output through [`fmt::Write`], so the bytes must
    /// land somewhere before they can be shared as an [`Arc<str>`]. Reusing one
    /// buffer per thread keeps that step off the allocator: once it has grown,
    /// the only remaining allocation per field is the `Arc<str>` itself.
    static REDACTION_BUFFER: RefCell<String> = const { RefCell::new(String::new()) };
}

impl Value {
    /// Builds a string value by running a classified value through the
    /// redaction engine.
    ///
    /// This is the only way to turn a classified value into a `Value`, and it
    /// is what the derive macros generate for a classified field.
    ///
    /// A [`RedactedDisplay`](data_privacy::RedactedDisplay) implementation that
    /// fails part-way through has already written an unknown prefix of its
    /// output. That prefix was never approved by the redactor, so exporting it
    /// could widen what reaches telemetry; a failed redaction therefore yields
    /// the erased (empty) value rather than the partial text.
    pub fn from_redacted(value: &(impl data_privacy::RedactedDisplay + ?Sized), redactor: &dyn data_privacy::Redactor) -> Self {
        let rendered = fmt::from_fn(|f| data_privacy::RedactedDisplay::fmt(value, redactor, f));

        REDACTION_BUFFER.with(|cell| {
            // A `RedactedDisplay` impl may emit an event of its own, re-entering
            // this function while the buffer is already borrowed. That is too
            // rare to optimize for, but it must not panic - which rules out
            // `to_string`, since that panics when the impl returns an error.
            let Ok(mut buffer) = cell.try_borrow_mut() else {
                let mut scratch = String::new();
                if write!(&mut scratch, "{rendered}").is_err() {
                    scratch.clear();
                }
                return Self::String(Text::from(scratch));
            };

            buffer.clear();
            // Writing to a `String` cannot itself fail, but the redaction it
            // drives can. Discard whatever prefix was already emitted.
            if write!(&mut *buffer, "{rendered}").is_err() {
                buffer.clear();
            }

            let text = if buffer.is_empty() {
                // An erasing redactor writes nothing; no allocation can hold
                // fewer than zero bytes.
                Text::Static("")
            } else {
                Text::Shared(Arc::from(buffer.as_str()))
            };

            if buffer.capacity() > MAX_RETAINED_BUFFER {
                *buffer = String::new();
            }

            Self::String(text)
        })
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(v) => v.fmt(f),
            Self::I64(v) => v.fmt(f),
            Self::U64(v) => v.fmt(f),
            Self::F64(v) => v.fmt(f),
            Self::String(v) => v.fmt(f),
            Self::BoolArray(v) => fmt_array(f, v),
            Self::I64Array(v) => fmt_array(f, v),
            Self::F64Array(v) => fmt_array(f, v),
            Self::StringArray(v) => fmt_array(f, v),
        }
    }
}

fn fmt_array<T: fmt::Display>(f: &mut fmt::Formatter<'_>, values: &[T]) -> fmt::Result {
    f.write_str("[")?;
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            f.write_str(", ")?;
        }
        value.fmt(f)?;
    }
    f.write_str("]")
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Self::I64(v)
    }
}

impl From<f32> for Value {
    fn from(v: f32) -> Self {
        Self::F64(f64::from(v))
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Self::F64(v)
    }
}

impl From<Text> for Value {
    fn from(v: Text) -> Self {
        Self::String(v)
    }
}

impl From<&'static str> for Value {
    fn from(v: &'static str) -> Self {
        Self::String(Text::Static(v))
    }
}

impl From<Arc<str>> for Value {
    fn from(v: Arc<str>) -> Self {
        Self::String(Text::Shared(v))
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Self::String(v.into())
    }
}

impl From<Cow<'static, str>> for Value {
    fn from(v: Cow<'static, str>) -> Self {
        Self::String(v.into())
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Self::I64(i64::from(v))
    }
}

impl From<u32> for Value {
    fn from(v: u32) -> Self {
        Self::I64(i64::from(v))
    }
}

impl From<i8> for Value {
    fn from(v: i8) -> Self {
        Self::I64(i64::from(v))
    }
}

impl From<i16> for Value {
    fn from(v: i16) -> Self {
        Self::I64(i64::from(v))
    }
}

impl From<u8> for Value {
    fn from(v: u8) -> Self {
        Self::I64(i64::from(v))
    }
}

impl From<u16> for Value {
    fn from(v: u16) -> Self {
        Self::I64(i64::from(v))
    }
}

impl From<u64> for Value {
    fn from(v: u64) -> Self {
        Self::U64(v)
    }
}

impl From<usize> for Value {
    /// Exact on every target Rust supports, where `usize` is at most 64 bits.
    /// The saturating fallback exists only so a hypothetical wider target
    /// cannot make this panic.
    fn from(v: usize) -> Self {
        Self::U64(u64::try_from(v).unwrap_or(u64::MAX))
    }
}

impl From<isize> for Value {
    /// Exact on every target Rust supports, where `isize` is at most 64 bits.
    /// The saturating fallback exists only so a hypothetical wider target
    /// cannot make this panic.
    fn from(v: isize) -> Self {
        Self::I64(i64::try_from(v).unwrap_or(i64::MAX))
    }
}

impl From<Vec<bool>> for Value {
    fn from(v: Vec<bool>) -> Self {
        Self::BoolArray(v)
    }
}

impl From<Vec<i64>> for Value {
    fn from(v: Vec<i64>) -> Self {
        Self::I64Array(v)
    }
}

impl From<Vec<f64>> for Value {
    fn from(v: Vec<f64>) -> Self {
        Self::F64Array(v)
    }
}

impl From<Vec<Text>> for Value {
    fn from(v: Vec<Text>) -> Self {
        Self::StringArray(v)
    }
}

impl From<Vec<String>> for Value {
    fn from(v: Vec<String>) -> Self {
        Self::StringArray(v.into_iter().map(Text::from).collect())
    }
}

impl From<Vec<&'static str>> for Value {
    fn from(v: Vec<&'static str>) -> Self {
        Self::StringArray(v.into_iter().map(Text::from).collect())
    }
}

// NOTE: There is intentionally NO `From<Sensitive<V>> for Value`.
// `Sensitive` must always go through a `RedactionEngine` before becoming a `Value`.
// For enrichments, classified types use `EnrichmentEntry::new` via `RedactedDisplay`.
// For event fields, classified types use `Value::from_redacted` via `RedactedDisplay`.

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_from_bool() {
        assert_eq!(Value::from(true), Value::Bool(true));
    }

    #[test]
    fn value_from_i64() {
        assert_eq!(Value::from(42_i64), Value::I64(42));
    }

    #[test]
    fn value_from_i32() {
        assert_eq!(Value::from(7_i32), Value::I64(7));
    }

    #[test]
    fn value_from_u32() {
        assert_eq!(Value::from(99_u32), Value::I64(99));
    }

    #[test]
    fn value_from_f32() {
        assert_eq!(Value::from(1.5_f32), Value::F64(1.5));
    }

    #[test]
    fn value_from_f64() {
        assert_eq!(Value::from(2.72_f64), Value::F64(2.72));
    }

    #[test]
    fn value_from_static_str_is_borrowed() {
        assert_eq!(Value::from("hello"), Value::String(Text::Static("hello")));
    }

    #[test]
    fn value_from_string_is_shared() {
        assert_eq!(Value::from(String::from("world")), Value::String(Text::from("world")));
    }

    #[test]
    fn value_from_arc_str_preserves_pointer() {
        let arc: Arc<str> = Arc::from("shared");
        let ptr = Arc::as_ptr(&arc);
        let Value::String(Text::Shared(stored)) = Value::from(arc) else {
            panic!("expected a shared string value");
        };
        // The Arc's data pointer must survive into the Value without cloning.
        assert_eq!(Arc::as_ptr(&stored), ptr);
    }

    #[test]
    fn value_from_cow_keeps_borrowed_static() {
        assert_eq!(Value::from(Cow::Borrowed("literal")), Value::String(Text::Static("literal")));
    }

    #[test]
    fn value_from_arrays() {
        assert_eq!(Value::from(vec![true, false]), Value::BoolArray(vec![true, false]));
        assert_eq!(Value::from(vec![1_i64, 2]), Value::I64Array(vec![1, 2]));
        assert_eq!(Value::from(vec![1.0_f64, 2.0]), Value::F64Array(vec![1.0, 2.0]));
        assert_eq!(Value::from(vec![String::from("a")]), Value::StringArray(vec![Text::from("a")]));
    }

    #[test]
    fn value_redacted_is_a_string() {
        let engine = data_privacy::RedactionEngine::default();
        let classified = data_privacy::Sensitive::new("plain", data_privacy::DataClass::new("test", "unclassified"));
        let redacted = data_privacy::RedactedToString::to_redacted_string(&classified, &engine);
        assert!(matches!(Value::from(redacted), Value::String(_)));
    }

    #[test]
    fn from_redacted_matches_the_redacted_string() {
        let engine = data_privacy::RedactionEngine::builder()
            .set_fallback_redactor(data_privacy::simple_redactor::SimpleRedactor::new())
            .build();
        let classified = data_privacy::Sensitive::new("secret", data_privacy::DataClass::new("test", "unclassified"));

        assert_eq!(
            Value::from_redacted(&classified, &engine),
            Value::String(Text::from(data_privacy::RedactedToString::to_redacted_string(&classified, &engine)))
        );
    }

    #[test]
    fn from_redacted_accepts_a_bare_redactor() {
        // The parameter is `&dyn Redactor`, not `&RedactionEngine`, so a
        // standalone redaction strategy drives it without an engine wrapping.
        let redactor =
            data_privacy::simple_redactor::SimpleRedactor::with_mode(data_privacy::simple_redactor::SimpleRedactorMode::Passthrough);
        let classified = data_privacy::Sensitive::new("secret", data_privacy::DataClass::new("test", "unclassified"));

        assert_eq!(Value::from_redacted(&classified, &redactor), Value::String(Text::from("secret")));
    }

    #[test]
    fn from_redacted_erased_value_allocates_nothing() {
        // The default engine erases, so the redacted form is empty and is
        // stored without allocating an `Arc` to hold no bytes.
        let engine = data_privacy::RedactionEngine::default();
        let classified = data_privacy::Sensitive::new("secret", data_privacy::DataClass::new("test", "unclassified"));

        assert_eq!(Value::from_redacted(&classified, &engine), Value::String(Text::Static("")));
    }

    #[test]
    fn from_redacted_survives_a_reentrant_redaction() {
        // A `RedactedDisplay` impl that redacts another value re-enters
        // `from_redacted` while the scratch buffer is borrowed. The nested call
        // must fall back to its own allocation rather than panic.
        struct Reentrant;

        impl data_privacy::RedactedDisplay for Reentrant {
            fn fmt(&self, _redactor: &dyn data_privacy::Redactor, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let engine = data_privacy::RedactionEngine::builder()
                    .set_fallback_redactor(data_privacy::simple_redactor::SimpleRedactor::new())
                    .build();
                let inner = data_privacy::Sensitive::new("abc", data_privacy::DataClass::new("test", "unclassified"));
                write!(f, "{}", Value::from_redacted(&inner, &engine))
            }
        }

        let engine = data_privacy::RedactionEngine::builder()
            .set_fallback_redactor(data_privacy::simple_redactor::SimpleRedactor::with_mode(
                data_privacy::simple_redactor::SimpleRedactorMode::Passthrough,
            ))
            .build();

        assert_eq!(Value::from_redacted(&Reentrant, &engine), Value::String(Text::from("***")));
    }

    #[test]
    fn from_redacted_erases_a_value_whose_redaction_fails() {
        // A `RedactedDisplay` impl can fail after writing part of its output.
        // That prefix was never approved by the redactor, so exporting it could
        // widen what reaches telemetry: the value must be erased instead.
        struct FailsMidway;

        impl data_privacy::RedactedDisplay for FailsMidway {
            fn fmt(&self, _redactor: &dyn data_privacy::Redactor, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("prefix-")?;
                Err(fmt::Error)
            }
        }

        let engine = data_privacy::RedactionEngine::builder()
            .set_fallback_redactor(data_privacy::simple_redactor::SimpleRedactor::with_mode(
                data_privacy::simple_redactor::SimpleRedactorMode::Passthrough,
            ))
            .build();

        assert_eq!(Value::from_redacted(&FailsMidway, &engine), Value::String(Text::Static("")));
    }

    #[test]
    fn from_redacted_erases_a_value_whose_reentrant_redaction_fails() {
        // The two edge cases together: the scratch buffer is already borrowed
        // by an outer call, so the nested call allocates its own, AND that
        // nested redaction fails part-way. The erase-on-failure rule has to
        // hold on the fallback path too, otherwise the partial text escapes
        // through exactly the route the buffer contention opens up.
        struct FailsMidway;

        impl data_privacy::RedactedDisplay for FailsMidway {
            fn fmt(&self, _redactor: &dyn data_privacy::Redactor, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("prefix-")?;
                Err(fmt::Error)
            }
        }

        struct ReentrantFailure;

        impl data_privacy::RedactedDisplay for ReentrantFailure {
            fn fmt(&self, _redactor: &dyn data_privacy::Redactor, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let engine = data_privacy::RedactionEngine::builder()
                    .set_fallback_redactor(data_privacy::simple_redactor::SimpleRedactor::with_mode(
                        data_privacy::simple_redactor::SimpleRedactorMode::Passthrough,
                    ))
                    .build();
                write!(f, "[{}]", Value::from_redacted(&FailsMidway, &engine))
            }
        }

        let engine = data_privacy::RedactionEngine::builder()
            .set_fallback_redactor(data_privacy::simple_redactor::SimpleRedactor::with_mode(
                data_privacy::simple_redactor::SimpleRedactorMode::Passthrough,
            ))
            .build();

        assert_eq!(Value::from_redacted(&ReentrantFailure, &engine), Value::String(Text::from("[]")),);
    }

    #[test]
    fn from_redacted_releases_an_outsized_scratch_buffer() {
        let engine = data_privacy::RedactionEngine::builder()
            .set_fallback_redactor(data_privacy::simple_redactor::SimpleRedactor::new())
            .build();
        let long = "x".repeat(MAX_RETAINED_BUFFER + 1);
        let classified = data_privacy::Sensitive::new(long.as_str(), data_privacy::DataClass::new("test", "unclassified"));

        let value = Value::from_redacted(&classified, &engine);
        assert_eq!(value.to_string().len(), long.len());
        REDACTION_BUFFER.with(|cell| assert_eq!(cell.borrow().capacity(), 0));
    }

    #[test]
    fn from_redacted_retains_a_buffer_at_the_retention_limit() {
        // The limit is inclusive: a buffer grown to exactly `MAX_RETAINED_BUFFER`
        // is kept for reuse, and only a larger one is released.
        let engine = data_privacy::RedactionEngine::builder()
            .set_fallback_redactor(data_privacy::simple_redactor::SimpleRedactor::new())
            .build();
        let exact = "x".repeat(MAX_RETAINED_BUFFER);
        let classified = data_privacy::Sensitive::new(exact.as_str(), data_privacy::DataClass::new("test", "unclassified"));

        let value = Value::from_redacted(&classified, &engine);
        assert_eq!(value.to_string().len(), exact.len());
        REDACTION_BUFFER.with(|cell| assert_eq!(cell.borrow().capacity(), MAX_RETAINED_BUFFER));
    }

    #[test]
    fn value_from_text_is_stored_as_is() {
        assert_eq!(Value::from(Text::Static("kept")), Value::String(Text::Static("kept")));
    }

    #[test]
    fn value_from_text_arrays() {
        assert_eq!(Value::from(vec![Text::Static("a")]), Value::StringArray(vec![Text::Static("a")]));
        assert_eq!(Value::from(vec!["b"]), Value::StringArray(vec![Text::Static("b")]));
    }

    #[test]
    fn value_display_delegates() {
        assert_eq!(Value::from(42_i64).to_string(), "42");
        assert_eq!(Value::from(true).to_string(), "true");
        assert_eq!(Value::from("hi").to_string(), "hi");
        assert_eq!(Value::from(vec![1_i64, 2]).to_string(), "[1, 2]");
    }

    #[test]
    fn value_display_covers_every_variant() {
        assert_eq!(Value::from(1.5_f64).to_string(), "1.5");
        assert_eq!(Value::from(vec![true, false]).to_string(), "[true, false]");
        assert_eq!(Value::from(vec![1.5_f64, 2.5]).to_string(), "[1.5, 2.5]");
        assert_eq!(Value::from(vec![Text::Static("a"), Text::Static("b")]).to_string(), "[a, b]");
    }
}

#[cfg(test)]
mod integer_conversion_tests {
    use super::*;

    #[test]
    fn small_integers_widen_losslessly_into_i64() {
        assert_eq!(Value::from(-1_i8), Value::I64(-1));
        assert_eq!(Value::from(i8::MIN), Value::I64(i64::from(i8::MIN)));
        assert_eq!(Value::from(-1_i16), Value::I64(-1));
        assert_eq!(Value::from(i16::MIN), Value::I64(i64::from(i16::MIN)));
        assert_eq!(Value::from(255_u8), Value::I64(255));
        assert_eq!(Value::from(u16::MAX), Value::I64(i64::from(u16::MAX)));
    }

    #[test]
    fn u64_keeps_its_own_variant() {
        // The whole point of `U64`: more than half of `u64`'s range does not
        // fit in `i64`, and byte counters live in that half.
        assert_eq!(Value::from(7_u64), Value::U64(7));
        assert_eq!(Value::from(u64::MAX), Value::U64(u64::MAX));

        // `U64` and `I64` are distinct even where the numbers agree, so a
        // matcher cannot confuse the signedness of what it is exporting.
        assert_ne!(Value::from(7_u64), Value::from(7_i64));
    }

    #[test]
    fn pointer_sized_integers_are_exact() {
        assert_eq!(Value::from(12_usize), Value::U64(12));
        assert_eq!(Value::from(-12_isize), Value::I64(-12));
        assert_eq!(Value::from(usize::MAX), Value::U64(usize::MAX as u64));
        assert_eq!(Value::from(isize::MAX), Value::I64(isize::MAX as i64));
    }

    #[test]
    fn integer_values_display_as_plain_numbers() {
        // Exporters that fall back to `Display` must not see a variant name.
        assert_eq!(Value::from(u64::MAX).to_string(), u64::MAX.to_string());
        assert_eq!(Value::from(-3_i8).to_string(), "-3");
    }
}
