// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Write as _;
use std::{fmt, str};

use super::super::invalid_syntax;
use super::shared::{trim_ows, validate_list};
use crate::sink::{EncodedValues, FieldSink, InsertError};
use crate::source::{FieldLines, FieldSource};
use crate::{DecodeError, DecodeErrorKind, DecodeMode, Field, FieldName, FieldValue};

/// Defines the `Sec-WebSocket-Version` header.
///
/// # Specification
///
/// Defined by [RFC 6455 section 11.3.5](https://www.rfc-editor.org/rfc/rfc6455#section-11.3.5).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{SecWebSocketVersion, SecWebSocketVersionOwned};
///
/// let mut map = HeaderMap::new();
/// SecWebSocketVersion::insert(&mut map, SecWebSocketVersionOwned::new(13))?;
/// assert!(SecWebSocketVersion::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct SecWebSocketVersion {
    _private: (),
}

/// Set of WebSocket versions, one bit per version.
///
/// [RFC 6455 section 11.3.5] bounds a version to 0-255, so the entire domain
/// fits in 256 bits. Holding the set inline keeps every field-line shape,
/// including the version list a `426` response advertises, free of allocation
/// and makes reading a version a bit test rather than a reparse.
///
/// [RFC 6455 section 11.3.5]: https://www.rfc-editor.org/rfc/rfc6455#section-11.3.5
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct VersionSet {
    words: [u64; 4],
}

impl VersionSet {
    const EMPTY: Self = Self { words: [0; 4] };

    const fn one(version: u8) -> Self {
        let mut set = Self::EMPTY;
        set.insert(version);
        set
    }

    const fn insert(&mut self, version: u8) {
        self.words[(version >> 6) as usize] |= 1_u64 << (version & 63);
    }

    const fn contains(self, version: u8) -> bool {
        self.words[(version >> 6) as usize] & (1_u64 << (version & 63)) != 0
    }

    const fn is_empty(self) -> bool {
        self.words[0] | self.words[1] | self.words[2] | self.words[3] == 0
    }

    fn len(self) -> usize {
        self.words.iter().map(|word| word.count_ones() as usize).sum()
    }

    /// Yields the members in ascending order.
    fn iter(self) -> impl Iterator<Item = u8> {
        self.words
            .into_iter()
            .zip([0_u8, 64, 128, 192])
            .flat_map(|(word, base)| VersionBits { word, base })
    }
}

/// Yields the versions one bitmap word holds, lowest first.
struct VersionBits {
    word: u64,
    base: u8,
}

impl Iterator for VersionBits {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        (self.word != 0).then(|| {
            let bit = self.word.trailing_zeros();
            self.word &= self.word - 1;
            self.base + u8::try_from(bit).expect("a set bit index of a word fits in one byte")
        })
    }
}

/// Owned value for the `Sec-WebSocket-Version` header.
///
/// The field value carries nothing but version numbers, so the set of those
/// numbers is all this type keeps: surrounding whitespace, member order, and
/// repetition are accepted when decoding and dropped, and encoding renders one
/// ascending comma-separated list.
///
/// # Specification
///
/// Defined by [RFC 6455 section 11.3.5].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::SecWebSocketVersionOwned::new(13);
/// assert_eq!(value.versions().next(), Some(13));
/// ```
///
/// `Sec-WebSocket-Version: 13` requests the standard version.
/// A server can advertise alternatives with
/// `Sec-WebSocket-Version: 7, 8, 13`.
///
/// [RFC 6455 section 11.3.5]: https://www.rfc-editor.org/rfc/rfc6455#section-11.3.5
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct SecWebSocketVersionOwned {
    versions: VersionSet,
}

impl fmt::Debug for SecWebSocketVersionOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecWebSocketVersionOwned")
            .field("versions", &self.versions.len())
            .finish_non_exhaustive()
    }
}

impl fmt::Display for SecWebSocketVersionOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut versions = self.versions();
        if let Some(version) = versions.next() {
            version.fmt(f)?;
        }
        for version in versions {
            f.write_str(", ")?;
            version.fmt(f)?;
        }
        Ok(())
    }
}

