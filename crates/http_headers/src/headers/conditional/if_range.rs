// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::time::SystemTime;

use super::shared::{ConditionalTagView, format_http_date, parse_http_date_with};
use crate::{DecodeError, FieldName, FieldValue, FieldValueRef, SingleValueField};

/// The parsed alternative represented by `If-Range`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::IfRangeOwned::try_from("\"revision\"")?;
/// assert!(matches!(
///     value.value()?,
///     http_headers::headers::IfRangeValueView::EntityTag(_)
/// ));
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub enum IfRangeValueView<'a> {
    /// A strong entity tag.
    EntityTag(ConditionalTagView<'a>),
    /// An HTTP date.
    Date(SystemTime),
}

/// Defines the `If-Range` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 13.1.5](https://www.rfc-editor.org/rfc/rfc9110#section-13.1.5).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{IfRange, IfRangeOwned};
///
/// let mut map = HeaderMap::new();
/// IfRange::insert(&mut map, IfRangeOwned::try_from("\"revision\"")?)?;
/// assert!(IfRange::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct IfRange {
    _private: (),
}

/// Owned value for the `If-Range` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 13.1.5].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::IfRangeOwned::try_from("\"revision\"")?;
/// assert!(matches!(
///     value.value()?,
///     http_headers::headers::IfRangeValueView::EntityTag(_)
/// ));
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `If-Range: "revision-42"` carries a strong entity tag, while
/// `If-Range: Wed, 21 Oct 2015 07:28:00 GMT` carries an HTTP date.
///
/// [RFC 9110 section 13.1.5]: https://www.rfc-editor.org/rfc/rfc9110#section-13.1.5
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct IfRangeOwned {
    value: FieldValue,
    parsed: IfRangeMetadata,
}

/// Borrowed value for the `If-Range` header.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{IfRange, IfRangeValueView, IfRangeView};
/// use http_headers::{FieldValueRef, SingleValueField};
///
/// let value: IfRangeView<'_> = IfRange::decode_view(FieldValueRef::new(b"\"revision\""))?;
/// assert!(matches!(
///     value.value(),
///     IfRangeValueView::EntityTag(tag) if tag.opaque_tag() == b"revision"
/// ));
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct IfRangeView<'a> {
    value: FieldValueRef<'a>,
    parsed: IfRangeValueView<'a>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum IfRangeMetadata {
    EntityTag,
    Date(SystemTime),
}

impl IfRangeMetadata {
    const fn from_view(value: IfRangeValueView<'_>) -> Self {
        match value {
            IfRangeValueView::EntityTag(_) => Self::EntityTag,
            IfRangeValueView::Date(date) => Self::Date(date),
        }
    }
}

impl fmt::Debug for IfRangeOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IfRangeOwned")
            .field("value", &self.parsed_value())
            .finish_non_exhaustive()
    }
}

