// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation-free scanners used only by generated route arms.
//!
//! `http::uri::Authority` owns its input and copies a borrowed `Host` field,
//! while `http` provides no borrowed media-field parser. These small linear
//! scanners retain the ecosystem header and URI types without allocating or
//! adding a media-type dependency.

use core::cmp::Ordering;

use http::HeaderMap;
use http::header::{ACCEPT, CONTENT_TYPE, HOST, HeaderValue};
use http::request::Parts;

use super::header::grammar::{is_tchar, parse_quality};

pub(super) enum ContentTypeCardinality {
    Missing,
    Multiple(usize),
}

/// A media type already split into its type and subtype.
#[derive(Clone, Copy, Debug)]
pub struct MediaType<'a> {
    pub(super) top_level: &'a [u8],
    pub(super) subtype: &'a [u8],
}

impl<'a> MediaType<'a> {
    /// Creates a split media type.
    #[must_use]
    pub const fn new(top_level: &'a [u8], subtype: &'a [u8]) -> Self {
        Self { top_level, subtype }
    }

    fn matches(self, actual: Self) -> bool {
        self.top_level.eq_ignore_ascii_case(actual.top_level) && self.subtype.eq_ignore_ascii_case(actual.subtype)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CachedIndex {
    Unparsed,
    Missing,
    Match(usize),
}

impl CachedIndex {
    fn parsed(value: Option<usize>) -> Self {
        value.map_or(Self::Missing, Self::Match)
    }

    fn is_match(self, expected: usize) -> bool {
        matches!(self, Self::Match(actual) if actual == expected)
    }
}

/// Lazily parsed request predicates shared by one generated overlap group.
#[derive(Debug)]
pub struct OverlapPredicateState<const HOSTS: usize, const CONSUMES: usize, const PRODUCES: usize> {
    hosts: &'static [&'static str; HOSTS],
    consumes: &'static [MediaType<'static>; CONSUMES],
    produces: &'static [MediaType<'static>; PRODUCES],
    produces_top_level: Option<&'static [u8]>,
    host_match: CachedIndex,
    content_type_match: CachedIndex,
    accept_matches: Option<[u16; PRODUCES]>,
}

impl<const HOSTS: usize, const CONSUMES: usize, const PRODUCES: usize> OverlapPredicateState<HOSTS, CONSUMES, PRODUCES> {
    /// Creates request-scoped state from generated predicate constants.
    #[must_use]
    pub const fn new(
        hosts: &'static [&'static str; HOSTS],
        consumes: &'static [MediaType<'static>; CONSUMES],
        produces: &'static [MediaType<'static>; PRODUCES],
        produces_top_level: Option<&'static [u8]>,
    ) -> Self {
        Self {
            hosts,
            consumes,
            produces,
            produces_top_level,
            host_match: CachedIndex::Unparsed,
            content_type_match: CachedIndex::Unparsed,
            accept_matches: None,
        }
    }

    /// Tests one generated host constant, extracting and validating the request
    /// authority only on the first host test.
    #[must_use]
    #[inline]
    pub fn host(&mut self, parts: &Parts, index: usize) -> bool {
        if self.host_match == CachedIndex::Unparsed {
            self.host_match = CachedIndex::parsed(matching_host(parts, self.hosts));
        }
        self.host_match.is_match(index)
    }

    /// Tests one generated consumed media type, parsing the request
    /// `Content-Type` only on the first consumes test.
    #[must_use]
    #[inline]
    pub fn consumes(&mut self, headers: &HeaderMap, index: usize) -> bool {
        if self.content_type_match == CachedIndex::Unparsed {
            self.content_type_match = CachedIndex::parsed(matching_content_type(headers, self.consumes));
        }
        self.content_type_match.is_match(index)
    }

    /// Tests one generated produced media type, scanning all `Accept` fields
    /// only on the first produces test.
    ///
    /// The scan follows the rules documented on [`accepts`].
    #[must_use]
    #[inline]
    pub fn produces(&mut self, headers: &HeaderMap, index: usize) -> bool {
        if self.accept_matches.is_none() {
            let mut matches = [0; PRODUCES];
            scan_accept(headers, self.produces, self.produces_top_level, &mut matches);
            self.accept_matches = Some(matches);
        }
        self.accept_matches
            .as_ref()
            .and_then(|matches| matches.get(index))
            .is_some_and(|score| quality(*score) != 0)
    }
}

