// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::{fmt, str};

use super::super::shared::FieldLinesIter;
use super::super::{invalid_syntax, normalized_comma_value, trim_ows};
use super::shared::validate_range_unit_for;
use crate::sink::{EncodedValues, FieldSink, InsertError};
use crate::source::{FieldLines, FieldSource};
use crate::{DecodeError, DecodeErrorKind, Field, FieldName, FieldValue, FieldValueRef, validate};

/// Defines the `Accept-Ranges` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 14.3](https://www.rfc-editor.org/rfc/rfc9110#section-14.3).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{AcceptRanges, AcceptRangesOwned};
///
/// let mut map = HeaderMap::new();
/// AcceptRanges::insert(&mut map, AcceptRangesOwned::from_units(["bytes"])?)?;
/// assert!(AcceptRanges::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct AcceptRanges {
    _private: (),
}

/// Owned value for the `Accept-Ranges` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 14.3].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::AcceptRangesOwned::from_units(["bytes"])?;
/// assert_eq!(value.units().collect::<Vec<_>>(), ["bytes"]);
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Accept-Ranges: bytes` advertises byte ranges, `Accept-Ranges: none`
/// advertises no supported unit, and `Accept-Ranges: custom-unit` preserves an
/// extension range unit.
///
/// [RFC 9110 section 14.3]: https://www.rfc-editor.org/rfc/rfc9110#section-14.3
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct AcceptRangesOwned {
    values: Option<FieldLinesIter>,
    none: bool,
}

/// Borrowed value for the `Accept-Ranges` header.
/// # Examples
///
/// ```rust
/// use http_headers::headers::{AcceptRanges, AcceptRangesView};
/// use http_headers::source::{FieldLines, FieldSource};
/// use http_headers::{Field, FieldName};
///
/// struct Source;
/// impl FieldSource for Source {
///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
///         (name == &FieldName::AcceptRanges)
///             .then_some(FieldLines::single(&FieldName::AcceptRanges, b"bytes"))
///     }
/// }
///
/// let source = Source;
/// let view: AcceptRangesView<'_> = AcceptRanges::view(&source)?.expect("present");
/// assert_eq!(view.units().collect::<Vec<_>>(), ["bytes"]);
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct AcceptRangesView<'a> {
    values: Option<FieldLines<'a>>,
    none: bool,
}

impl fmt::Debug for AcceptRangesOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcceptRangesOwned")
            .field("value_count", &self.values.as_ref().map_or(1, FieldLinesIter::len))
            .field("none", &self.none)
            .finish()
    }
}

impl fmt::Display for AcceptRangesOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut units = self.units();
        let unit = units.next().expect("constructors guarantee at least one range unit");
        f.write_str(unit)?;
        for unit in units {
            f.write_str(", ")?;
            f.write_str(unit)?;
        }
        Ok(())
    }
}

impl fmt::Debug for AcceptRangesView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcceptRangesView")
            .field("value_count", &self.values.as_ref().map_or(1, FieldLines::len))
            .field("none", &self.none)
            .finish()
    }
}

impl AcceptRangesOwned {
    #[cfg(all(feature = "serde", feature = "headers-range"))]
    pub(crate) fn field_values(&self) -> impl Iterator<Item = FieldValueRef<'_>> + '_ {
        let canonical = self
            .values
            .is_none()
            .then_some(FieldValueRef::new(if self.none { &b"none"[..] } else { &b"bytes"[..] }));
        canonical.into_iter().chain(
            self.values
                .iter()
                .flat_map(FieldLinesIter::iter)
                .map(FieldValue::as_field_value_ref),
        )
    }

    pub(crate) fn encoded_value(&self) -> Option<FieldValue> {
        normalized_comma_value(&FieldName::AcceptRanges, self.units().map(str::as_bytes))
            .expect("validated range units remain valid when comma-joined")
    }

