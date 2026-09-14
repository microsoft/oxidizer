// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Strong and weak entity-tag parsing and construction.

use crate::{DecodeError, DecodeMode, FieldName, FieldValue, FieldValueRef, SingleValueField};

/// Defines the `ETag` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 8.8.3](https://www.rfc-editor.org/rfc/rfc9110#section-8.8.3).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{ETag, ETagOwned};
///
/// let mut map = HeaderMap::new();
/// ETag::insert(&mut map, ETagOwned::weak("revision-42")?)?;
/// assert!(ETag::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct ETag {
    _private: (),
}

/// Owned value for the `ETag` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 8.8.3].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::ETagOwned::weak("revision-42")?;
/// assert_eq!(value.opaque_tag()?, b"revision-42");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `ETag: "revision-42"` is a strong validator.
/// `ETag: W/"revision-42"` is a weak validator; `ETag: ""` is also valid.
///
/// [RFC 9110 section 8.8.3]: https://www.rfc-editor.org/rfc/rfc9110#section-8.8.3
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ETagOwned {
    value: FieldValue,
}

/// Borrowed value for the `ETag` header.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{ETag, ETagView};
/// use http_headers::{FieldValue, SingleValueField};
///
/// let wire = FieldValue::from_static(r#""revision""#);
/// let view: ETagView<'_> = <ETag as SingleValueField>::decode_view(wire.as_field_value_ref())?;
/// assert_eq!(view.opaque_tag(), b"revision");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct ETagView<'a> {
    value: FieldValueRef<'a>,
}

impl ETagOwned {
    /// Constructs a strong entity tag from unquoted opaque text.
    ///
    /// # Errors
    ///
    /// Returns an error if the opaque tag contains a byte forbidden by the
    /// entity-tag grammar.
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::ETagOwned::strong("revision")?;
    /// assert_eq!(value.opaque_tag()?, b"revision");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn strong(opaque: impl AsRef<str>) -> Result<Self, DecodeError> {
        construct(opaque.as_ref().as_bytes(), false)
    }

    /// Constructs a weak entity tag from unquoted opaque text.
    ///
    /// # Errors
    ///
    /// Returns an error if the opaque tag contains a byte forbidden by the
    /// entity-tag grammar.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ETagOwned;
    ///
    /// let value = ETagOwned::weak("revision")?;
    /// assert!(value.is_weak());
    /// assert_eq!(value.opaque_tag()?, b"revision");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn weak(opaque: impl AsRef<str>) -> Result<Self, DecodeError> {
        construct(opaque.as_ref().as_bytes(), true)
    }

    /// Parses a complete wire-format entity tag.
    ///
    /// # Errors
    ///
    /// Returns an error when `wire` is not a valid entity tag.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ETagOwned;
    ///
    /// let value = ETagOwned::try_from_wire(r#"W/"revision""#)?;
    /// assert!(value.is_weak());
    /// assert_eq!(value.opaque_tag()?, b"revision");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn try_from_wire(wire: impl AsRef<str>) -> Result<Self, DecodeError> {
        Self::try_from(wire.as_ref())
    }

    /// Returns whether this entity tag is weak.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ETagOwned;
    ///
    /// let strong = ETagOwned::strong("revision")?;
    /// let weak = ETagOwned::weak("revision")?;
    /// assert!(!strong.is_weak());
    /// assert!(weak.is_weak());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn is_weak(&self) -> bool {
        matches!(self.value.as_bytes().first(), Some(b'W' | b'w'))
    }

    /// Returns the unquoted opaque tag bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the stored range and wire value disagree.
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::ETagOwned::strong("revision")?;
    /// assert_eq!(value.opaque_tag()?, b"revision");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn opaque_tag(&self) -> Result<&[u8], DecodeError> {
        let start = if self.is_weak() { 3 } else { 1 };
        let end = self
            .value
            .as_bytes()
            .len()
            .checked_sub(1)
            .ok_or_else(|| super::invalid_syntax(&FieldName::Etag))?;
        self.value
            .as_bytes()
            .get(start..end)
            .ok_or_else(|| super::invalid_syntax(&FieldName::Etag))
    }

    /// Performs the strong entity-tag comparison.
    ///
    /// # Errors
    ///
    /// Returns an error if either stored range disagrees with its wire value.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ETagOwned;
    ///
    /// let a = ETagOwned::try_from(r#""xyzzy""#)?;
    /// let b = ETagOwned::try_from(r#""xyzzy""#)?;
    /// let weak = ETagOwned::try_from(r#"W/"xyzzy""#)?;
    /// assert!(a.strong_eq(&b)?);
    /// assert!(!a.strong_eq(&weak)?);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn strong_eq(&self, other: &Self) -> Result<bool, DecodeError> {
        Ok(!self.is_weak() && !other.is_weak() && self.opaque_tag()? == other.opaque_tag()?)
    }

    /// Performs the weak entity-tag comparison.
    ///
    /// # Errors
    ///
    /// Returns an error if either stored range disagrees with its wire value.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ETagOwned;
    ///
    /// let strong = ETagOwned::strong("xyzzy")?;
    /// let weak = ETagOwned::weak("xyzzy")?;
    /// assert!(!strong.strong_eq(&weak)?);
    /// assert!(strong.weak_eq(&weak)?);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn weak_eq(&self, other: &Self) -> Result<bool, DecodeError> {
        Ok(self.opaque_tag()? == other.opaque_tag()?)
    }

    /// Returns reusable wire storage.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldValue;
    /// use http_headers::headers::ETagOwned;
    ///
    /// let value = ETagOwned::strong("revision")?;
    /// let field_value = value.into_field_value();
    /// assert_eq!(field_value, FieldValue::from_static(r#""revision""#));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }
}