/// Checks the URI authority, or the single `Host` field when the URI has none.
#[must_use]
#[inline]
pub fn host_matches(parts: &Parts, expected: &str) -> bool {
    request_authority(parts).is_some_and(|actual| actual.eq_ignore_ascii_case(expected.as_bytes()))
}

/// Checks one concrete request `Content-Type`, including legal parameters.
#[must_use]
#[inline]
pub fn content_type_matches(headers: &HeaderMap, expected: &str) -> bool {
    expected
        .split_once('/')
        .is_some_and(|(top_level, subtype)| content_type_matches_parsed(headers, MediaType::new(top_level.as_bytes(), subtype.as_bytes())))
}

/// Checks one request `Content-Type` against an already split media type.
#[must_use]
#[inline]
pub fn content_type_matches_parsed(headers: &HeaderMap, expected: MediaType<'_>) -> bool {
    let value = match single_content_type(headers) {
        Ok(value) => value,
        Err(ContentTypeCardinality::Missing) => return false,
        Err(ContentTypeCardinality::Multiple(count)) => {
            debug_assert!(count > 1, "multiple Content-Type values must have a count above one");
            return false;
        }
    };
    parse_content_type(value.as_bytes()).is_some_and(|actual| expected.matches(actual))
}

pub(super) fn single_content_type(headers: &HeaderMap) -> Result<&HeaderValue, ContentTypeCardinality> {
    let mut values = headers.get_all(CONTENT_TYPE).iter();
    let Some(value) = values.next() else {
        return Err(ContentTypeCardinality::Missing);
    };
    let additional = values.count();
    if additional != 0 {
        return Err(ContentTypeCardinality::Multiple(additional + 1));
    }
    Ok(value)
}

/// Checks whether all `Accept` fields validly permit one concrete media type.
///
/// This is the negotiation a `produces = "..."` route predicate performs, and
/// it decides between dispatching the route and answering `406 Not
/// Acceptable`. Three rules explain nearly every surprising `406`:
///
/// - A request with no `Accept` field, or whose `Accept` fields contain no
///   media range at all — an empty value, or one made only of whitespace and
///   commas such as `", ,"` — accepts every produced media type, as [RFC 9110]
///   specifies for an absent field.
/// - A media range carrying a media parameter *before* its `q` weight is never
///   matched, because it names a narrower variant that a bare produced media
///   type does not claim to satisfy. `Accept: application/json;charset=utf-8`
///   and `Accept: application/json;version=2` therefore both yield `406`
///   against `produces = "application/json"`. Declare the parameterized media
///   type explicitly if the route really serves that variant.
/// - Accept extension parameters, which follow `q`, are ignored, so
///   `Accept: application/json;q=0.5;extension=ok` does match
///   `produces = "application/json"`.
///
/// A field line that does not obey the `Accept` grammar fails closed: the whole
/// request is treated as accepting nothing, because a value that cannot be read
/// cannot be honored.
///
/// [RFC 9110]: https://www.rfc-editor.org/rfc/rfc9110#field.accept
#[must_use]
#[inline]
pub fn accepts(headers: &HeaderMap, produced: &str) -> bool {
    let Some((produced_type, produced_subtype)) = produced.split_once('/') else {
        return false;
    };
    accepts_parsed(headers, MediaType::new(produced_type.as_bytes(), produced_subtype.as_bytes()))
}

/// Checks all `Accept` fields against an already split produced media type.
///
/// This applies the rules documented on [`accepts`].
#[must_use]
#[inline]
pub fn accepts_parsed(headers: &HeaderMap, produced: MediaType<'_>) -> bool {
    let mut matches = [0];
    scan_accept(headers, core::slice::from_ref(&produced), Some(produced.top_level), &mut matches);
    quality(matches[0]) != 0
}