    /// Constructs `Accept-Ranges: bytes`.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::AcceptRangesOwned;
    ///
    /// let value = AcceptRangesOwned::bytes();
    /// assert!(!value.is_none());
    /// assert_eq!(value.units().collect::<Vec<_>>(), ["bytes"]);
    /// ```
    pub fn bytes() -> Self {
        Self { values: None, none: false }
    }

    /// Constructs `Accept-Ranges: none`.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::AcceptRangesOwned;
    ///
    /// let value = AcceptRangesOwned::none();
    /// assert!(value.is_none());
    /// assert_eq!(value.units().collect::<Vec<_>>(), ["none"]);
    ///
    /// let bytes = AcceptRangesOwned::bytes();
    /// assert!(!bytes.is_none());
    /// ```
    pub fn none() -> Self {
        Self { values: None, none: true }
    }

    /// Constructs a canonical comma-separated unit list.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty list, invalid token, or `none` combined
    /// with another unit.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::AcceptRangesOwned;
    ///
    /// let value = AcceptRangesOwned::from_units(["bytes", "items"])?;
    /// assert_eq!(value.units().collect::<Vec<_>>(), ["bytes", "items"]);
    /// assert!(AcceptRangesOwned::from_units(["none", "bytes"]).is_err());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn from_units<I, S>(units: I) -> Result<Self, DecodeError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let units = units.into_iter();
        let mut wire = String::with_capacity(units.size_hint().0.saturating_mul(8));
        let mut count = 0_usize;
        let mut has_none = false;
        for unit in units {
            let unit = unit.as_ref();
            if !validate::token(unit.as_bytes()) {
                return Err(DecodeError::new(&FieldName::AcceptRanges, DecodeErrorKind::InvalidToken));
            }
            if !wire.is_empty() {
                wire.push_str(", ");
            }
            wire.push_str(unit);
            count += 1;
            has_none |= unit.eq_ignore_ascii_case("none");
        }
        if wire.is_empty() {
            return Err(DecodeError::new(&FieldName::AcceptRanges, DecodeErrorKind::MissingValue));
        }
        if has_none && count != 1 {
            return Err(invalid_syntax(&FieldName::AcceptRanges));
        }
        if wire == "bytes" {
            return Ok(Self::bytes());
        }
        if wire == "none" {
            return Ok(Self::none());
        }
        let value = validated_units_value(wire);
        Ok(Self {
            values: Some(FieldLinesIter::one(value)),
            none: has_none,
        })
    }

    /// Returns whether the field exclusively contains `none`.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::AcceptRangesOwned;
    ///
    /// let none = AcceptRangesOwned::none();
    /// assert!(none.is_none());
    ///
    /// let bytes = AcceptRangesOwned::bytes();
    /// assert!(!bytes.is_none());
    /// ```
    pub const fn is_none(&self) -> bool {
        self.none
    }

    /// Iterates advertised range units in wire order.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::AcceptRangesOwned;
    ///
    /// let value = AcceptRangesOwned::from_units(["bytes", "items"])?;
    /// let units = value.units().collect::<Vec<_>>();
    /// assert_eq!(units, ["bytes", "items"]);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn units(&self) -> impl Iterator<Item = &str> {
        let canonical = self.values.is_none().then_some(if self.none { "none" } else { "bytes" });
        canonical
            .into_iter()
            .chain(self.values.iter().flat_map(FieldLinesIter::iter).flat_map(|value| {
                value
                    .as_bytes()
                    .split(|byte| *byte == b',')
                    .map(trim_ows)
                    .filter_map(|unit| str::from_utf8(unit).ok())
                    .filter(|unit| !unit.is_empty())
            }))
    }
}

impl<'a> AcceptRangesView<'a> {
    pub(crate) fn field_values(&self) -> impl Iterator<Item = FieldValueRef<'a>> + '_ {
        let canonical = self
            .values
            .is_none()
            .then_some(FieldValueRef::new(if self.none { &b"none"[..] } else { &b"bytes"[..] }));
        canonical.into_iter().chain(self.values.iter().flat_map(FieldLines::repeated))
    }

