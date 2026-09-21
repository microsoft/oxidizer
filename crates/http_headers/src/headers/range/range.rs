// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Write as _;
use std::ops::{Bound, RangeBounds};
use std::{fmt, str};

use http_headers_simd::ascii_str;

use super::super::{invalid_syntax, trim_ows};
use super::shared::{parse_number, validate_range_unit_for};
use crate::{DecodeError, DecodeErrorKind, FieldName, FieldValue, FieldValueRef, SingleValueField, validate};

/// One byte-range specification from a `Range` field.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{ByteRangeSpec, RangeOwned};
///
/// let value = RangeOwned::try_from("bytes=0-9, 20-, -5")?;
/// let specs = value
///     .byte_ranges()
///     .expect("byte ranges")
///     .collect::<Vec<_>>();
/// assert_eq!(
///     specs,
///     [
///         ByteRangeSpec::FromTo { first: 0, last: 9 },
///         ByteRangeSpec::From { first: 20 },
///         ByteRangeSpec::Suffix { length: 5 },
///     ]
/// );
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub enum ByteRangeSpec {
    /// A closed inclusive range.
    ///
    /// Direct construction can represent `last < first`; encoding through
    /// [`RangeOwned::bytes`] rejects that state.
    FromTo {
        /// The first byte position.
        first: u64,
        /// The last byte position.
        last: u64,
    },
    /// A range from a position through the end of the representation.
    From {
        /// The first byte position.
        first: u64,
    },
    /// The final `length` bytes of the representation.
    Suffix {
        /// The requested suffix length.
        length: u64,
    },
}

fn parse_range_with(bytes: &[u8], mode: crate::DecodeMode) -> Result<ParsedRange<'_>, DecodeError> {
    if mode == crate::DecodeMode::Strict {
        return parse_range(bytes);
    }
    if let Ok(parsed) = parse_range(bytes) {
        return Ok(parsed);
    }
    parse_range_relaxed(bytes)
}

fn parse_range_relaxed(bytes: &[u8]) -> Result<ParsedRange<'_>, DecodeError> {
    let Some(separator) = separator_index(bytes) else {
        return Err(invalid_syntax(&FieldName::Range));
    };
    let unit_bytes = trim_ows(&bytes[..separator]);
    let payload = trim_ows(&bytes[separator + 1..]);
    validate_range_unit_for(unit_bytes, &FieldName::Range)?;
    let unit = ascii_str(unit_bytes).expect("validated range units are ASCII");
    let is_bytes = validate::eq_ignore_ascii_case(unit_bytes, b"bytes");
    if is_bytes {
        validate_byte_range_set_relaxed(payload)?;
    } else if !valid_extension_payload(payload) {
        return Err(invalid_syntax(&FieldName::Range));
    }
    Ok(ParsedRange {
        unit,
        payload,
        bytes: is_bytes,
    })
}

fn parse_byte_spec_relaxed(bytes: &[u8]) -> Result<ByteRangeSpec, DecodeError> {
    let bytes = trim_ows(bytes);
    let Some(separator) = bytes.iter().position(|byte| *byte == b'-') else {
        return Err(invalid_syntax(&FieldName::Range));
    };
    if bytes[separator + 1..].contains(&b'-') {
        return Err(invalid_syntax(&FieldName::Range));
    }
    let first = trim_ows(&bytes[..separator]);
    let last = trim_ows(&bytes[separator + 1..]);
    if first.is_empty() {
        return parse_number(last, &FieldName::Range).map(|length| ByteRangeSpec::Suffix { length });
    }
    let first = parse_number(first, &FieldName::Range)?;
    if last.is_empty() {
        Ok(ByteRangeSpec::From { first })
    } else {
        let last = parse_number(last, &FieldName::Range)?;
        ByteRangeSpec::from_range(first..=last)
    }
}

fn validate_byte_range_set_relaxed(bytes: &[u8]) -> Result<(), DecodeError> {
    let mut count = 0_usize;
    for item in bytes.split(|byte| *byte == b',') {
        let item = trim_ows(item);
        if item.is_empty() {
            continue;
        }
        parse_byte_spec_relaxed(item)?;
        count += 1;
    }
    if count == 0 {
        Err(DecodeError::new(&FieldName::Range, DecodeErrorKind::MissingValue))
    } else {
        Ok(())
    }
}