fn request_authority(parts: &Parts) -> Option<&[u8]> {
    if let Some(authority) = parts.uri.authority() {
        return Some(authority.as_str().as_bytes());
    }

    let mut values = parts.headers.get_all(HOST).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    Some(value.as_bytes())
}

fn matching_host(parts: &Parts, expected: &[&str]) -> Option<usize> {
    // Comparing with a compile-validated authority proves validity on a match;
    // separately validating an unmatched request value would duplicate its
    // scan without changing the required HostMismatch result.
    find_ascii_case_insensitive(expected, request_authority(parts)?)
}

fn matching_content_type(headers: &HeaderMap, expected: &[MediaType<'_>]) -> Option<usize> {
    let value = single_content_type(headers).ok()?;
    find_media_type(expected, parse_content_type(value.as_bytes())?)
}

fn find_ascii_case_insensitive(expected: &[&str], actual: &[u8]) -> Option<usize> {
    let first = expected.first()?;
    if first.as_bytes().eq_ignore_ascii_case(actual) {
        return Some(0);
    }
    let mut left = 1;
    let mut right = expected.len();
    while left < right {
        let middle = left + (right - left) / 2;
        match compare_ascii_case_insensitive(expected[middle].as_bytes(), actual) {
            Ordering::Less => left = middle + 1,
            Ordering::Greater => right = middle,
            Ordering::Equal => return Some(middle),
        }
    }
    None
}

fn find_media_type(expected: &[MediaType<'_>], actual: MediaType<'_>) -> Option<usize> {
    let first = *expected.first()?;
    if first.matches(actual) {
        return Some(0);
    }
    let mut left = 1;
    let mut right = expected.len();
    while left < right {
        let middle = left + (right - left) / 2;
        match compare_media_type(expected[middle], actual) {
            Ordering::Less => left = middle + 1,
            Ordering::Greater => right = middle,
            Ordering::Equal => return Some(middle),
        }
    }
    None
}

fn find_accept_media_type(expected: &[MediaType<'_>], common_top_level: Option<&[u8]>, actual: MediaType<'_>) -> Option<usize> {
    let Some(common_top_level) = common_top_level else {
        return find_media_type(expected, actual);
    };
    if !common_top_level.eq_ignore_ascii_case(actual.top_level) {
        return None;
    }

    let first = *expected.first()?;
    if first.subtype.eq_ignore_ascii_case(actual.subtype) {
        return Some(0);
    }
    let mut left = 1;
    let mut right = expected.len();
    while left < right {
        let middle = left + (right - left) / 2;
        match compare_ascii_case_insensitive(expected[middle].subtype, actual.subtype) {
            Ordering::Less => left = middle + 1,
            Ordering::Greater => right = middle,
            Ordering::Equal => return Some(middle),
        }
    }
    None
}

fn compare_media_type(left: MediaType<'_>, right: MediaType<'_>) -> core::cmp::Ordering {
    compare_ascii_case_insensitive(left.top_level, right.top_level)
        .then_with(|| compare_ascii_case_insensitive(left.subtype, right.subtype))
}

fn compare_ascii_case_insensitive(left: &[u8], right: &[u8]) -> core::cmp::Ordering {
    left.iter()
        .map(u8::to_ascii_lowercase)
        .cmp(right.iter().map(u8::to_ascii_lowercase))
}

const QUALITY_MASK: u16 = 0x03ff;

/// The score every produced media type receives when the request expresses no
/// preference at all, equivalent to a `*/*` range at the maximum quality.
const DEFAULT_ACCEPT_SCORE: u16 = 1024 + 1000;

const fn quality(score: u16) -> u16 {
    score & QUALITY_MASK
}

/// What one well-formed or malformed `Accept` field line contributed.
enum AcceptOutcome {
    /// The field line does not obey the `Accept` grammar.
    Malformed,
    /// The field line is well formed but carries no media range at all.
    NoRange,
    /// The field line carries at least one media range.
    Ranges,
}

fn scan_accept(headers: &HeaderMap, produced: &[MediaType<'_>], common_top_level: Option<&[u8]>, matches: &mut [u16]) {
    debug_assert_eq!(
        produced.len(),
        matches.len(),
        "generated Accept media types and score slots must have equal lengths"
    );

    let mut saw_range = false;
    for value in headers.get_all(ACCEPT) {
        match parse_accept(value.as_bytes(), produced, common_top_level, matches) {
            AcceptOutcome::Malformed => {
                matches.fill(0);
                return;
            }
            AcceptOutcome::NoRange => {}
            AcceptOutcome::Ranges => saw_range = true,
        }
    }

    // No field line named a media range, so the request states no preference:
    // an empty or comma-only `Accept` is treated exactly like an absent one.
    if !saw_range {
        matches.fill(DEFAULT_ACCEPT_SCORE);
    }
}

pub(super) fn parse_content_type(value: &[u8]) -> Option<MediaType<'_>> {
    let mut cursor = Cursor::new(value);
    cursor.skip_ows();
    let top_level = cursor.token()?;
    if !cursor.consume(b'/') {
        return None;
    }
    let subtype = cursor.token()?;

    cursor.skip_ows();
    while !cursor.is_empty() {
        if !cursor.consume(b';') {
            return None;
        }
        cursor.skip_ows();
        cursor.token()?;
        cursor.skip_ows();
        if !cursor.consume(b'=') {
            return None;
        }
        cursor.skip_ows();
        if !cursor.parameter_value() {
            return None;
        }
        cursor.skip_ows();
    }
    Some(MediaType::new(top_level, subtype))
}

fn parse_accept(value: &[u8], produced: &[MediaType<'_>], common_top_level: Option<&[u8]>, matches: &mut [u16]) -> AcceptOutcome {
    let mut cursor = Cursor::new(value);
    let mut saw_range = false;
    loop {
        cursor.skip_ows();
        while cursor.consume(b',') {
            cursor.skip_ows();
        }
        if cursor.is_empty() {
            return if saw_range { AcceptOutcome::Ranges } else { AcceptOutcome::NoRange };
        }
        let Some(range) = parse_media_range(&mut cursor) else {
            return AcceptOutcome::Malformed;
        };
        saw_range = true;
        update_accept_matches(range, produced, common_top_level, matches);
        cursor.skip_ows();
        if cursor.is_empty() {
            return AcceptOutcome::Ranges;
        }
        if !cursor.consume(b',') {
            return AcceptOutcome::Malformed;
        }
    }
}

#[derive(Clone, Copy)]
struct AcceptRange<'a> {
    top_level: &'a [u8],
    subtype: &'a [u8],
    quality: u16,
    specificity: u16,
    has_media_parameter: bool,
}

fn parse_media_range<'a>(cursor: &mut Cursor<'a>) -> Option<AcceptRange<'a>> {
    let top_level = cursor.token()?;
    if !cursor.consume(b'/') {
        return None;
    }
    let subtype = cursor.token()?;

    let type_wildcard = top_level == b"*";
    let subtype_wildcard = subtype == b"*";
    if type_wildcard && !subtype_wildcard {
        return None;
    }
    let mut has_media_parameter = false;
    let mut quality = 1000;
    let mut saw_quality = false;

    cursor.skip_ows();
    while cursor.consume(b';') {
        cursor.skip_ows();
        let name = cursor.token()?;
        cursor.skip_ows();
        let has_value = cursor.consume(b'=');
        if has_value {
            cursor.skip_ows();
        }

        if name.eq_ignore_ascii_case(b"q") {
            if saw_quality || !has_value {
                return None;
            }
            let value = cursor.token()?;
            let parsed = parse_quality(value)?;
            quality = parsed;
            saw_quality = true;
        } else {
            if !saw_quality {
                has_media_parameter = true;
            }
            if has_value && !cursor.parameter_value() {
                return None;
            }
            if !has_value && !saw_quality {
                return None;
            }
        }
        cursor.skip_ows();
    }

    let specificity = if type_wildcard {
        0
    } else if subtype_wildcard {
        1
    } else {
        2
    };
    Some(AcceptRange {
        top_level,
        subtype,
        quality,
        specificity,
        has_media_parameter,
    })
}

fn update_accept_matches(range: AcceptRange<'_>, produced: &[MediaType<'_>], common_top_level: Option<&[u8]>, matches: &mut [u16]) {
    // A media parameter before `q` narrows the range to a specific variant that
    // a bare produced media type does not claim to serve, so the range is
    // ignored rather than credited. See `accepts` for the rationale.
    if range.has_media_parameter {
        return;
    }
    let score = (range.specificity + 1) * 1024 + range.quality;
    if range.top_level == b"*" {
        for current in matches {
            *current = (*current).max(score);
        }
    } else if range.subtype == b"*" {
        if common_top_level.is_some_and(|top_level| top_level.eq_ignore_ascii_case(range.top_level)) {
            for current in matches {
                *current = (*current).max(score);
            }
        } else if common_top_level.is_none() {
            for (candidate, current) in produced.iter().zip(matches) {
                if candidate.top_level.eq_ignore_ascii_case(range.top_level) {
                    *current = (*current).max(score);
                }
            }
        }
    } else if let Some(index) = find_accept_media_type(produced, common_top_level, MediaType::new(range.top_level, range.subtype))
        && let Some(current) = matches.get_mut(index)
    {
        *current = (*current).max(score);
    }
}

