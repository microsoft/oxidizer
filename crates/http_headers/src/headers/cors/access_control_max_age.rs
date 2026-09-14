// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::time::Duration;

use super::shared::{invalid_number, invalid_syntax, trimmed_range, untrimmed_range};
use crate::sink::{EncodedValues, FieldSink, InsertError};
use crate::source::FieldSource;
use crate::{DecodeError, Field, FieldName, FieldValue, FieldValueRef, validate};

/// Defines the `Access-Control-Max-Age` header.
///
/// # Specification
///
/// Defined by the Fetch standard's
/// [CORS protocol and credentials section](https://fetch.spec.whatwg.org/#http-access-control-max-age).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{AccessControlMaxAge, AccessControlMaxAgeOwned};
///
/// let mut map = HeaderMap::new();
/// AccessControlMaxAge::insert(&mut map, AccessControlMaxAgeOwned::new(600))?;
/// assert!(AccessControlMaxAge::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct AccessControlMaxAge {
    _private: (),
}

/// Owned value for the `Access-Control-Max-Age` header.
///
/// The field value carries nothing but a delta-seconds count, so that count is
/// all this type keeps: leading zeroes and surrounding whitespace are accepted
/// when decoding and dropped, and encoding renders the canonical decimal.
///
/// # Specification
///
/// Defined by the Fetch standard's [CORS protocol and credentials section].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::AccessControlMaxAgeOwned::new(600);
/// assert_eq!(value.seconds(), 600);
/// ```
///
/// `Access-Control-Max-Age: 600` permits caching for ten minutes, while
/// `Access-Control-Max-Age: 0` disables reuse. Decoding ` 00600 ` yields the
/// same header as decoding `600`.
///
/// [CORS protocol and credentials section]: https://fetch.spec.whatwg.org/#http-access-control-max-age
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AccessControlMaxAgeOwned {
    seconds: u64,
}

impl AccessControlMaxAgeOwned {
    /// Constructs a max age in seconds.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::AccessControlMaxAgeOwned;
    ///
    /// let value = AccessControlMaxAgeOwned::new(600);
    /// assert_eq!(value.seconds(), 600);
    /// assert_eq!(value.to_string(), "600");
    /// ```
    pub const fn new(seconds: u64) -> Self {
        Self { seconds }
    }

    /// Constructs a max age from a duration.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use http_headers::headers::AccessControlMaxAgeOwned;
    ///
    /// let value = AccessControlMaxAgeOwned::from_duration(Duration::from_millis(2_500));
    /// assert_eq!(value.seconds(), 2);
    /// assert_eq!(value.duration(), Duration::from_secs(2));
    /// ```
    pub const fn from_duration(duration: Duration) -> Self {
        Self {
            seconds: duration.as_secs(),
        }
    }

    /// Returns the max age in seconds.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::AccessControlMaxAgeOwned;
    ///
    /// let value = AccessControlMaxAgeOwned::new(600);
    /// assert_eq!(value.seconds(), 600);
    ///
    /// let disabled = AccessControlMaxAgeOwned::new(0);
    /// assert_eq!(disabled.seconds(), 0);
    /// ```
    #[expect(clippy::trivially_copy_pass_by_ref, reason = "accessors consistently borrow owned header values")]
    pub const fn seconds(&self) -> u64 {
        self.seconds
    }

    /// Returns the max age as a duration.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use http_headers::headers::AccessControlMaxAgeOwned;
    ///
    /// let value = AccessControlMaxAgeOwned::new(600);
    /// assert_eq!(value.duration(), Duration::from_secs(600));
    /// ```
    #[expect(clippy::trivially_copy_pass_by_ref, reason = "accessors consistently borrow owned header values")]
    pub const fn duration(&self) -> Duration {
        Duration::from_secs(self.seconds)
    }

    /// Renders the canonical field value.
    ///
    /// Decoding does not keep the bytes it read, so a value that carried
    /// leading zeroes or surrounding whitespace comes back without them.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::AccessControlMaxAgeOwned;
    ///
    /// let value = AccessControlMaxAgeOwned::try_from(" 00600 ")?;
    /// let field_value = value.into_field_value();
    /// assert_eq!(field_value.as_bytes(), b"600");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }
}

super::super::shared::impl_field_value_conversion!(AccessControlMaxAgeOwned, |value| FieldValue::from(value.seconds));

impl fmt::Display for AccessControlMaxAgeOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.seconds.fmt(f)
    }
}
/// Reads the delta-seconds a `Access-Control-Max-Age` field value holds.
fn max_age_seconds(value: FieldValueRef<'_>) -> Result<u64, DecodeError> {
    let all = value.as_bytes();
    let range = untrimmed_range(all).unwrap_or_else(|| trimmed_range(all));
    let bytes = &all[range];
    validate::decimal_u64(bytes).ok_or_else(|| invalid_number(&FieldName::AccessControlMaxAge))
}

impl Field for AccessControlMaxAge {
    type View<'a> = AccessControlMaxAgeOwned;
    type Owned = AccessControlMaxAgeOwned;