impl ByteRangeSpec {
    /// Constructs a closed inclusive range from explicit positions.
    ///
    /// Prefer [`Self::from_range`] when the caller already holds a Rust range.
    ///
    /// # Errors
    ///
    /// Returns an error when `last` precedes `first`.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{ByteRangeSpec, RangeOwned};
    ///
    /// let spec = ByteRangeSpec::from_to(0, 99)?;
    /// assert_eq!(spec, ByteRangeSpec::FromTo { first: 0, last: 99 });
    ///
    /// let value = RangeOwned::bytes([spec])?;
    /// assert_eq!(value.as_field_value().as_bytes(), b"bytes=0-99");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn from_to(first: u64, last: u64) -> Result<Self, DecodeError> {
        Self::from_range(first..=last)
    }

    /// Constructs a bounded or open-ended byte range.
    ///
    /// Included bounds map directly to HTTP's inclusive positions. An
    /// excluded start is incremented and an excluded end is decremented, so
    /// both `0..100` and `0..=99` represent bytes 0 through 99. An unbounded
    /// end constructs an open-ended range.
    ///
    /// # Errors
    ///
    /// Returns an error for an unbounded start, an empty or inverted range, or
    /// a bound adjustment that overflows.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ByteRangeSpec;
    ///
    /// let half_open = ByteRangeSpec::from_range(0..100)?;
    /// let inclusive = ByteRangeSpec::from_range(0..=99)?;
    /// assert_eq!(half_open, ByteRangeSpec::FromTo { first: 0, last: 99 });
    /// assert_eq!(half_open, inclusive);
    ///
    /// let open_ended = ByteRangeSpec::from_range(100..)?;
    /// assert_eq!(open_ended, ByteRangeSpec::From { first: 100 });
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn from_range(range: impl RangeBounds<u64>) -> Result<Self, DecodeError> {
        Self::from_bounds(range.start_bound().cloned(), range.end_bound().cloned())
    }

    fn from_bounds(start: Bound<u64>, end: Bound<u64>) -> Result<Self, DecodeError> {
        let first = match start {
            Bound::Included(first) => first,
            Bound::Excluded(first) => first.checked_add(1).ok_or_else(|| invalid_syntax(&FieldName::Range))?,
            Bound::Unbounded => return Err(invalid_syntax(&FieldName::Range)),
        };
        let last = match end {
            Bound::Included(last) => Some(last),
            Bound::Excluded(last) => Some(last.checked_sub(1).ok_or_else(|| invalid_syntax(&FieldName::Range))?),
            Bound::Unbounded => None,
        };
        match last {
            Some(last) if last >= first => Ok(Self::FromTo { first, last }),
            Some(_last) => Err(invalid_syntax(&FieldName::Range)),
            None => Ok(Self::From { first }),
        }
    }

    /// Constructs an open-ended range.
    #[must_use]
    #[deprecated(since = "0.1.0", note = "use ByteRangeSpec::starting_at")]
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::RangeOwned::try_from("bytes=0-99")?;
    /// assert!(value.is_bytes());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn from(first: u64) -> Self {
        Self::starting_at(first)
    }

    /// Constructs an open-ended range without resembling `From::from`.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{ByteRangeSpec, RangeOwned};
    ///
    /// let spec = ByteRangeSpec::starting_at(100);
    /// assert_eq!(spec, ByteRangeSpec::From { first: 100 });
    ///
    /// let value = RangeOwned::bytes([spec])?;
    /// assert_eq!(value.as_field_value().as_bytes(), b"bytes=100-");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn starting_at(first: u64) -> Self {
        Self::From { first }
    }

    /// Constructs a suffix range.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{ByteRangeSpec, RangeOwned};
    ///
    /// let spec = ByteRangeSpec::suffix(500);
    /// assert_eq!(spec, ByteRangeSpec::Suffix { length: 500 });
    ///
    /// let value = RangeOwned::bytes([spec])?;
    /// assert_eq!(value.as_field_value().as_bytes(), b"bytes=-500");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn suffix(length: u64) -> Self {
        Self::Suffix { length }
    }
}

/// Defines the `Range` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 14.2](https://www.rfc-editor.org/rfc/rfc9110#section-14.2).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{Range, RangeOwned};
///
/// let mut map = HeaderMap::new();
/// Range::insert(&mut map, RangeOwned::try_from("bytes=0-99")?)?;
/// assert!(Range::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct Range {
    _private: (),
}