super::shared::impl_field_value_conversion!(ETagOwned, |value| value.value);

impl<'a> ETagView<'a> {
    pub(crate) const fn field_value(self) -> FieldValueRef<'a> {
        self.value
    }

    /// Returns whether this entity tag is weak.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ETag;
    /// use http_headers::{FieldValue, SingleValueField};
    ///
    /// let wire = FieldValue::from_static(r#"W/"revision""#);
    /// let view = <ETag as SingleValueField>::decode_view(wire.as_field_value_ref())?;
    /// assert!(view.is_weak());
    /// assert_eq!(view.opaque_tag(), b"revision");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn is_weak(self) -> bool {
        matches!(self.value.as_bytes().first(), Some(b'W' | b'w'))
    }

    /// Returns the unquoted opaque tag bytes.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::ETagOwned::strong("revision")?;
    /// assert_eq!(value.opaque_tag()?, b"revision");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn opaque_tag(self) -> &'a [u8] {
        let bytes = self.value.as_bytes();
        let start = if self.is_weak() { 3 } else { 1 };
        let (_, opaque_and_quote) = bytes.split_at(start);
        let (opaque, _) = opaque_and_quote.split_at(opaque_and_quote.len() - 1);
        opaque
    }

    /// Performs the strong entity-tag comparison.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ETag;
    /// use http_headers::{FieldValue, SingleValueField};
    ///
    /// let current = FieldValue::from_static(r#""revision""#);
    /// let same = FieldValue::from_static(r#""revision""#);
    /// let weak = FieldValue::from_static(r#"W/"revision""#);
    /// let current = <ETag as SingleValueField>::decode_view(current.as_field_value_ref())?;
    /// let same = <ETag as SingleValueField>::decode_view(same.as_field_value_ref())?;
    /// let weak = <ETag as SingleValueField>::decode_view(weak.as_field_value_ref())?;
    /// assert!(current.strong_eq(same));
    /// assert!(!current.strong_eq(weak));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn strong_eq(self, other: Self) -> bool {
        !self.is_weak() && !other.is_weak() && self.opaque_tag() == other.opaque_tag()
    }

    /// Performs the weak entity-tag comparison.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ETag;
    /// use http_headers::{FieldValue, SingleValueField};
    ///
    /// let strong = FieldValue::from_static(r#""xyzzy""#);
    /// let weak = FieldValue::from_static(r#"W/"xyzzy""#);
    /// let other = FieldValue::from_static(r#""plugh""#);
    /// let strong = <ETag as SingleValueField>::decode_view(strong.as_field_value_ref())?;
    /// let weak = <ETag as SingleValueField>::decode_view(weak.as_field_value_ref())?;
    /// let other = <ETag as SingleValueField>::decode_view(other.as_field_value_ref())?;
    /// assert!(strong.weak_eq(weak));
    /// assert!(!strong.weak_eq(other));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn weak_eq(self, other: Self) -> bool {
        self.opaque_tag() == other.opaque_tag()
    }
}

