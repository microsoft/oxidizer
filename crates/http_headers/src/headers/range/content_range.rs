// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Write as _};
use std::ops::{Range as IndexRange, RangeBounds};
use std::str;

use super::super::invalid_syntax;
use super::range::ByteRangeSpec;
use super::shared::{parse_number, validate_range_unit_for};
use crate::{DecodeError, FieldName, FieldValue, FieldValueRef, SingleValueField, validate};

/// The complete representation length in a satisfied byte content range.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CompleteLength {
    /// The complete representation length is known.
    Known(u64),
    /// The complete representation length is unknown and is serialized as `*`.
    Unknown,
}

impl CompleteLength {
    const fn into_option(self) -> Option<u64> {
        match self {
            Self::Known(length) => Some(length),
            Self::Unknown => None,
        }
    }
}

impl From<u64> for CompleteLength {
    fn from(length: u64) -> Self {
        Self::Known(length)
    }
}

/// The byte-specific interpretation of a `Content-Range` value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{ByteContentRange, ContentRangeOwned};
///
/// let satisfied = ContentRangeOwned::try_from("bytes 0-99/200")?;
/// assert_eq!(
///     satisfied.byte_range(),
///     Some(ByteContentRange::Satisfied {
///         first: 0,
///         last: 99,
///         complete_length: Some(200),
///     })
/// );
///
/// let unsatisfied = ContentRangeOwned::try_from("bytes */200")?;
/// assert_eq!(
///     unsatisfied.byte_range(),
///     Some(ByteContentRange::Unsatisfied {
///         complete_length: 200,
///     })
/// );
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub enum ByteContentRange {
    /// A satisfied inclusive range.
    Satisfied {
        /// The first byte position.
        first: u64,
        /// The last byte position.
        last: u64,
        /// The complete representation length, or `None` when unknown.
        complete_length: Option<u64>,
    },
    /// An unsatisfied range response with the current complete length.
    Unsatisfied {
        /// The complete representation length.
        complete_length: u64,
    },
}

/// Defines the `Content-Range` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 14.4](https://www.rfc-editor.org/rfc/rfc9110#section-14.4).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{ContentRange, ContentRangeOwned};
///
/// let mut map = HeaderMap::new();
/// ContentRange::insert(&mut map, ContentRangeOwned::try_from("bytes 0-99/200")?)?;
/// assert!(ContentRange::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct ContentRange {
    _private: (),
}