    /// Returns whether the field exclusively contains `none`.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::AcceptRanges;
    /// use http_headers::source::{FieldLines, FieldSource};
    /// use http_headers::{Field, FieldName};
    ///
    /// struct Source(&'static [u8]);
    /// impl FieldSource for Source {
    ///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
    ///         (name == &FieldName::AcceptRanges)
    ///             .then_some(FieldLines::single(&FieldName::AcceptRanges, self.0))
    ///     }
    /// }
    ///
    /// let none_source = Source(b"none");
    /// let none = AcceptRanges::view(&none_source)?.expect("present");
    /// assert!(none.is_none());
    ///
    /// let bytes_source = Source(b"bytes");
    /// let bytes = AcceptRanges::view(&bytes_source)?.expect("present");
    /// assert!(!bytes.is_none());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn is_none(&self) -> bool {
        self.none
    }

    /// Iterates advertised range units in wire order.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::AcceptRanges;
    /// use http_headers::source::{FieldLines, FieldSource};
    /// use http_headers::{Field, FieldName};
    ///
    /// struct Source(&'static [u8]);
    /// impl FieldSource for Source {
    ///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
    ///         (name == &FieldName::AcceptRanges)
    ///             .then_some(FieldLines::single(&FieldName::AcceptRanges, self.0))
    ///     }
    /// }
    ///
    /// let source = Source(b"bytes, items");
    /// let view = AcceptRanges::view(&source)?.expect("present");
    /// let units = view.units().collect::<Vec<_>>();
    /// assert_eq!(units, ["bytes", "items"]);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn units(&self) -> impl Iterator<Item = &'a str> + '_ {
        let canonical = self.values.is_none().then_some(if self.none { "none" } else { "bytes" });
        canonical.into_iter().chain(
            self.values
                .iter()
                .flat_map(FieldLines::comma_items)
                .filter_map(|item| item.ok().and_then(|item| str::from_utf8(item).ok())),
        )
    }
}

impl Field for AcceptRanges {
    type View<'a> = AcceptRangesView<'a>;
    type Owned = AcceptRangesOwned;

    fn name() -> &'static FieldName {
        &FieldName::AcceptRanges
    }

    fn view_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        accept_ranges_view(source.lines(Self::name()))
    }

    /// Validates and clones in a single pass over the field values.
    fn owned_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        accept_ranges_owned(source.lines(Self::name()))
    }

    fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        let encoded = value.encoded_value().map_or_else(EncodedValues::new, EncodedValues::single);
        sink.set_values(Self::name(), encoded)
    }
}

fn accept_ranges_view(values: Option<FieldLines<'_>>) -> Result<Option<AcceptRangesView<'_>>, DecodeError> {
    let Some(values) = values else {
        return Ok(None);
    };
    values.validate_list_item_limit(b',', true)?;
    if let Some(none) = lone_canonical_unit(&values) {
        return Ok(Some(AcceptRangesView { values: None, none }));
    }
    let none = validate_accept_ranges(&values)?;
    Ok(Some(AcceptRangesView {
        values: Some(values),
        none,
    }))
}

fn accept_ranges_owned(values: Option<FieldLines<'_>>) -> Result<Option<AcceptRangesOwned>, DecodeError> {
    let Some(values) = values else {
        return Ok(None);
    };
    values.validate_list_item_limit(b',', true)?;
    let mut repeated = values.repeated_owned()?;
    if let Some((first, _first_owned)) = repeated.next()
        && repeated.next().is_none()
        && let Some(none) = canonical_unit(first.as_bytes())
    {
        return Ok(Some(AcceptRangesOwned { values: None, none }));
    }

    let mut repeated = values.repeated_owned()?;
    if let Some((first, first_owned)) = repeated.next()
        && repeated.next().is_none()
        && let Some((units, none)) = scan_units(first.as_bytes())
    {
        validate_none_cardinality(units, none)?;
        return Ok(Some(AcceptRangesOwned {
            values: Some(FieldLinesIter::one(first_owned)),
            none,
        }));
    }

    let none = validate_accept_ranges(&values)?;
    let mut repeated = values.repeated_owned()?;
    let mut collected = repeated.next().map(|(_, first_owned)| FieldLinesIter::one(first_owned));
    for (_value, owned) in repeated {
        collected.as_mut().expect("a later value has a preceding first value").push(owned);
    }
    Ok(Some(AcceptRangesOwned { values: collected, none }))
}

impl TryFrom<&str> for AcceptRangesOwned {
    type Error = DecodeError;

    fn try_from(wire: &str) -> Result<Self, Self::Error> {
        let value = FieldValue::from_str(wire).map_err(|_invalid| invalid_syntax(&FieldName::AcceptRanges))?;
        Self::try_from(value)
    }
}

impl TryFrom<String> for AcceptRangesOwned {
    type Error = DecodeError;

    fn try_from(wire: String) -> Result<Self, Self::Error> {
        let value = FieldValue::try_from(wire).map_err(|_invalid| invalid_syntax(&FieldName::AcceptRanges))?;
        Self::try_from(value)
    }
}

impl TryFrom<FieldValue> for AcceptRangesOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        if let Some(none) = canonical_unit(value.as_bytes()) {
            return Ok(Self { values: None, none });
        }
        let none = if let Some((units, none)) = scan_units(value.as_bytes()) {
            validate_none_cardinality(units, none)?;
            none
        } else {
            validate_units_slow(value.as_bytes())?
        };
        Ok(Self {
            values: Some(FieldLinesIter::one(value)),
            none,
        })
    }
}