impl SingleValueField for ETag {
    type View<'a> = ETagView<'a>;
    type Owned = ETagOwned;

    fn name() -> &'static FieldName {
        &FieldName::Etag
    }

    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
        parse(value.as_bytes())?;
        Ok(ETagView { value })
    }

    #[inline]
    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
        ETagOwned::try_from(value)
    }

    fn decode_view_with(value: FieldValueRef<'_>, mode: DecodeMode) -> Result<Self::View<'_>, DecodeError> {
        parse_with(value.as_bytes(), mode)?;
        Ok(ETagView { value })
    }

    fn decode_owned_with(value: FieldValue, mode: DecodeMode) -> Result<Self::Owned, DecodeError> {
        parse_with(value.as_bytes(), mode)?;
        Ok(ETagOwned { value })
    }

    fn as_field_value(value: &Self::Owned) -> &FieldValue {
        &value.value
    }

    fn into_field_value(value: Self::Owned) -> FieldValue {
        value.value
    }
}

impl TryFrom<&str> for ETagOwned {
    type Error = DecodeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = FieldValue::from_str(value).map_err(|_invalid| super::invalid_syntax(&FieldName::Etag))?;
        Self::try_from(value)
    }
}

impl TryFrom<String> for ETagOwned {
    type Error = DecodeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let value = FieldValue::try_from(value).map_err(|_invalid| super::invalid_syntax(&FieldName::Etag))?;
        Self::try_from(value)
    }
}

impl TryFrom<FieldValue> for ETagOwned {
    type Error = DecodeError;

    #[inline]
    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        parse(value.as_bytes())?;
        Ok(Self { value })
    }
}

fn construct(opaque: &[u8], weak: bool) -> Result<ETagOwned, DecodeError> {
    if !opaque.iter().copied().all(valid_opaque_byte) {
        return Err(super::invalid_syntax(&FieldName::Etag));
    }
    let prefix: &[u8] = if weak { b"W/\"" } else { b"\"" };
    let total_len = etag_wire_len(prefix.len(), opaque.len())?;
    let mut wire = Vec::with_capacity(total_len);
    wire.extend_from_slice(prefix);
    wire.extend_from_slice(opaque);
    wire.push(b'"');
    Ok(ETagOwned {
        value: super::value_from_bytes(&FieldName::Etag, wire)?,
    })
}

#[inline]
fn etag_wire_len(prefix_len: usize, opaque_len: usize) -> Result<usize, DecodeError> {
    prefix_len
        .checked_add(opaque_len)
        .and_then(|length| length.checked_add(1))
        .ok_or_else(|| super::invalid_syntax(&FieldName::Etag))
}

#[inline]
fn parse(bytes: &[u8]) -> Result<(), DecodeError> {
    parse_with(bytes, DecodeMode::Strict)
}

#[inline]
fn parse_with(bytes: &[u8], mode: DecodeMode) -> Result<(), DecodeError> {
    let opaque = match bytes {
        [b'"', opaque @ .., b'"'] | [b'W', b'/', b'"', opaque @ .., b'"'] => opaque,
        [b'w', b'/', b'"', opaque @ .., b'"'] if mode == DecodeMode::Relaxed => opaque,
        _ => return Err(super::invalid_syntax(&FieldName::Etag)),
    };
    if opaque_is_valid(opaque) {
        Ok(())
    } else {
        Err(super::invalid_syntax(&FieldName::Etag))
    }
}

/// Broadcasts `1` into every byte lane of a word.
const LANE_ONES: u64 = 0x0101_0101_0101_0101;

/// Selects the high bit of every byte lane of a word.
const LANE_HIGH: u64 = 0x8080_8080_8080_8080;