impl SecWebSocketVersionOwned {
    /// Constructs one WebSocket version.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::SecWebSocketVersionOwned::new(13);
    /// assert_eq!(value.versions().next(), Some(13));
    /// ```
    #[must_use]
    pub const fn new(version: u8) -> Self {
        Self {
            versions: VersionSet::one(version),
        }
    }

    /// Adds another supported version.
    ///
    /// Adding a version the value already holds leaves it unchanged.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketVersionOwned;
    ///
    /// let value = SecWebSocketVersionOwned::new(13).with_version(8);
    /// assert_eq!(value.versions().collect::<Vec<_>>(), vec![8, 13]);
    /// ```
    #[must_use]
    pub const fn with_version(mut self, version: u8) -> Self {
        self.versions.insert(version);
        self
    }

    /// Reports whether the value holds a version.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketVersionOwned;
    ///
    /// let value = SecWebSocketVersionOwned::new(13);
    /// assert!(value.supports(13));
    /// assert!(!value.supports(8));
    /// ```
    #[must_use]
    pub const fn supports(&self, version: u8) -> bool {
        self.versions.contains(version)
    }

    /// Iterates the versions in ascending order.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::SecWebSocketVersionOwned::new(13);
    /// assert_eq!(value.versions().next(), Some(13));
    /// ```
    pub fn versions(&self) -> impl Iterator<Item = u8> + '_ {
        self.versions.iter()
    }

    /// Returns the single requested version.
    ///
    /// # Errors
    ///
    /// Returns an error when the value holds no version or more than one.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketVersionOwned;
    ///
    /// let value = SecWebSocketVersionOwned::new(13);
    /// assert_eq!(value.requested()?, 13);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn requested(&self) -> Result<u8, DecodeError> {
        let mut versions = self.versions();
        let version = versions
            .next()
            .ok_or_else(|| DecodeError::new(&FieldName::SecWebSocketVersion, DecodeErrorKind::MissingValue))?;
        if versions.next().is_some() {
            return Err(DecodeError::new(
                &FieldName::SecWebSocketVersion,
                DecodeErrorKind::UnexpectedMultipleValues,
            ));
        }
        Ok(version)
    }

    /// Renders the versions as the one field value that carries them all.
    pub(crate) fn field_value(&self) -> FieldValue {
        let mut versions = self.versions();
        let Some(first) = versions.next() else {
            return FieldValue::from_static("");
        };
        let Some(second) = versions.next() else {
            return FieldValue::from(u64::from(first));
        };
        let mut rendered = format!("{first}, {second}");
        for version in versions {
            write!(rendered, ", {version}").expect("writing to a string cannot fail");
        }
        FieldValue::try_from(rendered).expect("decimal versions are valid field-value bytes")
    }
}

impl Field for SecWebSocketVersion {
    type View<'a> = SecWebSocketVersionOwned;
    type Owned = SecWebSocketVersionOwned;

    fn name() -> &'static FieldName {
        &FieldName::SecWebSocketVersion
    }

    #[expect(
        clippy::inline_always,
        reason = "folding the one-line fast path into the caller removes a full stack frame"
    )]
    #[inline(always)]
    fn view_with<S>(source: &S, _mode: DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        decode_versions(source.lines(Self::name()))
    }

    #[expect(
        clippy::inline_always,
        reason = "folding the one-line fast path into the caller removes a full stack frame"
    )]
    #[inline(always)]
    fn owned_with<S>(source: &S, _mode: DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        decode_versions(source.lines(Self::name()))
    }

    fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        sink.set_values(Self::name(), EncodedValues::single(value.field_value()))
    }
}