fn validated_units_value(wire: String) -> FieldValue {
    FieldValue::try_from(wire).expect("validated range units remain valid when comma-joined")
}

/// Reports whether the field carries exactly one value that is just `bytes`.
fn lone_canonical_unit(values: &FieldLines<'_>) -> Option<bool> {
    let mut repeated = values.repeated();
    let first = repeated.next().expect("untyped values always contain a field line");
    repeated.next().is_none().then(|| canonical_unit(first.as_bytes())).flatten()
}

fn canonical_unit(bytes: &[u8]) -> Option<bool> {
    match bytes {
        b"bytes" => Some(false),
        b"none" => Some(true),
        _ => None,
    }
}

fn validate_accept_ranges(values: &FieldLines<'_>) -> Result<bool, DecodeError> {
    let mut repeated = values.repeated();
    let first = repeated.next().expect("untyped values always contain a field line");
    if repeated.next().is_none()
        && let Some((units, none)) = scan_units(first.as_bytes())
    {
        validate_none_cardinality(units, none)?;
        return Ok(none);
    }
    validate_accept_ranges_repeated(values)
}

/// Validates a field spread over several lines.
fn validate_accept_ranges_repeated(values: &FieldLines<'_>) -> Result<bool, DecodeError> {
    let mut units = 0_usize;
    let mut none = false;
    for value in values.repeated() {
        let Some((scanned, has_none)) = scan_units(value.as_bytes()) else {
            return validate_accept_ranges_slow(values);
        };
        units += scanned;
        none |= has_none;
    }
    validate_none_cardinality(units, none)?;
    Ok(none)
}

#[cold]
#[inline(never)]
fn validate_accept_ranges_slow(values: &FieldLines<'_>) -> Result<bool, DecodeError> {
    let mut units = 0_usize;
    let mut none = false;
    for item in values.comma_items() {
        let item = item?;
        validate_range_unit(item)?;
        units += 1;
        if validate::eq_ignore_ascii_case(item, b"none") {
            none = true;
        }
    }
    validate_none_cardinality(units, none)?;
    Ok(none)
}

#[cold]
#[inline(never)]
pub(super) fn validate_units_slow(bytes: &[u8]) -> Result<bool, DecodeError> {
    let mut units = 0_usize;
    let mut none = false;
    for item in bytes.split(|byte| *byte == b',') {
        let item = trim_ows(item);
        if item.is_empty() {
            continue;
        }
        validate_range_unit(item)?;
        units += 1;
        if validate::eq_ignore_ascii_case(item, b"none") {
            none = true;
        }
    }
    validate_none_cardinality(units, none)?;
    Ok(none)
}