    fn name() -> &'static FieldName {
        &FieldName::AccessControlMaxAge
    }

    /// Reads the count directly, since the field value carries nothing a
    /// borrowed form could retain that the count does not already capture.
    fn view_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        lines.validate_custom_source()?;
        let seconds = max_age_seconds(lines.exactly_one()?)?;
        Ok(Some(AccessControlMaxAgeOwned { seconds }))
    }

    /// Reads the count without building a view, since the owned form keeps
    /// nothing else.
    fn owned_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        lines.validate_custom_source()?;
        let seconds = max_age_seconds(lines.exactly_one()?)?;
        Ok(Some(AccessControlMaxAgeOwned { seconds }))
    }

    fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        sink.set_values(Self::name(), EncodedValues::single(FieldValue::from(value.seconds)))
    }
}

impl From<Duration> for AccessControlMaxAgeOwned {
    fn from(duration: Duration) -> Self {
        Self::from_duration(duration)
    }
}

impl From<AccessControlMaxAgeOwned> for Duration {
    fn from(value: AccessControlMaxAgeOwned) -> Self {
        value.duration()
    }
}

impl TryFrom<&str> for AccessControlMaxAgeOwned {
    type Error = DecodeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = FieldValue::from_str(value).map_err(|_invalid| invalid_syntax(&FieldName::AccessControlMaxAge))?;
        Self::try_from(value)
    }
}

impl TryFrom<String> for AccessControlMaxAgeOwned {
    type Error = DecodeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let value = FieldValue::try_from(value).map_err(|_invalid| invalid_syntax(&FieldName::AccessControlMaxAge))?;
        Self::try_from(value)
    }
}

impl TryFrom<FieldValue> for AccessControlMaxAgeOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        let seconds = max_age_seconds(value.as_field_value_ref())?;
        Ok(Self { seconds })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use super::{AccessControlMaxAge, AccessControlMaxAgeOwned};
    use crate::headers::cors::test_support::TestMap;
    use crate::{DecodeErrorKind, FieldName, FieldValue};

    #[test]
    fn constructors_accessors_display_and_conversions_are_canonical() {
        let value = AccessControlMaxAgeOwned::new(600);
        assert_eq!(value.seconds(), 600);
        assert_eq!(value.duration(), Duration::from_mins(10));
        assert_eq!(value.to_string(), "600");
        assert_eq!(value.into_field_value(), "600");

        let from_duration = AccessControlMaxAgeOwned::from_duration(Duration::from_millis(2_500));
        assert_eq!(from_duration.seconds(), 2);
        assert_eq!(
            AccessControlMaxAgeOwned::from(Duration::from_secs(9)),
            AccessControlMaxAgeOwned::new(9)
        );
        assert_eq!(Duration::from(AccessControlMaxAgeOwned::new(7)), Duration::from_secs(7));

        assert_eq!(
            AccessControlMaxAgeOwned::try_from(" 00600 ").expect("whitespace and leading zeroes"),
            value
        );
        assert_eq!(
            AccessControlMaxAgeOwned::try_from(String::from("600")).expect("owned decimal"),
            value
        );
        assert_eq!(
            AccessControlMaxAgeOwned::try_from(FieldValue::from_static("600")).expect("decimal field"),
            value
        );
    }

    #[test]
    fn header_paths_and_numeric_errors_cover_borrowed_and_owned_decoding() {
        let source = TestMap::new(&FieldName::AccessControlMaxAge, vec![FieldValue::from_static(" 00600 ")]);
        let view = AccessControlMaxAge::view(&source).expect("valid max age").expect("present");
        assert_eq!(view.seconds(), 600);
        assert_eq!(view.duration(), Duration::from_mins(10));

        let owned = AccessControlMaxAge::owned(&source).expect("valid owned max age").expect("present");
        assert_eq!(owned.seconds(), 600);
        assert_eq!(view, owned);

        let mut sink = TestMap::new(&FieldName::Accept, Vec::new());
        AccessControlMaxAge::insert(&mut sink, owned).expect("insert max age");
        assert_eq!(sink.name, &FieldName::AccessControlMaxAge);
        assert_eq!(sink.values, [FieldValue::from_static("600")]);

        let absent = TestMap::new(&FieldName::Accept, Vec::new());
        assert!(AccessControlMaxAge::view(&absent).expect("absent").is_none());
        assert!(AccessControlMaxAge::owned(&absent).expect("absent").is_none());

        for wire in ["", " ", "-1", "+1", "1.0", "18446744073709551616"] {
            let error = AccessControlMaxAgeOwned::try_from(wire).expect_err("invalid delta-seconds");
            assert_eq!(error.kind(), DecodeErrorKind::InvalidNumber, "{wire:?}");
        }
        let error = AccessControlMaxAgeOwned::try_from(String::from("bad\nvalue")).expect_err("invalid field bytes");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
        assert_eq!(
            AccessControlMaxAgeOwned::try_from("bad\nvalue")
                .expect_err("invalid borrowed field bytes")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let duplicate = TestMap::new(
            &FieldName::AccessControlMaxAge,
            vec![FieldValue::from_static("1"), FieldValue::from_static("2")],
        );
        assert_eq!(
            AccessControlMaxAge::view(&duplicate).expect_err("singleton header").kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
    }
}