struct Cursor<'a> {
    value: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    const fn new(value: &'a [u8]) -> Self {
        Self { value, offset: 0 }
    }

    fn is_empty(&self) -> bool {
        self.offset == self.value.len()
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.value.get(self.offset) == Some(&expected) {
            self.offset += 1;
            true
        } else {
            false
        }
    }

    fn skip_ows(&mut self) {
        while self.value.get(self.offset).is_some_and(|byte| matches!(byte, b' ' | b'\t')) {
            self.offset += 1;
        }
    }

    fn token(&mut self) -> Option<&'a [u8]> {
        let start = self.offset;
        while self.value.get(self.offset).is_some_and(|byte| is_tchar(*byte)) {
            self.offset += 1;
        }
        (self.offset != start).then(|| &self.value[start..self.offset])
    }

    fn parameter_value(&mut self) -> bool {
        if self.value.get(self.offset) == Some(&b'"') {
            self.quoted_string()
        } else {
            self.token().is_some()
        }
    }

    fn quoted_string(&mut self) -> bool {
        if !self.consume(b'"') {
            return false;
        }
        while let Some(&byte) = self.value.get(self.offset) {
            self.offset += 1;
            match byte {
                b'"' => return true,
                b'\\' => {
                    let Some(&escaped) = self.value.get(self.offset) else {
                        return false;
                    };
                    if !is_quoted_pair_byte(escaped) {
                        return false;
                    }
                    self.offset += 1;
                }
                byte if is_quoted_text_byte(byte) => {}
                _ => return false,
            }
        }
        false
    }
}