/// Owned value for the `Content-Range` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 14.4].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::ContentRangeOwned::try_from("bytes 0-99/200")?;
/// assert_eq!(
///     value.byte_range(),
///     Some(http_headers::headers::ByteContentRange::Satisfied {
///         first: 0,
///         last: 99,
///         complete_length: Some(200),
///     })
/// );
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Content-Range: bytes 0-499/1234` is satisfied,
/// `Content-Range: bytes 500-999/*` has an unknown complete length, and
/// `Content-Range: bytes */1234` is unsatisfied. Extension forms such as
/// `Content-Range: custom opaque-payload` are preserved.
///
/// [RFC 9110 section 14.4]: https://www.rfc-editor.org/rfc/rfc9110#section-14.4
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct ContentRangeOwned {
    value: FieldValue,
    unit: IndexRange<usize>,
    payload: IndexRange<usize>,
    parsed: Option<ByteContentRange>,
}

/// Borrowed value for the `Content-Range` header.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{ContentRange, ContentRangeView};
/// use http_headers::{FieldValueRef, SingleValueField};
///
/// let view: ContentRangeView<'_> =
///     ContentRange::decode_view(FieldValueRef::new(b"bytes 0-99/200"))?;
/// assert_eq!(view.unit(), "bytes");
/// assert_eq!(view.as_field_value().as_bytes(), b"bytes 0-99/200");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct ContentRangeView<'a> {
    value: FieldValueRef<'a>,
    unit: &'a str,
    payload: &'a [u8],
    parsed: Option<ByteContentRange>,
}

impl fmt::Debug for ContentRangeOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContentRangeOwned")
            .field("unit", &self.unit())
            .field("byte_range", &self.parsed)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for ContentRangeView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContentRangeView")
            .field("unit", &self.unit)
            .field("byte_range", &self.parsed)
            .finish_non_exhaustive()
    }
}

impl ContentRangeOwned {
    /// Constructs a satisfied byte content range from explicit positions.
    ///
    /// Prefer [`Self::bytes_range`] when the caller already holds a Rust
    /// range.
    ///
    /// # Errors
    ///
    /// Returns an error for an inverted range or a complete length not
    /// greater than the last position.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{CompleteLength, ContentRangeOwned};
    ///
    /// let value = ContentRangeOwned::bytes(0, 9, CompleteLength::Known(10))?;
    /// assert_eq!(value.as_field_value().as_bytes(), b"bytes 0-9/10");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn bytes(first: u64, last: u64, complete_length: CompleteLength) -> Result<Self, DecodeError> {
        Self::bytes_range(first..=last, complete_length)
    }

    /// Constructs a satisfied byte content range.
    ///
    /// Included bounds map directly to HTTP's inclusive positions. An
    /// excluded start is incremented and an excluded end is decremented, so
    /// both `0..100` and `0..=99` represent bytes 0 through 99.
    ///
    /// # Errors
    ///
    /// Returns an error for an unbounded bound, an empty or inverted range, a
    /// bound adjustment that overflows, or a complete length not greater than
    /// the last position.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{CompleteLength, ContentRangeOwned};
    ///
    /// // Half-open and inclusive ranges describe the same bytes.
    /// let half_open = ContentRangeOwned::bytes_range(0..100, CompleteLength::Known(1000))?;
    /// let inclusive = ContentRangeOwned::bytes_range(0..=99, CompleteLength::Known(1000))?;
    /// assert_eq!(half_open.as_field_value().as_bytes(), b"bytes 0-99/1000");
    /// assert_eq!(
    ///     half_open.as_field_value().as_bytes(),
    ///     inclusive.as_field_value().as_bytes()
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn bytes_range(range: impl RangeBounds<u64>, complete_length: CompleteLength) -> Result<Self, DecodeError> {
        Self::bytes_range_from_spec(ByteRangeSpec::from_range(range), complete_length)
    }

    fn bytes_range_from_spec(spec: Result<ByteRangeSpec, DecodeError>, complete_length: CompleteLength) -> Result<Self, DecodeError> {
        let ByteRangeSpec::FromTo { first, last } = spec? else {
            return Err(invalid_syntax(&FieldName::ContentRange));
        };
        let complete_length = complete_length.into_option();
        validate_satisfied_content_range(first, last, complete_length)?;
        let mut wire = format!("bytes {first}-{last}/");
        if let Some(value) = complete_length {
            write!(&mut wire, "{value}").map_err(|_| invalid_syntax(&FieldName::ContentRange))?;
        } else {
            wire.push('*');
        }
        Ok(content_range_from_parts(
            wire,
            5,
            Some(ByteContentRange::Satisfied {
                first,
                last,
                complete_length,
            }),
        ))
    }

    /// Constructs an unsatisfied byte content range.
    ///
    /// This represents the wire form `bytes */complete_length`.
    ///
    /// # Errors
    ///
    /// The result type is retained for constructor API consistency; every
    /// `u64` complete length has a valid representation.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{ByteContentRange, ContentRangeOwned};
    ///
    /// let value = ContentRangeOwned::unsatisfied_bytes(1234)?;
    /// assert_eq!(value.as_field_value().as_bytes(), b"bytes */1234");
    /// assert_eq!(
    ///     value.byte_range(),
    ///     Some(ByteContentRange::Unsatisfied {
    ///         complete_length: 1234,
    ///     })
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[expect(
        clippy::unnecessary_wraps,
        reason = "content-range constructors consistently report validation through DecodeError"
    )]
    pub fn unsatisfied_bytes(complete_length: u64) -> Result<Self, DecodeError> {
        Ok(content_range_from_parts(
            format!("bytes */{complete_length}"),
            5,
            Some(ByteContentRange::Unsatisfied { complete_length }),
        ))
    }

    /// Constructs an extension content range.
    ///
    /// The extension payload is preserved; this type does not claim to
    /// normalize extension range semantics.
    ///
    /// # Errors
    ///
    /// Returns an error when the unit is not a token, is the reserved
    /// case-insensitive `bytes` unit, or the payload contains bytes outside
    /// the extension content-range grammar. Use [`Self::bytes`] or
    /// [`Self::unsatisfied_bytes`] for byte content ranges.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ContentRangeOwned;
    ///
    /// let value = ContentRangeOwned::extension("items", "0-9/100")?;
    /// assert_eq!(value.unit()?, "items");
    /// assert_eq!(value.byte_range(), None);
    /// assert_eq!(value.extension_payload(), Some(&b"0-9/100"[..]));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn extension(unit: impl AsRef<str>, payload: impl AsRef<str>) -> Result<Self, DecodeError> {
        let unit = unit.as_ref();
        let payload = payload.as_ref();
        validate_range_unit_for(unit.as_bytes(), &FieldName::ContentRange)?;
        if validate::eq_ignore_ascii_case(unit.as_bytes(), b"bytes") {
            return Err(invalid_syntax(&FieldName::ContentRange));
        }
        if !valid_content_range_extension(payload.as_bytes()) {
            return Err(invalid_syntax(&FieldName::ContentRange));
        }
        Ok(content_range_from_parts(format!("{unit} {payload}"), unit.len(), None))
    }

    /// Returns the range unit exactly as received.
    ///
    /// # Errors
    ///
    /// Returns an error if stored metadata does not match the wire value.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ContentRangeOwned;
    ///
    /// let bytes = ContentRangeOwned::try_from("bytes 0-99/200")?;
    /// assert_eq!(bytes.unit()?, "bytes");
    ///
    /// let items = ContentRangeOwned::extension("items", "0-9/100")?;
    /// assert_eq!(items.unit()?, "items");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn unit(&self) -> Result<&str, DecodeError> {
        let bytes = self
            .value
            .as_bytes()
            .get(self.unit.clone())
            .ok_or_else(|| invalid_syntax(&FieldName::ContentRange))?;
        str::from_utf8(bytes).map_err(|_invalid| invalid_syntax(&FieldName::ContentRange))
    }

    /// Returns the byte-specific interpretation, if the unit is `bytes`.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{ByteContentRange, ContentRangeOwned};
    ///
    /// let satisfied = ContentRangeOwned::try_from("bytes 0-99/200")?;
    /// assert_eq!(
    ///     satisfied.byte_range(),
    ///     Some(ByteContentRange::Satisfied {
    ///         first: 0,
    ///         last: 99,
    ///         complete_length: Some(200),
    ///     })
    /// );
    ///
    /// let extension = ContentRangeOwned::extension("items", "0-9/100")?;
    /// assert_eq!(extension.byte_range(), None);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn byte_range(&self) -> Option<ByteContentRange> {
        self.parsed
    }

    /// Returns an extension payload without normalization.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ContentRangeOwned;
    ///
    /// let extension = ContentRangeOwned::extension("items", "0-9/100")?;
    /// assert_eq!(extension.extension_payload(), Some(&b"0-9/100"[..]));
    ///
    /// let bytes = ContentRangeOwned::try_from("bytes 0-99/200")?;
    /// assert_eq!(bytes.extension_payload(), None);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn extension_payload(&self) -> Option<&[u8]> {
        if self.parsed.is_some() {
            None
        } else {
            self.value.as_bytes().get(self.payload.clone())
        }
    }

    /// Returns the preserved field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ContentRangeOwned;
    ///
    /// let value = ContentRangeOwned::try_from("bytes 0-99/200")?;
    /// assert_eq!(value.as_field_value().as_bytes(), b"bytes 0-99/200");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_field_value(&self) -> &FieldValue {
        &self.value
    }

    /// Consumes the header and returns its field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ContentRangeOwned;
    ///
    /// let value = ContentRangeOwned::try_from("bytes */1234")?;
    /// let field_value = value.into_field_value();
    /// assert_eq!(field_value.as_bytes(), b"bytes */1234");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }
}

super::super::shared::impl_field_value_conversion!(ContentRangeOwned, |value| value.value);

impl<'a> ContentRangeView<'a> {
    /// Returns the range unit exactly as received.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ContentRange;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view = ContentRange::decode_view(FieldValueRef::new(b"items 0-9/100"))?;
    /// assert_eq!(view.unit(), "items");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn unit(self) -> &'a str {
        self.unit
    }

    /// Returns the byte-specific interpretation, if the unit is `bytes`.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{ByteContentRange, ContentRange};
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view = ContentRange::decode_view(FieldValueRef::new(b"bytes 0-99/*"))?;
    /// assert_eq!(
    ///     view.byte_range(),
    ///     Some(ByteContentRange::Satisfied {
    ///         first: 0,
    ///         last: 99,
    ///         complete_length: None,
    ///     })
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn byte_range(self) -> Option<ByteContentRange> {
        self.parsed
    }

    /// Returns an extension payload without normalization.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ContentRange;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view = ContentRange::decode_view(FieldValueRef::new(b"items 0-9/100"))?;
    /// assert_eq!(view.extension_payload(), Some(&b"0-9/100"[..]));
    ///
    /// let bytes = ContentRange::decode_view(FieldValueRef::new(b"bytes 0-99/200"))?;
    /// assert_eq!(bytes.extension_payload(), None);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn extension_payload(self) -> Option<&'a [u8]> {
        if self.parsed.is_some() { None } else { Some(self.payload) }
    }

    /// Returns the borrowed field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ContentRange;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view = ContentRange::decode_view(FieldValueRef::new(b"bytes */1234"))?;
    /// assert_eq!(view.as_field_value().as_bytes(), b"bytes */1234");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn as_field_value(self) -> FieldValueRef<'a> {
        self.value
    }
}

impl SingleValueField for ContentRange {
    type View<'a> = ContentRangeView<'a>;
    type Owned = ContentRangeOwned;

    fn name() -> &'static FieldName {
        &FieldName::ContentRange
    }

    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
        let parsed = parse_content_range(value.as_bytes())?;
        Ok(ContentRangeView {
            value,
            unit: parsed.unit,
            payload: parsed.payload,
            parsed: parsed.byte_range,
        })
    }

    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
        ContentRangeOwned::try_from(value)
    }

    fn decode_view_with(value: FieldValueRef<'_>, mode: crate::DecodeMode) -> Result<Self::View<'_>, DecodeError> {
        let parsed = parse_content_range_with(value.as_bytes(), mode)?;
        Ok(ContentRangeView {
            value,
            unit: parsed.unit,
            payload: parsed.payload,
            parsed: parsed.byte_range,
        })
    }

    fn decode_owned_with(value: FieldValue, mode: crate::DecodeMode) -> Result<Self::Owned, DecodeError> {
        content_range_from_value_with(value, mode)
    }

    fn as_field_value(value: &Self::Owned) -> &FieldValue {
        &value.value
    }

    fn into_field_value(value: Self::Owned) -> FieldValue {
        value.value
    }
}

fn content_range_from_parts(wire: String, unit_end: usize, parsed: Option<ByteContentRange>) -> ContentRangeOwned {
    let payload_start = unit_end + 1;
    let payload_end = wire.len();
    let value = FieldValue::try_from(wire).expect("validated content-range parts form a valid field value");
    ContentRangeOwned {
        value,
        unit: 0..unit_end,
        payload: payload_start..payload_end,
        parsed,
    }
}

super::super::shared::impl_string_conversions!(ContentRangeOwned, &FieldName::ContentRange, invalid_syntax, wire);

impl TryFrom<FieldValue> for ContentRangeOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        content_range_from_value_with(value, crate::DecodeMode::Strict)
    }
}

fn content_range_from_value_with(value: FieldValue, mode: crate::DecodeMode) -> Result<ContentRangeOwned, DecodeError> {
    let (unit_end, payload_end, byte_range) = {
        let parsed = parse_content_range_with(value.as_bytes(), mode)?;
        // Every parser preserves the unit prefix followed by one SP.
        let unit_end = parsed.unit.len();
        (unit_end, unit_end + 1 + parsed.payload.len(), parsed.byte_range)
    };
    Ok(ContentRangeOwned {
        value,
        unit: 0..unit_end,
        payload: unit_end + 1..payload_end,
        parsed: byte_range,
    })
}

struct ParsedContentRange<'a> {
    unit: &'a str,
    payload: &'a [u8],
    byte_range: Option<ByteContentRange>,
}

fn parse_content_range(bytes: &[u8]) -> Result<ParsedContentRange<'_>, DecodeError> {
    if let Some(payload) = bytes.strip_prefix(b"bytes ") {
        let unit = http_headers_simd::ascii_str(&bytes[..5]).expect("the matched bytes unit is valid ASCII");
        return Ok(ParsedContentRange {
            unit,
            payload,
            byte_range: Some(parse_byte_content_range(payload)?),
        });
    }
    parse_content_range_extension(bytes)
}

fn parse_content_range_with(bytes: &[u8], mode: crate::DecodeMode) -> Result<ParsedContentRange<'_>, DecodeError> {
    if mode == crate::DecodeMode::Strict {
        return parse_content_range(bytes);
    }
    if let Ok(parsed) = parse_content_range(bytes) {
        return Ok(parsed);
    }
    parse_content_range_relaxed(bytes)
}

fn parse_content_range_relaxed(bytes: &[u8]) -> Result<ParsedContentRange<'_>, DecodeError> {
    if !validate::field_value(bytes) {
        return Err(invalid_syntax(&FieldName::ContentRange));
    }
    let Some(separator) = bytes.iter().position(|byte| *byte == b' ') else {
        return Err(invalid_syntax(&FieldName::ContentRange));
    };
    let unit_bytes = &bytes[..separator];
    let payload = &bytes[separator + 1..];
    if payload.first().is_some_and(|byte| matches!(byte, b' ' | b'\t')) {
        return Err(invalid_syntax(&FieldName::ContentRange));
    }
    validate_range_unit_for(unit_bytes, &FieldName::ContentRange)?;
    let unit = http_headers_simd::ascii_str(unit_bytes).expect("validated range units are ASCII");
    let byte_range = if validate::eq_ignore_ascii_case(unit_bytes, b"bytes") {
        Some(parse_byte_content_range_relaxed(payload)?)
    } else {
        if !valid_content_range_extension(payload) {
            return Err(invalid_syntax(&FieldName::ContentRange));
        }
        None
    };
    Ok(ParsedContentRange { unit, payload, byte_range })
}

#[cold]
#[inline(never)]
fn parse_content_range_extension(bytes: &[u8]) -> Result<ParsedContentRange<'_>, DecodeError> {
    let Some(separator) = bytes.iter().position(|byte| *byte == b' ') else {
        return Err(invalid_syntax(&FieldName::ContentRange));
    };
    let unit_bytes = &bytes[..separator];
    let payload = &bytes[separator + 1..];
    validate_range_unit_for(unit_bytes, &FieldName::ContentRange)?;
    let unit = http_headers_simd::ascii_str(unit_bytes).expect("validated range units are ASCII");
    let byte_range = if validate::eq_ignore_ascii_case(unit_bytes, b"bytes") {
        Some(parse_byte_content_range(payload)?)
    } else {
        if !valid_content_range_extension(payload) {
            return Err(invalid_syntax(&FieldName::ContentRange));
        }
        None
    };
    Ok(ParsedContentRange { unit, payload, byte_range })
}

fn parse_byte_content_range(bytes: &[u8]) -> Result<ByteContentRange, DecodeError> {
    if let Some(complete) = bytes.strip_prefix(b"*/") {
        return parse_number(complete, &FieldName::ContentRange).map(|complete_length| ByteContentRange::Unsatisfied { complete_length });
    }
    let Some(slash) = bytes.iter().position(|byte| *byte == b'/') else {
        return Err(invalid_syntax(&FieldName::ContentRange));
    };
    if bytes[slash + 1..].contains(&b'/') {
        return Err(invalid_syntax(&FieldName::ContentRange));
    }
    let included = &bytes[..slash];
    let Some(dash) = included.iter().position(|byte| *byte == b'-') else {
        return Err(invalid_syntax(&FieldName::ContentRange));
    };
    if included[dash + 1..].contains(&b'-') {
        return Err(invalid_syntax(&FieldName::ContentRange));
    }
    let first = parse_number(&included[..dash], &FieldName::ContentRange)?;
    let last = parse_number(&included[dash + 1..], &FieldName::ContentRange)?;
    let complete = &bytes[slash + 1..];
    let complete_length = match complete {
        b"*" => None,
        _ => Some(parse_number(complete, &FieldName::ContentRange)?),
    };
    validate_satisfied_content_range(first, last, complete_length)?;
    Ok(ByteContentRange::Satisfied {
        first,
        last,
        complete_length,
    })
}

fn parse_byte_content_range_relaxed(bytes: &[u8]) -> Result<ByteContentRange, DecodeError> {
    let Some(slash) = bytes.iter().position(|byte| *byte == b'/') else {
        return Err(invalid_syntax(&FieldName::ContentRange));
    };
    if bytes[slash + 1..].contains(&b'/') {
        return Err(invalid_syntax(&FieldName::ContentRange));
    }
    let included = trim_ows_end(&bytes[..slash]);
    let complete = trim_ows_start(&bytes[slash + 1..]);
    if included == b"*" {
        return parse_number(complete, &FieldName::ContentRange).map(|complete_length| ByteContentRange::Unsatisfied { complete_length });
    }
    let Some(dash) = included.iter().position(|byte| *byte == b'-') else {
        return Err(invalid_syntax(&FieldName::ContentRange));
    };
    if included[dash + 1..].contains(&b'-') {
        return Err(invalid_syntax(&FieldName::ContentRange));
    }
    let first = parse_number(trim_ows_end(&included[..dash]), &FieldName::ContentRange)?;
    let last = parse_number(trim_ows_start(&included[dash + 1..]), &FieldName::ContentRange)?;
    let complete_length = if complete == b"*" {
        None
    } else {
        Some(parse_number(complete, &FieldName::ContentRange)?)
    };
    validate_satisfied_content_range(first, last, complete_length)?;
    Ok(ByteContentRange::Satisfied {
        first,
        last,
        complete_length,
    })
}

fn trim_ows_start(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(|byte| matches!(byte, b' ' | b'\t')) {
        bytes = &bytes[1..];
    }
    bytes
}

fn trim_ows_end(mut bytes: &[u8]) -> &[u8] {
    while bytes.last().is_some_and(|byte| matches!(byte, b' ' | b'\t')) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn validate_satisfied_content_range(first: u64, last: u64, complete_length: Option<u64>) -> Result<(), DecodeError> {
    if last < first || complete_length.is_some_and(|length| last >= length) {
        Err(invalid_syntax(&FieldName::ContentRange))
    } else {
        Ok(())
    }
}

fn valid_content_range_extension(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| matches!(byte, b'\t' | 0x20..=0x7e))
}

#[cfg(test)]
#[expect(
    clippy::assertions_on_result_states,
    reason = "the tests classify many parser outcomes without needing their success values"
)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::ops::Bound;

    use super::{
        ByteContentRange, CompleteLength, ContentRange, ContentRangeOwned, content_range_from_value_with, parse_byte_content_range,
        parse_byte_content_range_relaxed, parse_content_range, parse_content_range_relaxed, parse_content_range_with,
        valid_content_range_extension, validate_satisfied_content_range,
    };
    use crate::{DecodeErrorKind, DecodeMode, FieldName, FieldValue, FieldValueRef, SingleValueField};

    #[test]
    fn relaxed_whitespace_trimming_is_directional_and_preserves_other_bytes() {
        assert_eq!(super::trim_ows_start(b" \t1 \t"), b"1 \t");
        assert_eq!(super::trim_ows_end(b" \t1 \t"), b" \t1");
        for bytes in [b"".as_slice(), b" \t"] {
            assert_eq!(super::trim_ows_start(bytes), b"");
            assert_eq!(super::trim_ows_end(bytes), b"");
        }
        for byte in u8::MIN..=u8::MAX {
            if !matches!(byte, b' ' | b'\t') {
                let bytes = [byte, b'1', byte];
                assert_eq!(super::trim_ows_start(&bytes), bytes);
                assert_eq!(super::trim_ows_end(&bytes), bytes);
            }
        }
    }

    #[test]
    fn owned_offsets_preserve_unit_and_payload_across_storage() {
        for (wire, mode, unit, payload) in [
            ("bytes 0-9/10", DecodeMode::Strict, "bytes", b"0-9/10".as_slice()),
            ("Bytes 0 - 9 / 10", DecodeMode::Relaxed, "Bytes", b"0 - 9 / 10"),
            ("items ", DecodeMode::Strict, "items", b""),
            ("items first-last/complete", DecodeMode::Strict, "items", b"first-last/complete"),
        ] {
            for value in [FieldValue::from_static(wire), FieldValue::try_from(wire.to_owned()).unwrap()] {
                let decoded = content_range_from_value_with(value, mode).unwrap();
                assert_eq!(decoded.unit().unwrap(), unit);
                assert_eq!(&decoded.value.as_bytes()[decoded.payload.clone()], payload);
                assert_eq!(decoded.value.as_bytes(), wire.as_bytes());
            }
        }
    }

    #[test]
    fn constructors_and_accessors_cover_byte_and_extension_forms() {
        let bytes = ContentRangeOwned::bytes(0, 9, CompleteLength::Known(10)).expect("satisfied range");
        assert_eq!(bytes.unit(), Ok("bytes"));
        assert_eq!(
            bytes.byte_range(),
            Some(ByteContentRange::Satisfied {
                first: 0,
                last: 9,
                complete_length: Some(10),
            })
        );
        assert_eq!(bytes.extension_payload(), None);
        assert_eq!(bytes.as_field_value().as_bytes(), b"bytes 0-9/10");
        assert_eq!(bytes.clone().into_field_value().as_bytes(), b"bytes 0-9/10");
        assert!(format!("{bytes:?}").contains("byte_range"));
        assert_eq!(
            ContentRangeOwned::try_from("bytes 0-1/2")
                .expect("valid borrowed content range")
                .byte_range(),
            Some(ByteContentRange::Satisfied {
                first: 0,
                last: 1,
                complete_length: Some(2),
            })
        );

        let open_length = ContentRangeOwned::bytes_range(0..10, CompleteLength::Unknown).expect("unknown length");
        assert_eq!(open_length.as_field_value().as_bytes(), b"bytes 0-9/*");
        let unsatisfied = ContentRangeOwned::unsatisfied_bytes(42).expect("unsatisfied range");
        assert_eq!(
            unsatisfied.byte_range(),
            Some(ByteContentRange::Unsatisfied { complete_length: 42 })
        );

        let extension = ContentRangeOwned::extension("items", "first-last/complete").expect("extension");
        assert_eq!(extension.unit(), Ok("items"));
        assert_eq!(extension.byte_range(), None);
        assert_eq!(extension.extension_payload(), Some(b"first-last/complete".as_slice()));
        assert_eq!(
            ContentRangeOwned::extension("bad unit", "payload")
                .expect_err("invalid unit")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
        assert_eq!(
            ContentRangeOwned::extension("items", "\u{7f}").expect_err("invalid payload").kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }

    #[test]
    fn range_bounds_and_complete_lengths_reject_empty_or_impossible_ranges() {
        for result in [
            ContentRangeOwned::bytes(9, 0, CompleteLength::Unknown),
            ContentRangeOwned::bytes(0, 9, CompleteLength::Known(9)),
            ContentRangeOwned::bytes_range((Bound::Excluded(u64::MAX), Bound::Unbounded), CompleteLength::Unknown),
            ContentRangeOwned::bytes_range((Bound::Included(0), Bound::Excluded(0)), CompleteLength::Unknown),
            ContentRangeOwned::bytes_range((Bound::Unbounded, Bound::Included(1)), CompleteLength::Unknown),
            ContentRangeOwned::bytes_range((Bound::Included(0), Bound::Unbounded), CompleteLength::Unknown),
        ] {
            assert_eq!(result.expect_err("invalid content range").kind(), DecodeErrorKind::InvalidSyntax);
        }
        assert!(validate_satisfied_content_range(0, 0, Some(1)).is_ok());
        assert!(validate_satisfied_content_range(1, 0, None).is_err());
    }

    #[test]
    fn strict_parsing_borrows_units_and_rejects_malformed_byte_ranges() {
        let wire = b"bytes 0-1/2";
        let parsed = parse_content_range(wire).expect("strict byte range");
        assert_eq!(parsed.unit, "bytes");
        assert_eq!(parsed.unit.as_ptr(), wire.as_ptr());
        assert_eq!(parsed.payload, b"0-1/2");

        assert_eq!(
            parse_content_range(b"Bytes */10").expect("case-insensitive bytes unit").byte_range,
            Some(ByteContentRange::Unsatisfied { complete_length: 10 })
        );
        assert_eq!(parse_content_range(b"items opaque").expect("extension range").byte_range, None);

        for malformed in [
            b"bytes".as_slice(),
            b"bytes 0-1",
            b"bytes 0-1/2/3",
            b"bytes 01/2",
            b"bytes 0--1/2",
            b"bytes 2-1/3",
            b"bytes 0-2/2",
            b"bytes 0-x/2",
            b"bytes 0-1/x",
            b"bytes */*",
            b"bad/unit payload",
            b"items \x7f",
        ] {
            assert!(parse_content_range(malformed).is_err(), "{malformed:?}");
        }
    }

    #[test]
    fn relaxed_parsing_accepts_only_whitespace_deviations() {
        let parsed = parse_content_range_with(b"Bytes 0 - 9 / 10", DecodeMode::Relaxed).expect("relaxed byte whitespace");
        assert_eq!(
            parsed.byte_range,
            Some(ByteContentRange::Satisfied {
                first: 0,
                last: 9,
                complete_length: Some(10),
            })
        );
        assert_eq!(
            parse_byte_content_range_relaxed(b"* / 10").expect("relaxed unsatisfied form"),
            ByteContentRange::Unsatisfied { complete_length: 10 }
        );
        assert!(parse_content_range_with(b"bytes  0-1/2", DecodeMode::Relaxed).is_err());
        assert!(parse_content_range_with(b"bytes", DecodeMode::Relaxed).is_err());
        assert!(parse_byte_content_range_relaxed(b"0-1/2/3").is_err());
        assert!(parse_byte_content_range_relaxed(b"0--1/2").is_err());
        assert!(parse_byte_content_range_relaxed(b"01/2").is_err());
        assert!(parse_byte_content_range_relaxed(b"2-1/3").is_err());
        assert!(parse_byte_content_range_relaxed(b"0-x/2").is_err());
        assert!(parse_byte_content_range_relaxed(b"x-1/2").is_err());
        assert!(parse_byte_content_range_relaxed(b"0-1/x").is_err());
        assert!(parse_byte_content_range_relaxed(b"*/x").is_err());
        assert!(parse_byte_content_range_relaxed(b"0-1").is_err());
        assert_eq!(
            parse_byte_content_range_relaxed(b"0-1/*").expect("unknown complete length"),
            ByteContentRange::Satisfied {
                first: 0,
                last: 1,
                complete_length: None,
            }
        );

        assert_eq!(
            parse_content_range_relaxed(b"items opaque")
                .expect("direct relaxed extension")
                .byte_range,
            None
        );
        assert_eq!(
            parse_content_range_relaxed(b"items ")
                .expect("empty extension payload is preserved")
                .byte_range,
            None
        );
        assert!(parse_content_range_relaxed(b"items \x7f").is_err());
        assert!(parse_content_range_relaxed(b"items \xff").is_err());
        assert!(parse_content_range_relaxed(b"bad/unit payload").is_err());
        assert!(parse_content_range_relaxed(b"bytes 0-x/2").is_err());

        let strict_fast_path =
            parse_content_range_with(b"bytes 0-1/2", DecodeMode::Relaxed).expect("strict syntax remains valid in relaxed mode");
        assert_eq!(strict_fast_path.unit, "bytes");
        let extension = parse_content_range_with(b"items opaque", DecodeMode::Relaxed).expect("relaxed extension");
        assert_eq!(extension.byte_range, None);
        assert!(parse_content_range_with(b"items \x7f", DecodeMode::Relaxed).is_err());
    }

    #[test]
    fn owned_and_borrowed_decoding_preserve_payloads() {
        let value = FieldValue::from_static("items opaque payload");
        let view = <ContentRange as SingleValueField>::decode_view(value.as_field_value_ref()).expect("borrowed extension");
        assert_eq!(view.unit(), "items");
        assert_eq!(view.byte_range(), None);
        assert_eq!(view.extension_payload(), Some(b"opaque payload".as_slice()));
        assert_eq!(view.as_field_value().as_bytes(), b"items opaque payload");
        assert!(format!("{view:?}").contains("items"));

        let bytes = FieldValue::from_static("bytes 0-1/2");
        let bytes_view = <ContentRange as SingleValueField>::decode_view(bytes.as_field_value_ref()).expect("borrowed byte content range");
        assert_eq!(bytes_view.extension_payload(), None);
        assert!(<ContentRange as SingleValueField>::decode_view(FieldValueRef::new(b"invalid")).is_err());
        assert!(<ContentRange as SingleValueField>::decode_view_with(FieldValueRef::new(b"invalid"), DecodeMode::Relaxed,).is_err());
        assert!(<ContentRange as SingleValueField>::decode_owned_with(FieldValue::from_static("invalid"), DecodeMode::Relaxed,).is_err());

        let owned = <ContentRange as SingleValueField>::decode_owned(value).expect("owned extension");
        assert_eq!(owned.extension_payload(), Some(b"opaque payload".as_slice()));
        assert_eq!(
            <ContentRange as SingleValueField>::as_field_value(&owned).as_bytes(),
            b"items opaque payload"
        );
        assert_eq!(
            <ContentRange as SingleValueField>::into_field_value(owned).as_bytes(),
            b"items opaque payload"
        );
        assert_eq!(
            ContentRangeOwned::try_from(String::from("line\nbreak"))
                .expect_err("invalid field value")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            ContentRangeOwned::try_from("line\nbreak")
                .expect_err("invalid borrowed field value")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        let owned = ContentRangeOwned::try_from(String::from("bytes 0-1/2")).expect("owned string conversion");
        assert_eq!(
            owned.byte_range(),
            Some(ByteContentRange::Satisfied {
                first: 0,
                last: 1,
                complete_length: Some(2),
            })
        );
        let relaxed =
            <ContentRange as SingleValueField>::decode_owned_with(FieldValue::from_static("bytes 0 - 1 / 2"), DecodeMode::Relaxed)
                .expect("relaxed owned decode");
        assert_eq!(
            relaxed.byte_range(),
            Some(ByteContentRange::Satisfied {
                first: 0,
                last: 1,
                complete_length: Some(2),
            })
        );
        assert_eq!(
            <ContentRange as SingleValueField>::decode_view_with(FieldValueRef::new(b"bytes 0 - 1 / 2"), DecodeMode::Relaxed,)
                .expect("relaxed view")
                .byte_range(),
            Some(ByteContentRange::Satisfied {
                first: 0,
                last: 1,
                complete_length: Some(2),
            })
        );
        assert_eq!(<ContentRange as SingleValueField>::name(), &FieldName::ContentRange);
        assert!(valid_content_range_extension(b""));
        assert!(!valid_content_range_extension(b"\x7f"));
        assert!(parse_byte_content_range(b"0-1/*").is_ok());
    }

    #[test]
    fn defensive_owned_accessors_revalidate_private_ranges() {
        let out_of_bounds = ContentRangeOwned {
            value: FieldValue::from_static("items payload"),
            unit: 0..99,
            payload: 6..13,
            parsed: None,
        };
        assert_eq!(
            out_of_bounds.unit().expect_err("unit range is outside storage").kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let invalid_utf8 = FieldValue::from_bytes(b"\xff payload").expect("obs-text field value");
        let invalid_utf8 = ContentRangeOwned {
            value: invalid_utf8,
            unit: 0..1,
            payload: 2..9,
            parsed: None,
        };
        assert_eq!(
            invalid_utf8.unit().expect_err("unit bytes are not UTF-8").kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let missing_payload = ContentRangeOwned {
            value: FieldValue::from_static("items payload"),
            unit: 0..5,
            payload: 99..100,
            parsed: None,
        };
        assert_eq!(missing_payload.extension_payload(), None);
    }
}