/// Reads the version set a `Sec-WebSocket-Version` header holds.
#[expect(
    clippy::inline_always,
    reason = "keep the version 13 comparison and constant bitmap inside typed decode callers"
)]
#[inline(always)]
fn decode_versions(values: Option<FieldLines<'_>>) -> Result<Option<SecWebSocketVersionOwned>, DecodeError> {
    let Some(values) = values else {
        return Ok(None);
    };
    values.validate_list_item_limit(b',', true)?;
    let mut lines = values.repeated();
    let lone = match (lines.next(), lines.next()) {
        (Some(first), None) => Some(first.as_bytes()),
        _ => None,
    };
    let versions = if lone == Some(b"13".as_slice()) {
        VersionSet::one(13)
    } else {
        collect_version_lines(values, lone)?
    };
    Ok(Some(SecWebSocketVersionOwned { versions }))
}

super::super::shared::impl_string_conversions!(SecWebSocketVersionOwned, &FieldName::SecWebSocketVersion, invalid_syntax, value);

impl TryFrom<FieldValue> for SecWebSocketVersionOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        let values = FieldLines::single(&FieldName::SecWebSocketVersion, value.as_bytes());
        Ok(decode_versions(Some(values))?.expect("FieldLines::single always supplies one field value"))
    }
}

/// Decodes uncommon bare versions and advertised version lists.
#[expect(
    clippy::needless_pass_by_value,
    reason = "moving fallback storage keeps its stack materialization off the version 13 fast path"
)]
#[cold]
#[inline(never)]
fn collect_version_lines(values: FieldLines<'_>, lone: Option<&[u8]>) -> Result<VersionSet, DecodeError> {
    if let Some(version) = lone.and_then(parse_version_value) {
        return Ok(VersionSet::one(version));
    }
    let mut versions = VersionSet::EMPTY;
    for value in values.repeated() {
        if !collect_bare_version_line(value.as_bytes(), &mut versions)? {
            return collect_quoted_version_lines(&values);
        }
    }
    require_version(!versions.is_empty())?;
    Ok(versions)
}

/// Collects lines a quoted member forced the delimiter-aware walk to reread.
///
/// Quoting never produces a valid version — the grammar admits only digits —
/// but it does move where members start and end, so the shared walk decides
/// the split and reports an unterminated quote.
#[cold]
#[inline(never)]
fn collect_quoted_version_lines(values: &FieldLines<'_>) -> Result<VersionSet, DecodeError> {
    let mut versions = VersionSet::EMPTY;
    let mut collect = |bytes: &[u8]| -> Result<(), DecodeError> {
        versions.insert(parse_version(bytes)?);
        Ok(())
    };
    validate_list(values, &mut collect)?;
    Ok(versions)
}

/// Collects one comma-separated line of bare versions.
///
/// Reports `false` when the line quotes a member, which only the general
/// walk can split correctly.
#[inline]
fn collect_bare_version_line(bytes: &[u8], versions: &mut VersionSet) -> Result<bool, DecodeError> {
    if bytes.iter().any(|byte| matches!(byte, b'"' | b'\\')) {
        return Ok(false);
    }
    for item in bytes.split(|byte| *byte == b',').map(trim_ows) {
        if !item.is_empty() {
            versions.insert(parse_version(item)?);
        }
    }
    Ok(true)
}

#[inline]
fn require_version(present: bool) -> Result<(), DecodeError> {
    present
        .then_some(())
        .ok_or_else(|| DecodeError::new(&FieldName::SecWebSocketVersion, DecodeErrorKind::MissingValue))
}

fn parse_version(bytes: &[u8]) -> Result<u8, DecodeError> {
    parse_version_value(bytes).ok_or_else(|| invalid_syntax(&FieldName::SecWebSocketVersion))
}

fn parse_version_value(bytes: &[u8]) -> Option<u8> {
    // One and two digits cover every version the protocol has ever defined,
    // including the draft numbers a `426` response advertises, so both stay
    // inline; only the three-digit form the grammar still permits is remote
    // enough to pay for a call.
    match *bytes {
        [only] if only.is_ascii_digit() => Some(only - b'0'),
        [first, second] if is_leading_digit(first) && second.is_ascii_digit() => Some((first - b'0') * 10 + (second - b'0')),
        [_, _, _] => parse_uncommon_version_value(bytes),
        _ => None,
    }
}