const fn is_quoted_text_byte(byte: u8) -> bool {
    matches!(byte, b'\t' | b' ' | b'!' | b'#'..=b'[' | b']'..=b'~' | 0x80..=0xff)
}

const fn is_quoted_pair_byte(byte: u8) -> bool {
    matches!(byte, b'\t' | b' ' | b'!'..=b'~' | 0x80..=0xff)
}

#[cfg(test)]
fn is_http_authority(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || bytes
            .iter()
            .any(|byte| !byte.is_ascii() || byte.is_ascii_whitespace() || matches!(byte, b'/' | b'?' | b'#' | b'@' | b'\\'))
    {
        return false;
    }

    if bytes[0] == b'[' {
        let Some(end) = bytes.iter().position(|byte| *byte == b']') else {
            return false;
        };
        if end == 1 || !valid_ip_literal(&value[1..end]) {
            return false;
        }
        return valid_authority_port(&value[end + 1..]);
    }

    if bytes.contains(&b'[') || bytes.contains(&b']') {
        return false;
    }
    let (host, port) = match value.split_once(':') {
        Some((host, port)) if !port.contains(':') => (host, Some(port)),
        Some(_) => return false,
        None => (value, None),
    };
    !host.is_empty() && valid_reg_name(host) && port.is_none_or(valid_port)
}