impl IfRangeOwned {
    /// Constructs an `If-Range` value from a strong entity tag.
    ///
    /// # Errors
    ///
    /// Returns an error for a weak tag.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{ETagOwned, IfRangeOwned, IfRangeValueView};
    ///
    /// let tag = ETagOwned::try_from("\"revision\"")?;
    /// let value = IfRangeOwned::entity_tag(tag)?;
    /// assert!(matches!(
    ///     value.value()?,
    ///     IfRangeValueView::EntityTag(tag) if tag.opaque_tag() == b"revision"
    /// ));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn entity_tag(tag: crate::headers::ETagOwned) -> Result<Self, DecodeError> {
        if tag.is_weak() {
            return Err(crate::headers::invalid_syntax(&FieldName::IfRange));
        }
        Ok(Self {
            value: tag.into_field_value(),
            parsed: IfRangeMetadata::EntityTag,
        })
    }

    /// Constructs a canonical IMF-fixdate alternative.
    ///
    /// Fractional seconds are discarded to match the whole-second wire precision.
    ///
    /// # Errors
    ///
    /// Returns an error when the date cannot be represented by `httpdate`.
    /// # Examples
    ///
    /// ```rust
    /// use std::time::{Duration, UNIX_EPOCH};
    ///
    /// use http_headers::headers::{IfRangeOwned, IfRangeValueView};
    ///
    /// let instant = UNIX_EPOCH + Duration::from_secs(784_111_777);
    /// let value = IfRangeOwned::date(instant)?;
    /// assert!(matches!(value.value()?, IfRangeValueView::Date(date) if date == instant));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn date(date: SystemTime) -> Result<Self, DecodeError> {
        format_http_date(&FieldName::IfRange, date).and_then(Self::try_from)
    }

    /// Returns the parsed alternative.
    ///
    /// # Errors
    ///
    /// Returns an error if internal preserved offsets no longer agree with
    /// the field value.
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::IfRangeOwned::try_from("\"revision\"")?;
    /// assert!(matches!(
    ///     value.value()?,
    ///     http_headers::headers::IfRangeValueView::EntityTag(_)
    /// ));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the fallible signature is retained for compatibility with the pre-1.0 accessor"
    )]
    pub fn value(&self) -> Result<IfRangeValueView<'_>, DecodeError> {
        Ok(self.parsed_value())
    }

    fn parsed_value(&self) -> IfRangeValueView<'_> {
        match self.parsed {
            IfRangeMetadata::EntityTag => IfRangeValueView::EntityTag(ConditionalTagView {
                wire: self.value.as_bytes(),
            }),
            IfRangeMetadata::Date(date) => IfRangeValueView::Date(date),
        }
    }

    /// Returns the preserved field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::IfRangeOwned;
    ///
    /// let value = IfRangeOwned::try_from("\"revision\"")?;
    /// assert_eq!(value.as_field_value().as_bytes(), b"\"revision\"");
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
    /// use http_headers::headers::IfRangeOwned;
    ///
    /// let value = IfRangeOwned::try_from("Sat, 29 Oct 1994 19:43:31 GMT")?;
    /// let field_value = value.into_field_value();
    /// assert_eq!(field_value.as_bytes(), b"Sat, 29 Oct 1994 19:43:31 GMT");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }
}

super::super::shared::impl_field_value_conversion!(IfRangeOwned, |value| value.value);

impl<'a> IfRangeView<'a> {
    /// Returns the parsed alternative.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::IfRangeOwned::try_from("\"revision\"")?;
    /// assert!(matches!(
    ///     value.value()?,
    ///     http_headers::headers::IfRangeValueView::EntityTag(_)
    /// ));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn value(self) -> IfRangeValueView<'a> {
        self.parsed
    }

    /// Returns the borrowed field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::IfRange;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let value = IfRange::decode_view(FieldValueRef::new(b"\"revision\""))?;
    /// assert_eq!(value.as_field_value().as_bytes(), b"\"revision\"");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn as_field_value(self) -> FieldValueRef<'a> {
        self.value
    }
}

impl SingleValueField for IfRange {
    type View<'a> = IfRangeView<'a>;
    type Owned = IfRangeOwned;

    fn name() -> &'static FieldName {
        &FieldName::IfRange
    }

    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
        Ok(IfRangeView {
            value,
            parsed: parse_if_range(value)?,
        })
    }

    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
        IfRangeOwned::try_from(value)
    }

    fn decode_view_with(value: FieldValueRef<'_>, mode: crate::DecodeMode) -> Result<Self::View<'_>, DecodeError> {
        Ok(IfRangeView {
            value,
            parsed: parse_if_range_with(value, mode)?,
        })
    }

    fn decode_owned_with(value: FieldValue, mode: crate::DecodeMode) -> Result<Self::Owned, DecodeError> {
        let parsed = IfRangeMetadata::from_view(parse_if_range_with(value.as_field_value_ref(), mode)?);
        Ok(IfRangeOwned { value, parsed })
    }

    fn as_field_value(value: &Self::Owned) -> &FieldValue {
        &value.value
    }

    fn into_field_value(value: Self::Owned) -> FieldValue {
        value.value
    }
}

super::super::shared::impl_string_conversions!(IfRangeOwned, &FieldName::IfRange, crate::headers::invalid_syntax, wire);