/// Owned value for the `Range` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 14.2].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::RangeOwned::try_from("bytes=0-99")?;
/// assert!(value.is_bytes());
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Range: bytes=0-499` is closed, `Range: bytes=500-` is open-ended, and
/// `Range: bytes=-500` is a suffix range. Multiple ranges can be sent as
/// `Range: bytes=0-499, 1000-1499`; extension units such as
/// `Range: custom=opaque-set` are preserved.
///
/// [RFC 9110 section 14.2]: https://www.rfc-editor.org/rfc/rfc9110#section-14.2
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct RangeOwned {
    value: FieldValue,
    bytes: bool,
}

/// Borrowed value for the `Range` header.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{Range, RangeView};
/// use http_headers::{FieldValueRef, SingleValueField};
///
/// let view: RangeView<'_> = Range::decode_view(FieldValueRef::new(b"bytes=0-99"))?;
/// assert_eq!(view.unit(), "bytes");
/// assert!(view.is_bytes());
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct RangeView<'a> {
    value: FieldValueRef<'a>,
    unit: &'a str,
    range_set: &'a [u8],
    bytes: bool,
}

impl fmt::Debug for RangeOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RangeOwned")
            .field("unit", &self.unit())
            .field("is_bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for RangeView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RangeView")
            .field("unit", &self.unit)
            .field("is_bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

impl RangeOwned {
    /// Constructs a canonical byte range-set.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty set or an inverted closed range.
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::RangeOwned::try_from("bytes=0-99")?;
    /// assert!(value.is_bytes());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn bytes<I>(specs: I) -> Result<Self, DecodeError>
    where
        I: IntoIterator<Item = ByteRangeSpec>,
    {
        let specs = specs.into_iter();
        let mut wire = String::with_capacity(6_usize.saturating_add(specs.size_hint().0.saturating_mul(8)));
        wire.push_str("bytes=");
        let mut count = 0_usize;
        for spec in specs {
            if count != 0 {
                wire.push_str(", ");
            }
            append_byte_spec(&mut wire, spec)?;
            count += 1;
        }
        if count == 0 {
            return Err(DecodeError::new(&FieldName::Range, DecodeErrorKind::MissingValue));
        }
        let value = validated_byte_range_value(wire);
        Ok(Self { value, bytes: true })
    }

    /// Constructs an extension range-unit and opaque range-set.
    ///
    /// The extension payload is preserved; this type does not claim to
    /// normalize extension range semantics.
    ///
    /// # Errors
    ///
    /// Returns an error when the unit is not a token, is the reserved
    /// case-insensitive `bytes` unit, or the payload is not nonempty visible
    /// ASCII. Use [`Self::bytes`] for byte ranges.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::RangeOwned;
    ///
    /// let value = RangeOwned::extension("items", "1-5")?;
    /// assert_eq!(value.unit()?, "items");
    /// assert!(!value.is_bytes());
    /// assert_eq!(value.extension_range_set(), Some(&b"1-5"[..]));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn extension(unit: impl AsRef<str>, range_set: impl AsRef<str>) -> Result<Self, DecodeError> {
        let unit = unit.as_ref();
        let range_set = range_set.as_ref();
        validate_range_unit_for(unit.as_bytes(), &FieldName::Range)?;
        if validate::eq_ignore_ascii_case(unit.as_bytes(), b"bytes") {
            return Err(invalid_syntax(&FieldName::Range));
        }
        if !valid_extension_payload(range_set.as_bytes()) {
            return Err(invalid_syntax(&FieldName::Range));
        }
        let mut wire = String::with_capacity(unit.len() + 1 + range_set.len());
        wire.push_str(unit);
        wire.push('=');
        wire.push_str(range_set);
        let value = validated_extension_range_value(wire);
        Ok(Self { value, bytes: false })
    }

    /// Returns the range unit exactly as received.
    ///
    /// # Errors
    ///
    /// Returns an error if stored metadata does not match the wire value.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::RangeOwned;
    ///
    /// let bytes = RangeOwned::try_from("bytes=0-99")?;
    /// assert_eq!(bytes.unit()?, "bytes");
    ///
    /// let items = RangeOwned::extension("items", "1-5")?;
    /// assert_eq!(items.unit()?, "items");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn unit(&self) -> Result<&str, DecodeError> {
        let bytes = self.value.as_bytes();
        let unit = separator_index(bytes)
            .and_then(|separator| bytes.get(..separator))
            .ok_or_else(|| invalid_syntax(&FieldName::Range))?;
        str::from_utf8(trim_ows(unit)).map_err(|_invalid| invalid_syntax(&FieldName::Range))
    }

    /// Returns whether the range unit is `bytes`, case-insensitively.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::RangeOwned::try_from("bytes=0-99")?;
    /// assert!(value.is_bytes());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn is_bytes(&self) -> bool {
        self.bytes
    }

    /// Iterates byte range specifications, or returns `None` for an extension unit.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{ByteRangeSpec, RangeOwned};
    ///
    /// let value = RangeOwned::try_from("bytes=0-49, 100-, -500")?;
    /// let specs = value
    ///     .byte_ranges()
    ///     .expect("byte ranges")
    ///     .collect::<Vec<_>>();
    /// assert_eq!(
    ///     specs,
    ///     [
    ///         ByteRangeSpec::FromTo { first: 0, last: 49 },
    ///         ByteRangeSpec::From { first: 100 },
    ///         ByteRangeSpec::Suffix { length: 500 },
    ///     ]
    /// );
    ///
    /// let extension = RangeOwned::extension("items", "1-5")?;
    /// assert!(extension.byte_ranges().is_none());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn byte_ranges(&self) -> Option<impl Iterator<Item = ByteRangeSpec> + '_> {
        self.range_set().map(ByteRangeIter::new).filter(|_iterator| self.bytes)
    }

    /// Returns an extension range-set without normalization.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::RangeOwned;
    ///
    /// let extension = RangeOwned::extension("items", "1-5")?;
    /// assert_eq!(extension.extension_range_set(), Some(&b"1-5"[..]));
    ///
    /// let bytes = RangeOwned::try_from("bytes=0-99")?;
    /// assert_eq!(bytes.extension_range_set(), None);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn extension_range_set(&self) -> Option<&[u8]> {
        if self.bytes { None } else { self.range_set() }
    }

    #[inline]
    fn range_set(&self) -> Option<&[u8]> {
        let bytes = self.value.as_bytes();
        bytes.get(separator_index(bytes)?.saturating_add(1)..).map(trim_ows)
    }

    /// Returns the preserved field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::RangeOwned;
    ///
    /// let value = RangeOwned::try_from("bytes=0-99")?;
    /// assert_eq!(value.as_field_value().as_bytes(), b"bytes=0-99");
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
    /// use http_headers::headers::RangeOwned;
    ///
    /// let value = RangeOwned::try_from("bytes=-500")?;
    /// let field_value = value.into_field_value();
    /// assert_eq!(field_value.as_bytes(), b"bytes=-500");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }
}

super::super::shared::impl_field_value_conversion!(RangeOwned, |value| value.value);

impl<'a> RangeView<'a> {
    /// Returns the range unit exactly as received.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::Range;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view = Range::decode_view(FieldValueRef::new(b"items=1-5"))?;
    /// assert_eq!(view.unit(), "items");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn unit(self) -> &'a str {
        self.unit
    }

    /// Returns whether the range unit is `bytes`, case-insensitively.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::RangeOwned::try_from("bytes=0-99")?;
    /// assert!(value.is_bytes());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn is_bytes(self) -> bool {
        self.bytes
    }

    /// Iterates byte range specifications, or returns `None` for an extension unit.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{ByteRangeSpec, Range};
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view = Range::decode_view(FieldValueRef::new(b"bytes=0-49, 100-, -500"))?;
    /// let specs = view.byte_ranges().expect("byte ranges").collect::<Vec<_>>();
    /// assert_eq!(
    ///     specs,
    ///     [
    ///         ByteRangeSpec::FromTo { first: 0, last: 49 },
    ///         ByteRangeSpec::From { first: 100 },
    ///         ByteRangeSpec::Suffix { length: 500 },
    ///     ]
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn byte_ranges(self) -> Option<impl Iterator<Item = ByteRangeSpec> + 'a> {
        self.bytes.then(|| ByteRangeIter::new(self.range_set))
    }

    /// Returns the extension range-set bytes without normalization.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::Range;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let extension = Range::decode_view(FieldValueRef::new(b"items=1-5"))?;
    /// assert_eq!(extension.extension_range_set(), Some(&b"1-5"[..]));
    ///
    /// let bytes = Range::decode_view(FieldValueRef::new(b"bytes=0-99"))?;
    /// assert_eq!(bytes.extension_range_set(), None);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn extension_range_set(self) -> Option<&'a [u8]> {
        if self.bytes { None } else { Some(self.range_set) }
    }

    /// Returns the borrowed field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::Range;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view = Range::decode_view(FieldValueRef::new(b"bytes=100-"))?;
    /// assert_eq!(view.as_field_value().as_bytes(), b"bytes=100-");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn as_field_value(self) -> FieldValueRef<'a> {
        self.value
    }
}

impl SingleValueField for Range {
    type View<'a> = RangeView<'a>;
    type Owned = RangeOwned;

    fn name() -> &'static FieldName {
        &FieldName::Range
    }

    #[inline]
    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
        let parsed = parse_range(value.as_bytes())?;
        Ok(RangeView {
            value,
            unit: parsed.unit,
            range_set: parsed.payload,
            bytes: parsed.bytes,
        })
    }

    #[expect(
        clippy::inline_always,
        reason = "measured: Criterion otherwise outlines this conversion while Callgrind inlines it"
    )]
    #[inline(always)]
    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
        RangeOwned::try_from(value)
    }

    fn decode_view_with(value: FieldValueRef<'_>, mode: crate::DecodeMode) -> Result<Self::View<'_>, DecodeError> {
        let parsed = parse_range_with(value.as_bytes(), mode)?;
        Ok(RangeView {
            value,
            unit: parsed.unit,
            range_set: parsed.payload,
            bytes: parsed.bytes,
        })
    }

    fn decode_owned_with(value: FieldValue, mode: crate::DecodeMode) -> Result<Self::Owned, DecodeError> {
        let bytes = parse_range_with(value.as_bytes(), mode)?.bytes;
        Ok(RangeOwned { value, bytes })
    }

    fn as_field_value(value: &Self::Owned) -> &FieldValue {
        &value.value
    }

    fn into_field_value(value: Self::Owned) -> FieldValue {
        value.value
    }
}

super::super::shared::impl_string_conversions!(RangeOwned, &FieldName::Range, invalid_syntax, wire);

impl TryFrom<FieldValue> for RangeOwned {
    type Error = DecodeError;

    #[expect(
        clippy::inline_always,
        reason = "measured: outlining this conversion leaves large Result traffic in Criterion"
    )]
    #[inline(always)]
    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        let bytes = parse_range(value.as_bytes())?.bytes;
        Ok(Self { value, bytes })
    }
}

/// Locates the `=` that separates the range unit from the range set.
fn separator_index(bytes: &[u8]) -> Option<usize> {
    bytes.iter().position(|byte| *byte == b'=')
}

pub(super) struct ParsedRange<'a> {
    pub(super) unit: &'a str,
    pub(super) payload: &'a [u8],
    pub(super) bytes: bool,
}

#[expect(
    clippy::inline_always,
    reason = "measured: Callgrind inlines this parser while Criterion otherwise emits a real call"
)]
#[inline(always)]
pub(super) fn parse_range(bytes: &[u8]) -> Result<ParsedRange<'_>, DecodeError> {
    if let Some(payload) = bytes.strip_prefix(b"bytes=") {
        if !http_headers_simd::scan_byte_range_set(bytes, 6) {
            validate_byte_range_set_wide(bytes, 6)?;
        }
        return Ok(ParsedRange {
            unit: "bytes",
            payload,
            bytes: true,
        });
    }
    parse_range_slow(bytes)
}

#[cold]
#[inline(never)]
pub(super) fn parse_range_slow(bytes: &[u8]) -> Result<ParsedRange<'_>, DecodeError> {
    let Some(separator) = bytes.iter().position(|byte| *byte == b'=') else {
        return Err(invalid_syntax(&FieldName::Range));
    };
    let unit_bytes = &bytes[..separator];
    let payload = &bytes[separator + 1..];
    validate_range_unit_for(unit_bytes, &FieldName::Range)?;
    let unit = ascii_str(unit_bytes).expect("validated range units are ASCII");
    let is_bytes = validate::eq_ignore_ascii_case(unit_bytes, b"bytes");
    if is_bytes {
        validate_byte_range_set(bytes, separator + 1)?;
    } else if !valid_extension_payload(payload) {
        return Err(invalid_syntax(&FieldName::Range));
    }
    Ok(ParsedRange {
        unit,
        payload,
        bytes: is_bytes,
    })
}

struct ByteRangeIter<'a> {
    remaining: &'a [u8],
}

impl<'a> ByteRangeIter<'a> {
    const fn new(remaining: &'a [u8]) -> Self {
        Self { remaining }
    }
}

impl Iterator for ByteRangeIter<'_> {
    type Item = ByteRangeSpec;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.remaining.is_empty() {
                return None;
            }
            let (item, remaining) = if let Some(separator) = self.remaining.iter().position(|byte| *byte == b',') {
                (&self.remaining[..separator], &self.remaining[separator + 1..])
            } else {
                (self.remaining, &[] as &[u8])
            };
            self.remaining = remaining;
            let item = trim_ows(item);
            if item.is_empty() {
                continue;
            }
            return parse_byte_spec_relaxed(item).ok();
        }
    }
}

#[inline]
pub(super) fn validate_byte_range_set(bytes: &[u8], start: usize) -> Result<(), DecodeError> {
    // The accelerated scanner settles any set that fits one classification
    // window; the word-at-a-time scanner covers the longer ones.
    if http_headers_simd::scan_byte_range_set(bytes, start) {
        return Ok(());
    }
    validate_byte_range_set_wide(bytes, start)
}

/// Validates a range-set that the one-window scanner could not settle.
#[cold]
#[inline(never)]
fn validate_byte_range_set_wide(bytes: &[u8], start: usize) -> Result<(), DecodeError> {
    if scan_byte_range_set(bytes, start) {
        return Ok(());
    }
    validate_byte_range_set_slow(bytes.get(start..).unwrap_or_default())
}

/// Marks every byte of `word` that is not an ASCII digit.
///
/// The low seven bits are biased so that only digits stay below `0x80`, and
/// the original word is folded back in so that `obs-text` bytes never borrow
/// into their neighbor.
#[inline]
const fn nondigit_bits(word: u64) -> u64 {
    // Repeated-byte SWAR masks: clear high bits, align ASCII zero, then bias
    // values outside `0..=9` into each lane's high bit.
    const LOW: u64 = 0x7f7f_7f7f_7f7f_7f7f;
    const ZEROS: u64 = 0x3030_3030_3030_3030;
    const BIAS: u64 = 0x7676_7676_7676_7676;

    (((word & LOW) ^ ZEROS).wrapping_add(BIAS) | word) & !LOW
}

/// Counts the leading ASCII digits marked by `nondigit`, saturating at eight.
#[inline]
const fn digit_run(nondigit: u64) -> usize {
    (nondigit.trailing_zeros() >> 3) as usize
}

/// Reads the eight bytes at `at`, zero-padded past the end of `bytes`.
///
/// Callers guarantee `8 <= bytes.len()` and `at < bytes.len()`, so a window
/// that would overrun is taken from the end of `bytes` and shifted into place.
#[inline]
fn word_at(bytes: &[u8], at: usize) -> u64 {
    if let Some(chunk) = bytes.get(at..at.saturating_add(8)) {
        return u64::from_le_bytes(<[u8; 8]>::try_from(chunk).unwrap_or_default());
    }
    let offset = bytes.len().saturating_sub(8);
    let tail = <[u8; 8]>::try_from(bytes.get(offset..).unwrap_or_default()).unwrap_or_default();
    u64::from_le_bytes(tail) >> ((at.wrapping_sub(offset) & 7) * 8)
}

/// Validates the range-set of `bytes` that starts at `start`.
///
/// Returns `false` for anything unusual so the general implementation can
/// decide between acceptance and the precise error it reports.
#[expect(
    clippy::cast_possible_truncation,
    reason = "item lengths and single bytes are extracted from a machine word"
)]
#[inline]
fn scan_byte_range_set(bytes: &[u8], start: usize) -> bool {
    if bytes.len() < 8 {
        return false;
    }
    let mut at = start;
    loop {
        if at >= bytes.len() {
            return false;
        }
        let word = word_at(bytes, at);
        let Some(item) = scan_byte_range_spec(word) else {
            return false;
        };
        let end = at + item;
        if end == bytes.len() {
            return true;
        }

        // The delimiter fits the word, but following OWS may lie beyond it.
        let rest = word.rotate_right((item as u32) << 3);
        if rest as u8 != b',' {
            return false;
        }
        at = end + 1;
        if matches!(bytes.get(at), Some(b' ' | b'\t')) {
            at += 1;
        }
    }
}

/// Returns the length of the byte-range-spec that starts `word`.
///
/// Specs whose digits could continue past the eight byte window, that carry a
/// leading zero on a compared position, or that are inverted return `None`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "digit runs never exceed eight and single bytes are extracted from a word"
)]
#[inline]
fn scan_byte_range_spec(word: u64) -> Option<usize> {
    /// Longest digit run that always leaves room for a terminator.
    const MAX_RUN: usize = 6;

    let nondigit = nondigit_bits(word);
    let first_len = digit_run(nondigit);
    if first_len == 0 {
        if word as u8 != b'-' {
            return None;
        }
        let suffix_len = digit_run(nondigit.rotate_right(8));
        if suffix_len == 0 || suffix_len > MAX_RUN {
            return None;
        }
        return Some(1 + suffix_len);
    }
    if first_len > MAX_RUN {
        return None;
    }

    // Rotating keeps the vacated bytes outside every window this function
    // inspects, so an over-long run always trips the `MAX_RUN` guard.
    let shift = (first_len as u32) << 3;
    if word.rotate_right(shift) as u8 != b'-' {
        return None;
    }
    let last = word.rotate_right(shift + 8);
    let last_len = digit_run(nondigit.rotate_right(shift + 8));
    if first_len + last_len > MAX_RUN {
        return None;
    }
    if last_len != 0 {
        // A leading zero would let the shorter run compare as the smaller
        // number even when it is not, so leave those to the general path.
        if last_len > 1 && last as u8 == b'0' {
            return None;
        }
        if first_len > last_len {
            return None;
        }
        if first_len == last_len {
            let mask = u64::MAX >> ((8 - first_len) * 8);
            if (word & mask).swap_bytes() > (last & mask).swap_bytes() {
                return None;
            }
        }
    }
    Some(first_len + 1 + last_len)
}

#[cold]
#[inline(never)]
pub(super) fn validate_byte_range_set_slow(bytes: &[u8]) -> Result<(), DecodeError> {
    let mut count = 0_usize;
    for item in bytes.split(|byte| *byte == b',') {
        let item = trim_ows(item);
        if item.is_empty() {
            continue;
        }
        let _spec = parse_byte_spec(item)?;
        count += 1;
    }
    if count == 0 {
        Err(DecodeError::new(&FieldName::Range, DecodeErrorKind::MissingValue))
    } else {
        Ok(())
    }
}

fn parse_byte_spec(bytes: &[u8]) -> Result<ByteRangeSpec, DecodeError> {
    if let Some(suffix) = bytes.strip_prefix(b"-") {
        return parse_number(suffix, &FieldName::Range).map(|length| ByteRangeSpec::Suffix { length });
    }
    let Some(separator) = bytes.iter().position(|byte| *byte == b'-') else {
        return Err(invalid_syntax(&FieldName::Range));
    };
    if bytes[separator + 1..].contains(&b'-') {
        return Err(invalid_syntax(&FieldName::Range));
    }
    let first = parse_number(&bytes[..separator], &FieldName::Range)?;
    let last = &bytes[separator + 1..];
    if last.is_empty() {
        Ok(ByteRangeSpec::From { first })
    } else {
        let last = parse_number(last, &FieldName::Range)?;
        ByteRangeSpec::from_range(first..=last)
    }
}

fn append_byte_spec(wire: &mut String, spec: ByteRangeSpec) -> Result<(), DecodeError> {
    match spec {
        ByteRangeSpec::FromTo { first, last } => {
            if last < first {
                return Err(invalid_syntax(&FieldName::Range));
            }
            write!(wire, "{first}").expect("writing to a String is infallible");
            wire.push('-');
            write!(wire, "{last}").expect("writing to a String is infallible");
        }
        ByteRangeSpec::From { first } => {
            write!(wire, "{first}").expect("writing to a String is infallible");
            wire.push('-');
        }
        ByteRangeSpec::Suffix { length } => {
            wire.push('-');
            write!(wire, "{length}").expect("writing to a String is infallible");
        }
    }
    Ok(())
}

fn validated_byte_range_value(wire: String) -> FieldValue {
    FieldValue::try_from(wire).expect("formatted byte ranges form a valid field value")
}

fn validated_extension_range_value(wire: String) -> FieldValue {
    FieldValue::try_from(wire).expect("validated extension ranges form a valid field value")
}

fn valid_extension_payload(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.iter().all(|byte| matches!(byte, 0x21..=0x7e))
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
        ByteRangeIter, ByteRangeSpec, Range, RangeOwned, digit_run, nondigit_bits, parse_byte_spec, parse_byte_spec_relaxed, parse_range,
        parse_range_with, scan_byte_range_set, scan_byte_range_spec, valid_extension_payload, validate_byte_range_set_relaxed,
        validate_byte_range_set_slow, word_at,
    };
    use crate::{DecodeErrorKind, DecodeMode, FieldName, FieldValue, FieldValueRef, SingleValueField};

    fn word(bytes: &[u8]) -> u64 {
        u64::from_le_bytes(bytes.try_into().expect("test words contain eight bytes"))
    }

    #[test]
    fn byte_range_bounds_cover_closed_open_suffix_and_error_cases() {
        assert_eq!(
            ByteRangeSpec::from_to(0, 9).expect("closed range"),
            ByteRangeSpec::FromTo { first: 0, last: 9 }
        );
        assert_eq!(
            ByteRangeSpec::from_range(0..10).expect("exclusive end"),
            ByteRangeSpec::FromTo { first: 0, last: 9 }
        );
        assert_eq!(ByteRangeSpec::from_range(5..).expect("open end"), ByteRangeSpec::From { first: 5 });
        assert_eq!(
            ByteRangeSpec::from_range((Bound::Excluded(4), Bound::Included(9))).expect("excluded start"),
            ByteRangeSpec::FromTo { first: 5, last: 9 }
        );
        assert_eq!(ByteRangeSpec::starting_at(7), ByteRangeSpec::From { first: 7 });
        assert_eq!(ByteRangeSpec::suffix(12), ByteRangeSpec::Suffix { length: 12 });

        for result in [
            ByteRangeSpec::from_to(9, 0),
            ByteRangeSpec::from_range((Bound::Unbounded, Bound::Included(1))),
            ByteRangeSpec::from_range((Bound::Excluded(u64::MAX), Bound::Unbounded)),
            ByteRangeSpec::from_range((Bound::Included(0), Bound::Excluded(0))),
        ] {
            assert_eq!(result.expect_err("invalid bounds").kind(), DecodeErrorKind::InvalidSyntax);
        }
    }

    #[test]
    #[expect(deprecated, reason = "the compatibility constructor remains covered")]
    fn deprecated_open_range_constructor_delegates_to_starting_at() {
        assert_eq!(ByteRangeSpec::from(7), ByteRangeSpec::starting_at(7));
    }

    #[test]
    fn owned_range_constructors_and_accessors_preserve_semantics() {
        let owned = RangeOwned::bytes(vec![
            ByteRangeSpec::FromTo { first: 0, last: 9 },
            ByteRangeSpec::From { first: 20 },
            ByteRangeSpec::Suffix { length: 5 },
        ])
        .expect("valid byte ranges");
        assert_eq!(owned.unit(), Ok("bytes"));
        assert!(owned.is_bytes());
        assert_eq!(
            owned.byte_ranges().expect("byte iterator").collect::<Vec<_>>(),
            [
                ByteRangeSpec::FromTo { first: 0, last: 9 },
                ByteRangeSpec::From { first: 20 },
                ByteRangeSpec::Suffix { length: 5 },
            ]
        );
        assert_eq!(owned.extension_range_set(), None);
        assert_eq!(owned.as_field_value().as_bytes(), b"bytes=0-9, 20-, -5");
        assert_eq!(owned.clone().into_field_value().as_bytes(), b"bytes=0-9, 20-, -5");
        assert!(format!("{owned:?}").contains("is_bytes: true"));

        assert_eq!(
            RangeOwned::bytes(Vec::<ByteRangeSpec>::new()).expect_err("empty set").kind(),
            DecodeErrorKind::MissingValue
        );
        assert_eq!(
            RangeOwned::bytes(vec![ByteRangeSpec::FromTo { first: 2, last: 1 }])
                .expect_err("inverted range")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let extension = RangeOwned::extension("items", "opaque-set").expect("extension range");
        assert_eq!(extension.unit(), Ok("items"));
        assert!(!extension.is_bytes());
        assert!(extension.byte_ranges().is_none());
        assert_eq!(extension.extension_range_set(), Some(b"opaque-set".as_slice()));
        assert_eq!(
            RangeOwned::extension("bad unit", "opaque").expect_err("invalid unit").kind(),
            DecodeErrorKind::InvalidToken
        );
        assert_eq!(
            RangeOwned::extension("items", "").expect_err("empty payload").kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            RangeOwned::try_from(String::from("line\nbreak"))
                .expect_err("invalid field value")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }

    #[test]
    fn strict_and_relaxed_parsers_cover_byte_and_extension_paths() {
        assert!(RangeOwned::try_from("bytes=0-1").expect("valid borrowed range").is_bytes());
        let strict = parse_range(b"bytes=0-9, 20-, -5").expect("strict byte set");
        assert_eq!(strict.unit, "bytes");
        assert_eq!(strict.payload, b"0-9, 20-, -5");
        assert!(strict.bytes);

        let extension = parse_range(b"items=opaque").expect("extension range");
        assert_eq!(extension.unit, "items");
        assert!(!extension.bytes);
        assert!(parse_range(b"missing-separator").is_err());
        assert!(parse_range(b"bad unit=opaque").is_err());
        assert!(parse_range(b"items=").is_err());
        assert!(parse_range(b"bytes=").is_err());
        assert!(parse_range(b"bytes=1--2").is_err());
        assert!(parse_range(b"bytes=2-1").is_err());
        let strict_mode = parse_range_with(b"bytes=0-1", DecodeMode::Strict).expect("explicit strict mode");
        assert!(strict_mode.bytes);
        let relaxed_fast_path = parse_range_with(b"bytes=0-1", DecodeMode::Relaxed).expect("strict syntax remains valid in relaxed mode");
        assert!(relaxed_fast_path.bytes);

        let relaxed = parse_range_with(b" Bytes = 0 - 9 , - 5 ", DecodeMode::Relaxed).expect("relaxed whitespace");
        assert_eq!(relaxed.unit, "Bytes");
        assert_eq!(relaxed.payload, b"0 - 9 , - 5");
        assert!(relaxed.bytes);
        assert!(parse_range_with(b" = 0-1", DecodeMode::Relaxed).is_err());
        assert!(parse_range_with(b"bytes 0-1", DecodeMode::Relaxed).is_err());
        assert!(parse_range_with(b"bytes = , ,", DecodeMode::Relaxed).is_err());
        assert!(parse_range_with(b"items = ", DecodeMode::Relaxed).is_err());
        assert!(validate_byte_range_set_relaxed(b"0 - 1, - 2, 3 -").is_ok());
        assert!(validate_byte_range_set_relaxed(b"x - 1").is_err());
        assert!(parse_byte_spec_relaxed(b"x - 1").is_err());
        assert!(parse_byte_spec_relaxed(b"1 - x").is_err());
        assert!(<Range as SingleValueField>::decode_view(FieldValueRef::new(b"invalid")).is_err());
        assert!(<Range as SingleValueField>::decode_view_with(FieldValueRef::new(b"invalid"), DecodeMode::Relaxed,).is_err());
        assert!(<Range as SingleValueField>::decode_owned_with(FieldValue::from_static("invalid"), DecodeMode::Relaxed,).is_err());
    }

    #[test]
    fn byte_spec_parsers_and_iterator_skip_empty_members_and_stop_at_invalid_ones() {
        assert_eq!(parse_byte_spec(b"-10").expect("suffix"), ByteRangeSpec::Suffix { length: 10 });
        assert_eq!(parse_byte_spec(b"10-").expect("open range"), ByteRangeSpec::From { first: 10 });
        assert_eq!(
            parse_byte_spec(b"10-20").expect("closed range"),
            ByteRangeSpec::FromTo { first: 10, last: 20 }
        );
        for malformed in [b"10".as_slice(), b"1--2", b"-", b"2-1"] {
            assert!(parse_byte_spec(malformed).is_err(), "{malformed:?}");
        }
        assert_eq!(
            parse_byte_spec_relaxed(b" 10 - 20 ").expect("relaxed closed range"),
            ByteRangeSpec::FromTo { first: 10, last: 20 }
        );
        assert_eq!(
            parse_byte_spec_relaxed(b" - 5 ").expect("relaxed suffix"),
            ByteRangeSpec::Suffix { length: 5 }
        );
        assert!(parse_byte_spec_relaxed(b"1--2").is_err());
        assert!(parse_byte_spec_relaxed(b"not-a-range").is_err());

        assert_eq!(
            ByteRangeIter::new(b", 0-1, , -5, 9-, invalid").collect::<Vec<_>>(),
            [
                ByteRangeSpec::FromTo { first: 0, last: 1 },
                ByteRangeSpec::Suffix { length: 5 },
                ByteRangeSpec::From { first: 9 },
            ]
        );
    }

    #[test]
    fn borrowed_views_cover_byte_and_extension_accessors() {
        let byte_view = <Range as SingleValueField>::decode_view(FieldValueRef::new(b"bytes=0-1")).expect("borrowed byte range");
        assert_eq!(byte_view.unit(), "bytes");
        assert!(byte_view.is_bytes());
        assert_eq!(
            byte_view.byte_ranges().expect("byte ranges").collect::<Vec<_>>(),
            [ByteRangeSpec::FromTo { first: 0, last: 1 }]
        );
        assert_eq!(byte_view.extension_range_set(), None);
        assert_eq!(byte_view.as_field_value().as_bytes(), b"bytes=0-1");
        assert!(format!("{byte_view:?}").contains("is_bytes: true"));

        let value = FieldValue::from_static("items=opaque");
        let extension = <Range as SingleValueField>::decode_owned(value).expect("owned extension");
        assert_eq!(extension.extension_range_set(), Some(b"opaque".as_slice()));
        let extension_view = <Range as SingleValueField>::decode_view_with(FieldValueRef::new(b" items = opaque "), DecodeMode::Relaxed)
            .expect("relaxed extension");
        assert_eq!(extension_view.unit(), "items");
        assert_eq!(extension_view.extension_range_set(), Some(b"opaque".as_slice()));
        assert!(extension_view.byte_ranges().is_none());

        let decoded = <Range as SingleValueField>::decode_owned(FieldValue::from_static("bytes=0-1")).expect("owned byte range");
        assert_eq!(<Range as SingleValueField>::as_field_value(&decoded).as_bytes(), b"bytes=0-1");
        assert_eq!(<Range as SingleValueField>::into_field_value(decoded).as_bytes(), b"bytes=0-1");
        let relaxed_owned = <Range as SingleValueField>::decode_owned_with(FieldValue::from_static("bytes = 0 - 1"), DecodeMode::Relaxed)
            .expect("relaxed owned range");
        assert!(relaxed_owned.is_bytes());
        assert_eq!(
            RangeOwned::try_from("line\nbreak")
                .expect_err("invalid borrowed field value")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        let from_string = RangeOwned::try_from(String::from("bytes=0-1")).expect("owned string conversion");
        assert!(from_string.is_bytes());
        assert_eq!(<Range as SingleValueField>::name(), &FieldName::Range);
    }

    #[test]
    fn defensive_owned_accessors_revalidate_private_wire_storage() {
        let missing_separator = RangeOwned {
            value: FieldValue::from_static("bytes"),
            bytes: true,
        };
        assert_eq!(
            missing_separator.unit().expect_err("unit separator is missing").kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert!(missing_separator.byte_ranges().is_none());

        let invalid_utf8 = FieldValue::from_bytes(b"\xff=opaque").expect("obs-text field value");
        let invalid_utf8 = RangeOwned {
            value: invalid_utf8,
            bytes: false,
        };
        assert_eq!(
            invalid_utf8.unit().expect_err("unit is not UTF-8").kind(),
            DecodeErrorKind::InvalidSyntax
        );

        assert_eq!(
            <Range as SingleValueField>::decode_owned(FieldValue::from_static("bad range"))
                .expect_err("invalid owned range")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }

    #[test]
    fn range_unit_projection_preserves_opaque_payloads_and_rejects_unicode_units() {
        for (wire, mode, unit, payload) in [
            (
                b"items=first:last".as_slice(),
                DecodeMode::Strict,
                "items",
                b"first:last".as_slice(),
            ),
            (b" Items = first:last ", DecodeMode::Relaxed, "Items", b"first:last"),
            (b"Bytes=0-9", DecodeMode::Strict, "Bytes", b"0-9"),
            (b" BYTES = 0 - 9 ", DecodeMode::Relaxed, "BYTES", b"0 - 9"),
        ] {
            let parsed = parse_range_with(wire, mode).unwrap();
            assert_eq!(parsed.unit, unit);
            assert_eq!(parsed.payload, payload);
        }
        for wire in [
            b"it\xc3\xa9ms=opaque".as_slice(),
            b"\xff=opaque",
            b"Byt\x80s=0-9",
            b"items=\xff\x80",
        ] {
            for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
                assert!(parse_range_with(wire, mode).is_err(), "{wire:?}");
            }
        }
    }

    #[test]
    fn word_scanner_reads_ows_after_full_windows() {
        for wire in [
            "100-109, 200-209",
            "100-109,\t200-209",
            "100-109,200-209",
            "-123456, 200-209",
            "123456-, 200-209",
        ] {
            assert!(scan_byte_range_set(wire.as_bytes(), 0), "{wire}");
            validate_byte_range_set_slow(wire.as_bytes()).unwrap();
        }
        for separator in u8::MIN..=u8::MAX {
            let mut wire = b"100-109,".to_vec();
            wire.push(separator);
            wire.extend_from_slice(b"200-209");
            if scan_byte_range_set(&wire, 0) {
                assert!(validate_byte_range_set_slow(&wire).is_ok(), "{wire:?}");
            }
        }
        for wire in ["100-109, \t200-209", "100-109, ", "100-109, 300-299", "100-109,\t"] {
            assert!(!scan_byte_range_set(wire.as_bytes(), 0), "{wire}");
        }
    }

    #[test]
    fn word_scanner_helpers_cover_window_boundaries_and_rejections() {
        let digits = nondigit_bits(word(b"12345678"));
        assert_eq!(digit_run(digits), 8);
        assert_eq!(digit_run(nondigit_bits(word(b"12-45678"))), 2);

        assert_eq!(word_at(b"12345678", 0), word(b"12345678"));
        assert_eq!(word_at(b"123456789", 7) & 0xff, u64::from(b'8'));
        assert_eq!(scan_byte_range_spec(word(b"-12,rest")), Some(3));
        assert_eq!(scan_byte_range_spec(word(b"12-,rest")), Some(3));
        assert_eq!(scan_byte_range_spec(word(b"12-34,re")), Some(5));
        for rejected in [
            word(b"x2-3,res"),
            word(b"-x,rest!"),
            word(b"-1234567"),
            word(b"1234567-"),
            word(b"12x34,re"),
            word(b"12-034,r"),
            word(b"123-12,r"),
            word(b"34-12,re"),
        ] {
            assert_eq!(scan_byte_range_spec(rejected), None);
        }
        assert!(scan_byte_range_set(b"0-1, 2-3", 0));
        assert!(!scan_byte_range_set(b"short", 0));
        assert!(!scan_byte_range_set(b"0-1; 2-3", 0));
        assert!(valid_extension_payload(b"!opaque~"));
        assert!(!valid_extension_payload(b""));
        assert!(!valid_extension_payload(b"has space"));
    }
}