#[cfg(test)]
fn valid_authority_port(suffix: &str) -> bool {
    suffix.is_empty() || suffix.strip_prefix(':').is_some_and(valid_port)
}

#[cfg(test)]
fn valid_port(port: &str) -> bool {
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    port.bytes()
        .try_fold(0_u16, |value, byte| {
            value.checked_mul(10).and_then(|value| value.checked_add(u16::from(byte - b'0')))
        })
        .is_some()
}

#[cfg(test)]
fn valid_reg_name(value: &str) -> bool {
    valid_percent_encoded_sequence(value.as_bytes(), |byte| {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'.' | b'_' | b'~' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'='
            )
    })
}

#[cfg(test)]
fn valid_ip_literal(value: &str) -> bool {
    if let Some(version) = value.strip_prefix('v').or_else(|| value.strip_prefix('V')) {
        let Some((version, address)) = version.split_once('.') else {
            return false;
        };
        return !version.is_empty()
            && version.bytes().all(|byte| byte.is_ascii_hexdigit())
            && !address.is_empty()
            && address.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(
                        byte,
                        b'-' | b'.' | b'_' | b'~' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=' | b':'
                    )
            });
    }

    let (address, zone) = value
        .split_once("%25")
        .map_or((value, None), |(address, zone)| (address, Some(zone)));
    address.parse::<core::net::Ipv6Addr>().is_ok()
        && zone.is_none_or(|zone| {
            !zone.is_empty()
                && valid_percent_encoded_sequence(zone.as_bytes(), |byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
                })
        })
}