impl TryFrom<FieldValue> for IfRangeOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        let parsed = IfRangeMetadata::from_view(parse_if_range(value.as_field_value_ref())?);
        Ok(Self { value, parsed })
    }
}

/// Returns whether every byte of `opaque` is a legal `etagc`.
///
/// Kept out of line so that the `If-Range` decoders stay small enough for the
/// caller to inline them, which measures better than inlining the scan itself.
#[inline(never)]
fn opaque_is_valid(opaque: &[u8]) -> bool {
    crate::headers::etag::opaque_is_valid(opaque)
}

fn parse_if_range(value: FieldValueRef<'_>) -> Result<IfRangeValueView<'_>, DecodeError> {
    parse_if_range_with(value, crate::DecodeMode::Strict)
}

fn parse_if_range_with(value: FieldValueRef<'_>, mode: crate::DecodeMode) -> Result<IfRangeValueView<'_>, DecodeError> {
    let wire = value.as_bytes();
    if let [b'"', opaque @ .., b'"'] = wire {
        return if opaque_is_valid(opaque) {
            Ok(IfRangeValueView::EntityTag(ConditionalTagView { wire }))
        } else {
            Err(crate::headers::invalid_syntax(&FieldName::IfRange))
        };
    }
    parse_if_range_date_with(value, mode)
}

/// Handles every `If-Range` alternative that is not a complete strong tag.
///
/// A weak validator is never a legal alternative, and it can never be an
/// HTTP-date either. Anything else, including a truncated quoted tag, is left
/// to the date parser, which rejects it with the same error.
fn parse_if_range_date_with(value: FieldValueRef<'_>, mode: crate::DecodeMode) -> Result<IfRangeValueView<'_>, DecodeError> {
    if value.as_bytes().starts_with(b"W/") {
        return Err(crate::headers::invalid_syntax(&FieldName::IfRange));
    }
    parse_http_date_with(&FieldName::IfRange, value, mode).map(IfRangeValueView::Date)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{Duration, UNIX_EPOCH};

    use super::{IfRange, IfRangeOwned, IfRangeValueView};
    use crate::headers::ETagOwned;
    use crate::{DecodeErrorKind, DecodeMode, FieldName, FieldValue, SingleValueField};

    fn hash(value: &impl Hash) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn date_construction_matches_wire_precision_and_round_trip_identity() {
        for seconds in [0, 1, 784_111_776, 784_111_777, 253_402_300_799] {
            let whole = UNIX_EPOCH + Duration::from_secs(seconds);
            let canonical = IfRangeOwned::date(whole).unwrap();
            for nanos in [0, 1, 500_000_000, 999_999_999] {
                let constructed = IfRangeOwned::date(whole + Duration::from_nanos(nanos)).unwrap();
                assert_eq!(constructed.value().unwrap(), IfRangeValueView::Date(whole));
                assert_eq!(constructed, canonical);
                assert_eq!(hash(&constructed), hash(&canonical));

                let wire = constructed.as_field_value();
                let parsed = IfRangeOwned::try_from(wire.try_as_str().unwrap()).unwrap();
                assert_eq!(constructed, parsed);
                assert_eq!(hash(&constructed), hash(&parsed));
                for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
                    let view = IfRange::decode_view_with(wire.as_field_value_ref(), mode).unwrap();
                    let owned = IfRange::decode_owned_with(wire.clone(), mode).unwrap();
                    assert_eq!(view.value(), constructed.value().unwrap());
                    assert_eq!(hash(&view.value()), hash(&constructed.value().unwrap()));
                    assert_eq!(owned, constructed);
                    assert_eq!(hash(&owned), hash(&constructed));
                }

                #[cfg(all(feature = "serde", feature = "headers-conditional"))]
                {
                    let serialized = serde_json::to_string(&constructed).unwrap();
                    let decoded: IfRangeOwned = serde_json::from_str(&serialized).unwrap();
                    assert_eq!(decoded, constructed);
                    assert_eq!(decoded.value().unwrap(), IfRangeValueView::Date(whole));
                    assert_eq!(hash(&decoded), hash(&constructed));
                }
            }
        }
    }

    #[test]
    fn date_constructor_retains_representable_bounds() {
        // Windows SystemTime uses 100 ns intervals.
        let before_epoch = UNIX_EPOCH - Duration::from_micros(1);
        assert!(before_epoch < UNIX_EPOCH);
        for instant in [
            before_epoch,
            UNIX_EPOCH - Duration::from_secs(1),
            UNIX_EPOCH + Duration::from_hours(70_389_528),
        ] {
            assert_eq!(IfRangeOwned::date(instant).unwrap_err().kind(), DecodeErrorKind::InvalidNumber);
        }
    }

    #[test]
    fn constructors_accessors_and_trait_paths_preserve_the_selected_alternative() {
        let tag = ETagOwned::try_from("\"revision\"").expect("strong tag");
        let tagged = IfRangeOwned::entity_tag(tag).expect("If-Range strong tag");
        let parsed = tagged.value().expect("tag value");
        let opaque = |value| match value {
            IfRangeValueView::EntityTag(view) => Some(view.opaque_tag()),
            IfRangeValueView::Date(_) => None,
        };
        assert_eq!(opaque(parsed), Some(b"revision".as_slice()));
        assert_eq!(tagged.as_field_value().as_bytes(), b"\"revision\"");
        assert!(format!("{tagged:?}").contains("EntityTag"));
        assert_eq!(
            IfRangeOwned::try_from(String::from("\"owned\""))
                .expect("owned tag string")
                .as_field_value(),
            "\"owned\""
        );

        let instant = UNIX_EPOCH + Duration::from_secs(784_111_777);
        let dated = IfRangeOwned::date(instant).expect("representable date");
        assert_eq!(dated.value().expect("date value"), IfRangeValueView::Date(instant));
        assert_eq!(opaque(dated.value().expect("date value")), None);

        let view = <IfRange as SingleValueField>::decode_view(tagged.as_field_value().as_field_value_ref()).expect("tag view");
        assert_eq!(view.as_field_value(), tagged.as_field_value().as_field_value_ref());
        assert!(matches!(view.value(), IfRangeValueView::EntityTag(_)));
        assert_eq!(<IfRange as SingleValueField>::name(), &FieldName::IfRange);

        let owned = <IfRange as SingleValueField>::decode_owned(tagged.clone().into_field_value()).expect("owned tag");
        assert_eq!(<IfRange as SingleValueField>::as_field_value(&owned), tagged.as_field_value());
        assert_eq!(<IfRange as SingleValueField>::into_field_value(owned), tagged.into_field_value());
    }

    #[test]
    fn weak_malformed_and_relaxed_date_inputs_are_distinguished() {
        let weak = ETagOwned::try_from("W/\"weak\"").expect("weak tag");
        let error = IfRangeOwned::entity_tag(weak).expect_err("weak If-Range tag");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);

        for wire in ["W/\"weak\"", "\"bad tag\"", "\"unterminated", "not a date"] {
            let error = IfRangeOwned::try_from(wire).expect_err("invalid If-Range");
            assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax, "{wire:?}");
        }
        let error = IfRangeOwned::try_from(String::from("bad\nvalue")).expect_err("invalid field value");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
        assert_eq!(
            IfRangeOwned::try_from("bad\nvalue")
                .expect_err("invalid borrowed field value")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let relaxed = FieldValue::from_static(" Tue, 8 Nov 1994 8:49:37 UTC ");
        <IfRange as SingleValueField>::decode_view(relaxed.as_field_value_ref()).expect_err("strict date");
        let view =
            <IfRange as SingleValueField>::decode_view_with(relaxed.as_field_value_ref(), DecodeMode::Relaxed).expect("relaxed date");
        assert!(matches!(view.value(), IfRangeValueView::Date(_)));
        let owned = <IfRange as SingleValueField>::decode_owned_with(relaxed, DecodeMode::Relaxed).expect("relaxed owned date");
        assert_eq!(owned.as_field_value().as_bytes(), b" Tue, 8 Nov 1994 8:49:37 UTC ");
        assert!(matches!(owned.value(), Ok(IfRangeValueView::Date(_))));
    }
}
