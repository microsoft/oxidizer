// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Numeric `Content-Length` parsing and encoding.

use std::fmt;
use std::str::FromStr;

use crate::sink::{EncodedValues, FieldSink, InsertError};
use crate::source::FieldSource;
use crate::{DecodeError, DecodeErrorKind, Field, FieldName, FieldValue};

/// Defines the `Content-Length` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 8.6](https://www.rfc-editor.org/rfc/rfc9110#section-8.6).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{ContentLength, ContentLengthOwned};
///
/// let mut map = HeaderMap::new();
/// ContentLength::insert(&mut map, ContentLengthOwned::new(42))?;
/// assert_eq!(
///     ContentLength::view(&map)?.map(ContentLengthOwned::get),
///     Some(42)
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct ContentLength {
    _private: (),
}

/// Owned value for the `Content-Length` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 8.6].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::ContentLengthOwned::new(42);
/// assert_eq!(value.get(), 42);
/// ```
///
/// `Content-Length: 0` describes an empty content body, while
/// `Content-Length: 1024` describes 1,024 octets.
///
/// [RFC 9110 section 8.6]: https://www.rfc-editor.org/rfc/rfc9110#section-8.6
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ContentLengthOwned(u64);

impl ContentLengthOwned {
    /// Creates a content length.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::ContentLengthOwned::new(42);
    /// assert_eq!(value.get(), 42);
    /// ```
    pub const fn new(length: u64) -> Self {
        Self(length)
    }

    /// Returns the length in octets.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::ContentLengthOwned::new(42);
    /// assert_eq!(value.get(), 42);
    /// ```
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ContentLengthOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for ContentLengthOwned {
    type Err = DecodeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_decimal_ows(value.as_bytes())
            .map(Self)
            .ok_or_else(|| DecodeError::new(&FieldName::ContentLength, DecodeErrorKind::InvalidNumber))
    }
}

impl Field for ContentLength {
    type View<'a> = ContentLengthOwned;
    type Owned = ContentLengthOwned;

    fn name() -> &'static FieldName {
        &FieldName::ContentLength
    }

    fn view_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        lines.validate_list_item_limit(b',', false)?;
        let mut repeated = lines.repeated();
        let first = repeated.next().expect("FieldLines always contains at least one value");
        let second = repeated.next();
        if second.is_none() && !first.as_bytes().contains(&b',') {
            return parse_decimal_ows(first.as_bytes())
                .map(ContentLengthOwned)
                .map(Some)
                .ok_or_else(|| DecodeError::new(&FieldName::ContentLength, DecodeErrorKind::InvalidNumber).at_value(0));
        }

        let mut parsed = None;
        parse_content_length_line(&mut parsed, first.as_bytes(), 0)?;
        if let Some(second) = second {
            parse_content_length_line(&mut parsed, second.as_bytes(), 1)?;
        }
        for (offset, value) in repeated.enumerate() {
            parse_content_length_line(&mut parsed, value.as_bytes(), offset + 2)?;
        }
        Ok(parsed.map(ContentLengthOwned))
    }

    fn owned_with<S>(source: &S, mode: crate::DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        Self::view_with(source, mode)
    }

    fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        sink.set_values(Self::name(), EncodedValues::single(FieldValue::from(value.0)))
    }
}

fn parse_content_length_line(parsed: &mut Option<u64>, bytes: &[u8], value_index: usize) -> Result<(), DecodeError> {
    for item in bytes.split(|byte| *byte == b',') {
        let number = parse_decimal_ows(item)
            .ok_or_else(|| DecodeError::new(&FieldName::ContentLength, DecodeErrorKind::InvalidNumber).at_value(value_index))?;
        if parsed.is_some_and(|previous| previous != number) {
            return Err(DecodeError::new(&FieldName::ContentLength, DecodeErrorKind::InvalidSyntax).at_value(value_index));
        }
        *parsed = Some(number);
    }
    Ok(())
}

fn parse_decimal_ows(bytes: &[u8]) -> Option<u64> {
    let mut index = 0;
    while bytes.get(index).is_some_and(|byte| matches!(byte, b' ' | b'\t')) {
        index += 1;
    }
    let digit_start = index;
    let mut value = 0_u64;
    while let Some(&byte) = bytes.get(index) {
        if byte.is_ascii_digit() {
            value = value.checked_mul(10)?.checked_add(u64::from(byte - b'0'))?;
            index += 1;
        } else {
            break;
        }
    }
    if index == digit_start {
        return None;
    }
    while bytes.get(index).is_some_and(|byte| matches!(byte, b' ' | b'\t')) {
        index += 1;
    }
    (index == bytes.len()).then_some(value)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::str;

    use super::parse_decimal_ows;

    #[test]
    fn decimal_parser_matches_independent_whitespace_and_digit_oracle() {
        for wire in [
            b" \t00123\t ".as_slice(),
            b"18446744073709551615",
            b"18446744073709551616",
            b"000000000000000000000000000001",
        ] {
            for offset in 0..wire.len() {
                for replacement in u8::MIN..=u8::MAX {
                    let mut candidate = wire.to_vec();
                    candidate[offset] = replacement;
                    let expected = str::from_utf8(&candidate)
                        .ok()
                        .map(|text| text.trim_matches([' ', '\t']))
                        .filter(|text| !text.is_empty() && text.as_bytes().iter().all(u8::is_ascii_digit))
                        .and_then(|text| text.parse::<u64>().ok());
                    assert_eq!(parse_decimal_ows(&candidate), expected, "{candidate:?}");
                }
            }
        }
    }

    #[test]
    fn decimal_parser_rejects_every_invalid_shape() {
        assert_eq!(parse_decimal_ows(b"0"), Some(0));
        assert_eq!(parse_decimal_ows(b"\t18446744073709551615 "), Some(u64::MAX));
        for value in [b"".as_slice(), b" \t", b"+1", b"-1", b"1 2", b"18446744073709551616"] {
            assert_eq!(parse_decimal_ows(value), None, "{value:?}");
        }
    }
}