/// Returns whether the eight packed bytes of `word` are all legal `etagc`.
///
/// A `FieldValue` byte other than DEL is legal when setting bit one lifts it
/// to `0x23` or above: HTAB becomes `0x0B`, SP and DQUOTE both become `0x22`,
/// `!` becomes `0x23`, and every other byte already exceeds `0x22`.
///
/// `ored | LANE_HIGH` lifts every lane above the threshold so the subtraction
/// never borrows across lane boundaries, and masking with the untouched high
/// bits keeps `obs-text` lanes out of the comparison. XOR maps DEL to zero;
/// the standard zero-byte test rejects it in any lane.
const fn word_is_valid(word: u64) -> bool {
    let ored = word | (0x02 * LANE_ONES);
    let lifted = ored | LANE_HIGH;
    let below_minimum = !(lifted.wrapping_sub(0x23 * LANE_ONES) | ored) & LANE_HIGH;
    let deltas = word ^ (0x7f * LANE_ONES);
    let contains_del = deltas.wrapping_sub(LANE_ONES) & !deltas & LANE_HIGH;
    below_minimum == 0 && contains_del == 0
}

/// Returns whether every byte of `opaque` is a legal `etagc`.
///
/// Words of eight bytes are classified at once; the final word overlaps the
/// previous one when the length is not a multiple of eight, which re-checks a
/// few bytes rather than paying per-byte for the tail.
#[inline]
pub(super) fn opaque_is_valid(opaque: &[u8]) -> bool {
    let length = opaque.len();
    if length < 8 {
        return opaque.iter().copied().all(valid_opaque_byte);
    }
    let mut index = 0;
    while index + 8 < length {
        if !word_is_valid(read_word(opaque, index)) {
            return false;
        }
        index += 8;
    }
    word_is_valid(read_word(opaque, length - 8))
}

/// Reads the eight bytes of `bytes` starting at `index` as a packed word.
fn read_word(bytes: &[u8], index: usize) -> u64 {
    let mut word = [0; 8];
    word.copy_from_slice(&bytes[index..index + 8]);
    u64::from_ne_bytes(word)
}