/// Parses the three-digit versions the wire rarely carries.
#[cold]
#[inline(never)]
fn parse_uncommon_version_value(bytes: &[u8]) -> Option<u8> {
    let version = match *bytes {
        [first, second, third] if is_leading_digit(first) && second.is_ascii_digit() && third.is_ascii_digit() => {
            u16::from(first - b'0') * 100 + u16::from(second - b'0') * 10 + u16::from(third - b'0')
        }
        _ => return None,
    };
    u8::try_from(version).ok()
}

const fn is_leading_digit(byte: u8) -> bool {
    byte.wrapping_sub(b'1') < 9
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{
        SecWebSocketVersion, SecWebSocketVersionOwned, VersionSet, collect_quoted_version_lines, decode_versions, parse_version,
        parse_version_value,
    };
    use crate::sink::{EncodedValues, FieldSink};
    use crate::source::{FieldLines, FieldSource};
    use crate::{DecodeErrorKind, Field, FieldName, FieldValue, FieldValueRef, TestSink};

    struct Source<'a>(&'a [u8]);

    impl FieldSource for Source<'_> {
        fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
            (name == &FieldName::SecWebSocketVersion).then(|| FieldLines::single(name, self.0))
        }
    }

    #[test]
    fn a_version_set_round_trips_every_representable_version() {
        for version in 0..=u8::MAX {
            let set = VersionSet::one(version);
            assert!(set.contains(version), "{version} is absent after insertion");
            assert!(!set.is_empty());
            assert_eq!(set.len(), 1);
            assert_eq!(set.iter().collect::<Vec<_>>(), vec![version]);
        }

        let mut every = VersionSet::EMPTY;
        assert!(every.is_empty());
        for version in 0..=u8::MAX {
            every.insert(version);
        }
        assert_eq!(every.len(), 256);
        assert_eq!(every.iter().collect::<Vec<_>>(), (0..=u8::MAX).collect::<Vec<_>>());
    }

    #[test]
    fn the_canonical_version_decodes_without_allocating() {
        let owned = SecWebSocketVersionOwned::new(13);
        assert_eq!(owned.requested(), Ok(13));
        assert!(owned.supports(13));
        assert!(!owned.supports(8));
        assert_eq!(owned.versions().collect::<Vec<_>>(), vec![13]);
        assert_eq!(format!("{owned:?}"), "SecWebSocketVersionOwned { versions: 1, .. }");

        let source = Source(b"13");
        let view = SecWebSocketVersion::view(&source)
            .expect("version is valid")
            .expect("version is present");
        assert_eq!(view.requested(), Ok(13));
        let owned = SecWebSocketVersion::owned(&source)
            .expect("version is valid")
            .expect("version is present");
        assert_eq!(owned.requested(), Ok(13));
        assert_eq!(view, owned);
    }

    #[test]
    fn an_advertised_version_list_collapses_to_an_ascending_set() {
        let owned = SecWebSocketVersionOwned::new(8);
        assert_eq!(owned.requested(), Ok(8));

        let source = Source(b"7, 8, 13");
        let view = SecWebSocketVersion::view(&source)
            .expect("versions are valid")
            .expect("versions are present");
        assert_eq!(view.versions().collect::<Vec<_>>(), vec![7, 8, 13]);

        // Order and repetition carry no meaning, so both are dropped.
        let shuffled = Source(b"13, 7, 8, 13");
        assert_eq!(
            SecWebSocketVersion::view(&shuffled)
                .expect("versions are valid")
                .expect("versions are present"),
            view
        );

        let source = Source(b"8");
        let view = SecWebSocketVersion::view(&source)
            .expect("uncommon version is valid")
            .expect("version is present");
        assert_eq!(view.requested(), Ok(8));
    }

    #[test]
    fn the_version_parser_matches_a_reference_implementation() {
        /// Parses the `version` rule of RFC 6455 section 11.3.5 directly.
        fn reference(bytes: &[u8]) -> Option<u8> {
            let text = str::from_utf8(bytes).ok()?;
            if text.len() > 1 && text.starts_with('0') {
                return None;
            }
            if !text.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            text.parse::<u8>().ok()
        }

        for first in 0..=u8::MAX {
            assert_eq!(
                parse_version_value(&[first]),
                reference(&[first]),
                "one-byte disagreement for {first:#04x}"
            );
            for second in 0..=u8::MAX {
                let two = [first, second];
                assert_eq!(parse_version_value(&two), reference(&two), "two-byte disagreement for {two:?}");
            }
        }
        for value in 0_u32..1000 {
            let three = format!("{value:03}");
            assert_eq!(
                parse_version_value(three.as_bytes()),
                reference(three.as_bytes()),
                "three-byte disagreement for {three}"
            );
        }
        assert_eq!(parse_version_value(b""), None);
        assert_eq!(parse_version_value(b"1234"), None);
    }

    #[test]
    fn version_line_substitutions_match_structured_parsing() {
        for literal in [
            b"13".as_slice(),
            b"7, 8, 13",
            b" \t13\t ",
            b" , , \t",
            b"\"13",
            b"\"13\"",
            b"256, \"13",
            b"13, \"13",
            b"13\\13",
            b"13, 256",
        ] {
            let mut bytes = literal.to_vec();
            for index in 0..bytes.len() {
                for replacement in 0..=u8::MAX {
                    bytes[index] = replacement;
                    for prefix in [None, Some(b"13".as_slice()), Some(b"256".as_slice())] {
                        let repeated = [FieldValueRef::new(prefix.unwrap_or_default()), FieldValueRef::new(&bytes)];
                        let lines = if prefix.is_some() {
                            FieldLines::from_borrowed(&FieldName::SecWebSocketVersion, &repeated).unwrap()
                        } else {
                            FieldLines::single(&FieldName::SecWebSocketVersion, &bytes)
                        };
                        let expected = collect_quoted_version_lines(&lines).map(|versions| Some(versions.words));
                        let actual = decode_versions(Some(lines)).map(|value| value.map(|value| value.versions.words));
                        assert_eq!(
                            actual, expected,
                            "{literal:?}, index {index}, replacement {replacement}, prefix {prefix:?}"
                        );
                    }
                }
                bytes[index] = literal[index];
            }
        }
    }

    #[test]
    fn builders_conversions_and_insert_cover_canonical_and_general_storage() {
        let versions = SecWebSocketVersionOwned::new(13).with_version(8).with_version(255);
        assert_eq!(versions.versions().collect::<Vec<_>>(), vec![8, 13, 255]);
        assert_eq!(versions.with_version(8), versions);
        assert_eq!(
            versions.requested().expect_err("multiple versions").kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
        assert_eq!(format!("{versions:?}"), "SecWebSocketVersionOwned { versions: 3, .. }");

        let uncommon = SecWebSocketVersionOwned::new(7).with_version(13);
        assert_eq!(uncommon.versions().collect::<Vec<_>>(), vec![7, 13]);

        assert_eq!(
            SecWebSocketVersionOwned::try_from("13").expect("canonical conversion").requested(),
            Ok(13)
        );
        for value in [
            SecWebSocketVersionOwned::try_from("7, 8, 13"),
            SecWebSocketVersionOwned::try_from(String::from("7, 8, 13")),
            SecWebSocketVersionOwned::try_from(FieldValue::from_static("7, 8, 13")),
        ] {
            assert_eq!(value.expect("valid version list").versions().collect::<Vec<_>>(), vec![7, 8, 13]);
        }
        assert_eq!(
            SecWebSocketVersionOwned::try_from(String::from("13\n"))
                .expect_err("invalid field string")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            SecWebSocketVersionOwned::try_from("13\n")
                .expect_err("invalid borrowed field string")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            SecWebSocketVersionOwned::try_from(FieldValue::from_static("256"))
                .expect_err("out-of-range stored version")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let mut table = TestSink::new();
        SecWebSocketVersion::insert(&mut table, SecWebSocketVersionOwned::new(13)).expect("canonical version inserts");
        assert_eq!(
            table
                .lines(SecWebSocketVersion::name())
                .expect("inserted version")
                .exactly_one()
                .expect("one line")
                .as_bytes(),
            b"13"
        );
        // The whole set renders as one ascending list rather than one line
        // per version.
        SecWebSocketVersion::insert(&mut table, versions).expect("general versions insert");
        assert_eq!(
            table
                .lines(SecWebSocketVersion::name())
                .expect("inserted versions")
                .exactly_one()
                .expect("one line")
                .as_bytes(),
            b"8, 13, 255"
        );
    }

    #[test]
    fn repeated_lines_decode_owned_and_borrowed_and_invalid_lists_fail() {
        let mut table = TestSink::new();
        assert!(SecWebSocketVersion::view(&table).expect("absent view succeeds").is_none());
        assert!(SecWebSocketVersion::owned(&table).expect("absent owned succeeds").is_none());
        table
            .set_values(
                SecWebSocketVersion::name(),
                EncodedValues::from_vec(vec![FieldValue::from_static("7, 8"), FieldValue::from_static("13")]),
            )
            .expect("table accepts versions");
        let view = SecWebSocketVersion::view(&table).expect("view decodes").expect("header is present");
        assert_eq!(view.versions().collect::<Vec<_>>(), vec![7, 8, 13]);
        assert_eq!(
            SecWebSocketVersion::owned(&table)
                .expect("owned decode succeeds")
                .expect("header is present")
                .versions()
                .collect::<Vec<_>>(),
            vec![7, 8, 13]
        );

        for (raw, kind) in [
            (", ,", DecodeErrorKind::MissingValue),
            ("256", DecodeErrorKind::InvalidSyntax),
            ("01", DecodeErrorKind::InvalidSyntax),
            ("\"13", DecodeErrorKind::UnterminatedQuote),
        ] {
            table
                .set_values(
                    SecWebSocketVersion::name(),
                    EncodedValues::single(FieldValue::from_str(raw).expect("safe raw value")),
                )
                .expect("table accepts raw value");
            assert_eq!(SecWebSocketVersion::view(&table).expect_err("invalid version list").kind(), kind);
            assert_eq!(
                SecWebSocketVersion::owned(&table).expect_err("invalid owned version list").kind(),
                kind
            );
        }

        table
            .set_values(SecWebSocketVersion::name(), EncodedValues::single(FieldValue::from_static("8")))
            .expect("table accepts uncommon version");
        assert_eq!(
            SecWebSocketVersion::owned(&table)
                .expect("owned uncommon version decodes")
                .expect("header is present")
                .requested(),
            Ok(8)
        );
    }

    #[test]
    fn private_version_helpers_report_empty_multiple_late_errors_and_bounds() {
        let empty = SecWebSocketVersionOwned {
            versions: VersionSet::EMPTY,
        };
        assert_eq!(empty.requested().expect_err("empty set").kind(), DecodeErrorKind::MissingValue);
        assert_eq!(empty.versions().next(), None);
        assert_eq!(empty.field_value().as_bytes(), b"");
        assert_eq!(empty.to_string(), "");
        assert_eq!(
            SecWebSocketVersionOwned::new(13)
                .with_version(8)
                .requested()
                .expect_err("two versions are not one request version")
                .kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
        assert_eq!(parse_version(b"255"), Ok(255));
        assert_eq!(
            parse_version(b"256").expect_err("out-of-range version").kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(parse_version_value(b"0"), Some(0));
        assert_eq!(parse_version_value(b"9"), Some(9));
        assert_eq!(parse_version_value(b"99"), Some(99));
        assert_eq!(parse_version_value(b"100"), Some(100));
        assert_eq!(parse_version_value(b"255"), Some(255));
        assert_eq!(parse_version_value(b"256"), None);
        assert_eq!(parse_version_value(b"01"), None);
        assert_eq!(parse_version_value(b"000"), None);
        assert_eq!(parse_version_value(b"1000"), None);

        let bare = FieldLines::single(&FieldName::SecWebSocketVersion, b"8, 13");
        assert_eq!(
            super::collect_quoted_version_lines(&bare)
                .expect("bare versions are accepted by the general collector")
                .iter()
                .collect::<Vec<_>>(),
            vec![8, 13]
        );
    }
}