#[cfg(test)]
fn valid_percent_encoded_sequence(bytes: &[u8], ordinary: impl Fn(u8) -> bool) -> bool {
    let mut index = 0;
    while index < bytes.len() {
        if ordinary(bytes[index]) {
            index += 1;
        } else if bytes[index] == b'%'
            && bytes
                .get(index + 1..index + 3)
                .is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit))
        {
            index += 3;
        } else {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use http::Request;

    use super::*;

    /// The generated tables keep the winner first and sort the tail with the
    /// `(top_level, subtype)` comparator this binary search uses. Sorting the
    /// joined `type/subtype` strings instead diverges whenever a top-level type
    /// contains a tchar below `/`, and the search then misses an entry.
    #[test]
    fn media_type_tables_sorted_by_type_and_subtype_are_fully_searchable() {
        use alloc::vec::Vec;

        fn split(value: &str) -> MediaType<'_> {
            let (top_level, subtype) = value.split_once('/').expect("test media types are `type/subtype`");
            MediaType::new(top_level.as_bytes(), subtype.as_bytes())
        }

        let values = ["application/json", "x.foo/bar", "x-foo/bar", "x+foo/bar", "x/bar", "x/baz"];
        let mut tail: Vec<&str> = values[1..].to_vec();
        tail.sort_unstable_by_key(|value| value.split_once('/').expect("test media types are `type/subtype`"));
        let table: Vec<MediaType<'_>> = core::iter::once(values[0]).chain(tail).map(split).collect();

        for value in values {
            assert!(find_media_type(&table, split(value)).is_some(), "{value} is not found");
        }
        assert_eq!(find_media_type(&table, split("x/absent")), None);
    }

    #[test]
    fn authorities_require_a_host_and_valid_optional_port() {
        for valid in [
            "api.example",
            "API.EXAMPLE:443",
            "[2001:db8::1]",
            "[2001:db8::1]:8443",
            "[fe80::1%25eth0]",
            "[v1.future]:80",
        ] {
            assert!(is_http_authority(valid), "{valid}");
        }
        for invalid in [
            "",
            ":80",
            "api.example:",
            "api.example:65536",
            "api.example:abc",
            "https://api.example",
            "user@api.example",
            "api.example/path",
            "api example",
            "2001:db8::1",
            "[not-ipv6]",
        ] {
            assert!(!is_http_authority(invalid), "{invalid}");
        }
    }

    #[test]
    fn uri_authority_precedes_one_valid_host_field() {
        let request = Request::get("http://api.example:80/items")
            .header(HOST, "wrong.example")
            .body(())
            .expect("test request metadata is valid");
        let (parts, ()) = request.into_parts();
        assert!(host_matches(&parts, "API.EXAMPLE:80"));
        assert!(!host_matches(&parts, "wrong.example"));

        let request = Request::get("/items")
            .header(HOST, "[2001:DB8::1]:443")
            .body(())
            .expect("test request metadata is valid");
        let (parts, ()) = request.into_parts();
        assert!(host_matches(&parts, "[2001:db8::1]:443"));
    }

    #[test]
    fn content_type_requires_one_well_formed_matching_value() {
        let matching = headers(CONTENT_TYPE, " Application/JSON ; charset=\"utf-8\" ");
        assert!(content_type_matches(&matching, "application/json"));
        assert!(!content_type_matches(&matching, "text/plain"));

        for malformed in [
            "application",
            "application/",
            "application /json",
            "application/json, text/plain",
            "application/json; charset",
            "application/json; charset=\"unterminated",
        ] {
            assert!(!content_type_matches(&headers(CONTENT_TYPE, malformed), "application/json"));
        }
        let mut duplicate = headers(CONTENT_TYPE, "application/json");
        duplicate.append(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        assert!(!content_type_matches(&duplicate, "application/json"));
        assert!(!content_type_matches(&HeaderMap::new(), "application/json"));
    }

    #[test]
    fn accept_uses_the_most_specific_matching_quality() {
        assert!(accepts(&HeaderMap::new(), "application/json"));
        for value in [
            "application/json",
            "application/*",
            "*/*",
            "text/plain, application/json;q=0.2",
            "application/json;q=0.5;extension=ok",
            ", application/json,",
        ] {
            assert!(accepts(&headers(ACCEPT, value), "application/json"), "{value}");
        }
        for value in [
            "text/plain",
            "*/*;q=1, application/json;q=0",
            "application/*;q=1, application/json;q=0",
            "application/json;q=2",
            "application/json;q=.5",
            "application/json;q=0.000",
            "application/json;version=2",
            "*/json",
            "application/json trailing",
        ] {
            assert!(!accepts(&headers(ACCEPT, value), "application/json"), "{value}");
        }
    }

    #[test]
    fn accept_combines_multiple_field_lines_and_rejects_any_malformed_line() {
        let mut values = headers(ACCEPT, "text/plain");
        values.append(ACCEPT, HeaderValue::from_static("application/json;q=0.4"));
        assert!(accepts(&values, "application/json"));

        values.append(ACCEPT, HeaderValue::from_static("broken"));
        assert!(!accepts(&values, "application/json"));
    }

    #[test]
    fn an_accept_field_without_any_media_range_is_treated_as_absent() {
        for value in ["", " ", ",", ", ,", " \t , , \t "] {
            assert!(accepts(&headers(ACCEPT, value), "application/json"), "{value:?}");
        }
    }

    #[test]
    fn an_empty_accept_line_does_not_reset_an_earlier_lines_preference() {
        let mut values = headers(ACCEPT, "text/plain");
        values.append(ACCEPT, HeaderValue::from_static(""));
        assert!(accepts(&values, "text/plain"));
        assert!(!accepts(&values, "application/json"));

        let mut values = headers(ACCEPT, "");
        values.append(ACCEPT, HeaderValue::from_static("text/plain"));
        assert!(accepts(&values, "text/plain"));
        assert!(!accepts(&values, "application/json"));
    }

    fn headers(name: http::header::HeaderName, value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(name, HeaderValue::from_static(value));
        headers
    }
}