const fn valid_opaque_byte(byte: u8) -> bool {
    matches!(byte, b'!' | b'#'..=b'~' | 0x80..=0xff)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![expect(
        clippy::assertions_on_result_states,
        reason = "tests classify parser outcomes without needing successful values"
    )]

    use super::{ETag, ETagOwned, etag_wire_len, opaque_is_valid, parse_with, read_word, valid_opaque_byte, word_is_valid};
    use crate::sink::FieldSink;
    use crate::{DecodeMode, FieldName, FieldValue, SingleValueField, TestSink};

    #[test]
    fn owned_and_borrowed_tags_cover_accessors_and_comparisons() {
        let strong = ETagOwned::strong("revision").expect("valid strong tag");
        let strong_same = ETagOwned::try_from_wire("\"revision\"").expect("valid wire tag");
        let weak = ETagOwned::weak("revision").expect("valid weak tag");
        assert!(!strong.is_weak());
        assert!(weak.is_weak());
        assert_eq!(strong.opaque_tag(), Ok(b"revision".as_slice()));
        assert_eq!(strong.strong_eq(&strong_same), Ok(true));
        assert_eq!(strong.strong_eq(&weak), Ok(false));
        assert_eq!(strong.weak_eq(&weak), Ok(true));
        assert_eq!(
            String::from("\"owned\"")
                .parse::<ETagOwned>()
                .expect("owned string parses")
                .opaque_tag(),
            Ok(b"owned".as_slice())
        );
        assert_eq!(
            "\"parsed\"".parse::<ETagOwned>().expect("shared FromStr parses").opaque_tag(),
            Ok(b"parsed".as_slice())
        );

        let mut table = TestSink::new();
        ETag::insert(&mut table, strong.clone()).expect("table accepts etag");
        let view = ETag::view(&table).expect("valid tag").expect("present");
        let same = <ETag as SingleValueField>::decode_view(view.field_value()).expect("valid borrowed tag");
        let weak_view = <ETag as SingleValueField>::decode_view(weak.value.as_field_value_ref()).expect("valid weak borrowed tag");
        assert!(!view.is_weak());
        assert_eq!(view.opaque_tag(), b"revision");
        assert!(view.strong_eq(same));
        assert!(!view.strong_eq(weak_view));
        assert!(view.weak_eq(weak_view));
        assert_eq!(
            ETag::owned(&table).expect("valid owned tag").expect("present").opaque_tag(),
            Ok(b"revision".as_slice())
        );
        table.remove_values(&FieldName::Etag);
        assert!(ETag::view(&table).expect("absence is valid").is_none());
        assert_eq!(strong.into_field_value(), FieldValue::from_static("\"revision\""));
    }

    #[test]
    fn parsing_covers_relaxed_and_invalid_opaque_bytes() {
        assert!(parse_with(b"w/\"tag\"", DecodeMode::Strict).is_err());
        assert!(parse_with(b"w/\"tag\"", DecodeMode::Relaxed).is_ok());
        for wire in [
            b"tag".as_slice(),
            b"W/tag",
            b"\"space inside\"",
            b"\"tab\tinside\"",
            b"\"del\x7finside\"",
            b"\"unterminated",
        ] {
            assert!(parse_with(wire, DecodeMode::Relaxed).is_err(), "{wire:?}");
        }
        assert!(ETagOwned::strong("bad\"tag").is_err());
        assert!(ETagOwned::weak("bad tag").is_err());

        let relaxed = FieldValue::from_static("w/\"tag\"");
        assert!(<ETag as SingleValueField>::decode_view_with(relaxed.as_field_value_ref(), DecodeMode::Relaxed).is_ok());
        assert!(<ETag as SingleValueField>::decode_owned_with(relaxed, DecodeMode::Relaxed).is_ok());

        let strict = FieldValue::from_static("\"tag\"");
        let strict_view = <ETag as SingleValueField>::decode_view(strict.as_field_value_ref()).expect("valid strict borrowed tag");
        assert_eq!(strict_view.opaque_tag(), b"tag");
        let strict_owned = <ETag as SingleValueField>::decode_owned(strict.clone()).expect("valid strict owned tag");
        assert_eq!(<ETag as SingleValueField>::as_field_value(&strict_owned), &strict);
        assert_eq!(
            ETagOwned::try_from(String::from("\"owned\""))
                .expect("valid owned string")
                .opaque_tag(),
            Ok(b"owned".as_slice())
        );
        assert!(ETagOwned::try_from("\n").is_err());
        assert!(ETagOwned::try_from(String::from("\n")).is_err());
        assert!(ETagOwned::try_from(FieldValue::from_static("invalid")).is_err());

        let malformed = ETagOwned {
            value: FieldValue::from_static(""),
        };
        assert!(malformed.opaque_tag().is_err());
        assert!(malformed.strong_eq(&strict_owned).is_err());
        assert!(malformed.weak_eq(&strict_owned).is_err());
        let malformed_range = ETagOwned {
            value: FieldValue::from_static("\""),
        };
        assert!(malformed_range.opaque_tag().is_err());
        assert_eq!(etag_wire_len(1, 2), Ok(4));
        assert!(etag_wire_len(usize::MAX, 0).is_err());
        assert!(etag_wire_len(usize::MAX - 1, 1).is_err());
    }

    #[test]
    fn opaque_validator_covers_scalar_words_tails_and_rejections() {
        assert!(opaque_is_valid(b""));
        assert!(opaque_is_valid(b"short"));
        assert!(opaque_is_valid(b"12345678"));
        assert!(opaque_is_valid(b"123456789"));
        assert!(opaque_is_valid(b"12345678901234567"));
        assert!(!opaque_is_valid(b"1234567 901234567"));
        assert!(!opaque_is_valid(b"1234567890123456 "));
        assert!(!opaque_is_valid(b"short\x7f"));
        assert!(!opaque_is_valid(b"1234\x7f678"));
        assert!(word_is_valid(read_word(b"12345678", 0)));
        assert!(!word_is_valid(read_word(b"1234 678", 0)));
        assert!(!word_is_valid(read_word(b"1234\x7f678", 0)));
        assert!(valid_opaque_byte(b'!'));
        assert!(valid_opaque_byte(0x80));
        assert!(!valid_opaque_byte(b' '));

        for byte in u8::MIN..=u8::MAX {
            assert_eq!(opaque_is_valid(&[byte]), valid_opaque_byte(byte), "scalar byte {byte:#04x}");
            for lane in 0..8 {
                let mut bytes = [b'a'; 8];
                bytes[lane] = byte;
                assert_eq!(
                    word_is_valid(u64::from_ne_bytes(bytes)),
                    valid_opaque_byte(byte),
                    "packed byte {byte:#04x} in lane {lane}"
                );
            }
        }
    }
}