/// Counts the units of a well-formed token list in one pass.
///
/// Returns `None` for anything unusual so the general implementation can
/// decide between acceptance and the precise error it reports.
pub(super) fn scan_units(bytes: &[u8]) -> Option<(usize, bool)> {
    if bytes == b"bytes" {
        return Some((1, false));
    }
    if bytes == b"none" {
        return Some((1, true));
    }
    let mut rest = bytes;
    let mut units = 0_usize;
    let mut none = false;
    loop {
        while let [b' ' | b'\t', tail @ ..] = rest {
            rest = tail;
        }
        let end = rest.iter().position(|byte| !TOKEN_BYTE[*byte as usize]).unwrap_or(rest.len());
        let (item, tail) = rest.split_at(end);
        rest = tail;
        while let [b' ' | b'\t', tail @ ..] = rest {
            rest = tail;
        }
        if !item.is_empty() {
            units += 1;
            none |= is_none_unit(item);
        }
        match rest {
            [] => return Some((units, none)),
            [b',', tail @ ..] => rest = tail,
            _ => return None,
        }
    }
}

fn is_none_unit(item: &[u8]) -> bool {
    let Ok(unit) = <[u8; 4]>::try_from(item) else {
        return false;
    };
    [unit[0] | 0x20, unit[1] | 0x20, unit[2] | 0x20, unit[3] | 0x20] == *b"none"
}

/// Maps every byte to whether it is permitted in a token.
static TOKEN_BYTE: [bool; 256] = {
    let mut table = [false; 256];
    let mut index = 0_u8;
    loop {
        table[index as usize] = validate::token_byte(index);
        if index == u8::MAX {
            break;
        }
        index += 1;
    }
    table
};

pub(super) fn validate_none_cardinality(units: usize, none: bool) -> Result<(), DecodeError> {
    if units == 0 {
        Err(DecodeError::new(&FieldName::AcceptRanges, DecodeErrorKind::MissingValue))
    } else if none && units != 1 {
        Err(invalid_syntax(&FieldName::AcceptRanges))
    } else {
        Ok(())
    }
}

fn validate_range_unit(bytes: &[u8]) -> Result<(), DecodeError> {
    validate_range_unit_for(bytes, &FieldName::AcceptRanges)
}

#[cfg(test)]
#[expect(
    clippy::assertions_on_result_states,
    reason = "the tests classify many parser outcomes without needing their success values"
)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{
        AcceptRanges, AcceptRangesOwned, canonical_unit, is_none_unit, lone_canonical_unit, scan_units, validate_accept_ranges,
        validate_accept_ranges_slow, validate_none_cardinality, validate_units_slow,
    };
    use crate::sink::{EncodedValues, FieldSink, InsertError};
    use crate::source::{FieldLines, FieldSource};
    use crate::{DecodeErrorKind, FieldName, FieldValue};

    #[derive(Default)]
    struct Store {
        values: Vec<FieldValue>,
    }

    impl Store {
        fn new(values: &[&'static str]) -> Self {
            Self {
                values: values.iter().copied().map(FieldValue::from_static).collect(),
            }
        }
    }

    impl FieldSource for Store {
        fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
            (name == &FieldName::AcceptRanges)
                .then(|| FieldLines::from_slice(name, &self.values))
                .flatten()
        }
    }

    impl FieldSink for Store {
        fn set_values(&mut self, _name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
            self.values = values.into_iter().collect();
            Ok(())
        }

        fn append_values(&mut self, _name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
            self.values.extend(values);
            Ok(())
        }

        fn remove_values(&mut self, _name: &'static FieldName) {
            self.values.clear();
        }
    }

    #[test]
    fn constructors_cover_canonical_extension_and_rejection_paths() {
        let bytes = AcceptRangesOwned::bytes();
        assert!(!bytes.is_none());
        assert_eq!(bytes.units().collect::<Vec<_>>(), ["bytes"]);
        assert!(format!("{bytes:?}").contains("none: false"));

        let none = AcceptRangesOwned::none();
        assert!(none.is_none());
        assert_eq!(none.units().collect::<Vec<_>>(), ["none"]);
        assert_eq!(
            AcceptRangesOwned::from_units(vec!["bytes"])
                .expect("canonical bytes")
                .units()
                .collect::<Vec<_>>(),
            ["bytes"]
        );
        assert!(AcceptRangesOwned::from_units(vec!["none"]).expect("canonical none").is_none());

        let extensions = AcceptRangesOwned::from_units(vec!["bytes", "items"]).expect("two valid range units");
        assert_eq!(extensions.units().collect::<Vec<_>>(), ["bytes", "items"]);
        assert_eq!(
            AcceptRangesOwned::from_units(Vec::<&str>::new()).expect_err("empty list").kind(),
            DecodeErrorKind::MissingValue
        );
        assert_eq!(
            AcceptRangesOwned::from_units(vec!["bad unit"]).expect_err("invalid token").kind(),
            DecodeErrorKind::InvalidToken
        );
        assert_eq!(
            AcceptRangesOwned::from_units(vec!["none", "bytes"])
                .expect_err("none must stand alone")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            AcceptRangesOwned::try_from(String::from("line\nbreak"))
                .expect_err("invalid field value")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            AcceptRangesOwned::try_from("line\nbreak")
                .expect_err("invalid borrowed field value")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            AcceptRangesOwned::try_from("Bytes, items")
                .expect("valid borrowed string")
                .units()
                .collect::<Vec<_>>(),
            ["Bytes", "items"]
        );
        assert_eq!(
            AcceptRangesOwned::try_from(String::from("Bytes, items"))
                .expect("valid owned string")
                .units()
                .collect::<Vec<_>>(),
            ["Bytes", "items"]
        );

        let canonical = AcceptRangesOwned::try_from(FieldValue::from_static("bytes")).expect("canonical");
        assert!(canonical.values.is_none());
        let scanned = AcceptRangesOwned::try_from(FieldValue::from_static("Bytes, items")).expect("fast scanned list");
        assert!(scanned.values.is_some());
        let fallback = AcceptRangesOwned::try_from(FieldValue::from_static("bytes ,\titems")).expect("fallback list");
        assert_eq!(fallback.units().collect::<Vec<_>>(), ["bytes", "items"]);
        assert_eq!(
            AcceptRangesOwned::try_from(FieldValue::from_static("bad unit"))
                .expect_err("slow validation reports invalid syntax")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
    }

    #[test]
    fn header_views_and_owned_values_cover_canonical_and_repeated_storage() {
        assert!(AcceptRanges::view(&Store::default()).expect("absent").is_none());
        assert!(AcceptRanges::owned(&Store::default()).expect("absent").is_none());

        for (wire, expected_none) in [("bytes", false), ("none", true)] {
            let store = Store::new(&[wire]);
            let view = AcceptRanges::view(&store).expect("valid canonical value").expect("present");
            assert_eq!(view.is_none(), expected_none);
            assert_eq!(view.units().collect::<Vec<_>>(), [wire]);
            let fields = view.field_values().collect::<Vec<_>>();
            assert_eq!(fields.len(), 1);
            assert_eq!(fields[0].as_bytes(), wire.as_bytes());
            assert!(format!("{view:?}").contains("value_count: 1"));

            let owned = AcceptRanges::owned(&store).expect("valid canonical value").expect("present");
            assert_eq!(owned.is_none(), expected_none);
        }

        let store = Store::new(&["bytes, items", "records"]);
        let view = AcceptRanges::view(&store).expect("valid repeated values").expect("present");
        assert_eq!(view.units().collect::<Vec<_>>(), ["bytes", "items", "records"]);
        assert_eq!(view.field_values().count(), 2);
        let owned = AcceptRanges::owned(&store).expect("valid repeated values").expect("present");
        assert_eq!(owned.units().collect::<Vec<_>>(), ["bytes", "items", "records"]);

        let single = Store::new(&["Bytes, items"]);
        let owned = AcceptRanges::owned(&single).expect("fast single-line validation").expect("present");
        assert_eq!(owned.units().collect::<Vec<_>>(), ["Bytes", "items"]);

        assert!(AcceptRanges::view(&Store::new(&["bad unit"])).is_err());
        assert!(AcceptRanges::owned(&Store::new(&["none, bytes"])).is_err());
        assert!(AcceptRanges::owned(&Store::new(&["bad unit"])).is_err());

        let mut sink = Store::default();
        AcceptRanges::insert(&mut sink, owned).expect("normalized insert");
        assert_eq!(sink.values.len(), 1);
        assert_eq!(sink.values[0].as_bytes(), b"Bytes, items");
        sink.remove_values(&FieldName::AcceptRanges);
        assert!(sink.values.is_empty());
    }

    #[test]
    fn scanners_and_fallback_validation_agree_on_edge_lists() {
        assert_eq!(canonical_unit(b"bytes"), Some(false));
        assert_eq!(canonical_unit(b"none"), Some(true));
        assert_eq!(canonical_unit(b"Bytes"), None);
        assert!(!is_none_unit(b"byte"));
        assert!(is_none_unit(b"NoNe"));

        for (wire, expected) in [
            (b"bytes".as_slice(), Some((1, false))),
            (b"none", Some((1, true))),
            (b" bytes , items ", Some((2, false))),
            (b"bytes,,items", Some((2, false))),
            (b"", Some((0, false))),
            (b"bad unit", None),
        ] {
            assert_eq!(scan_units(wire), expected, "{wire:?}");
        }
        assert_eq!(
            validate_units_slow(b", ,").expect_err("empty members do not supply a unit").kind(),
            DecodeErrorKind::MissingValue
        );
        assert!(!validate_units_slow(b"bytes, items").expect("valid list"));
        assert_eq!(
            validate_units_slow(b"none, bytes").expect_err("none cannot be combined").kind(),
            DecodeErrorKind::InvalidSyntax
        );

        assert_eq!(
            validate_none_cardinality(0, false).expect_err("missing units").kind(),
            DecodeErrorKind::MissingValue
        );
        assert_eq!(
            validate_none_cardinality(2, true).expect_err("none cardinality").kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert!(validate_none_cardinality(1, true).is_ok());

        let none = FieldLines::single(&FieldName::AcceptRanges, b"none");
        assert!(validate_accept_ranges_slow(&none).expect("direct slow-path validation"));
        assert!(AcceptRangesOwned::try_from(FieldValue::from_static("none, bytes")).is_err());
        let invalid_single = FieldLines::single(&FieldName::AcceptRanges, b"none, bytes");
        assert!(validate_accept_ranges(&invalid_single).is_err());
        let repeated = [FieldValue::from_static("none"), FieldValue::from_static("bytes")];
        let invalid_repeated = FieldLines::from_slice(&FieldName::AcceptRanges, &repeated).expect("two field lines");
        assert!(validate_accept_ranges(&invalid_repeated).is_err());
        assert!(validate_accept_ranges_slow(&invalid_repeated).is_err());

        let values = [FieldValue::from_static("bytes"), FieldValue::from_static("bad unit")];
        let repeated = FieldLines::from_slice(&FieldName::AcceptRanges, &values).expect("values");
        assert_eq!(
            validate_accept_ranges(&repeated).expect_err("invalid repeated unit").kind(),
            DecodeErrorKind::InvalidToken
        );

        let one = [FieldValue::from_static("bytes")];
        let values = FieldLines::from_slice(&FieldName::AcceptRanges, &one).expect("one value");
        assert_eq!(lone_canonical_unit(&values), Some(false));

        let invalid = [FieldValue::from_static("\"unterminated")];
        let values = FieldLines::from_slice(&FieldName::AcceptRanges, &invalid).expect("one value");
        assert_eq!(
            validate_accept_ranges(&values).expect_err("unterminated quoted member").kind(),
            DecodeErrorKind::UnterminatedQuote
        );
    }
}
