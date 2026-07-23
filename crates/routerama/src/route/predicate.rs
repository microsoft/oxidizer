// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation-free scanners used only by generated route arms.
//!
//! `http::uri::Authority` owns its input and copies a borrowed `Host` field,
//! while `http` provides no borrowed media-field parser. These small linear
//! scanners retain the ecosystem header and URI types without allocating or
//! adding a media-type dependency.

use http::header::{ACCEPT, CONTENT_TYPE, HOST, HeaderValue};
use http::request::Parts;
use http::{HeaderMap, Response};

pub(super) enum ContentTypeCardinality {
    Missing,
    Multiple(usize),
}

pub(super) struct ParsedMediaType<'a> {
    pub(super) top_level: &'a [u8],
    pub(super) subtype: &'a [u8],
}

/// Checks the URI authority, or the single `Host` field when the URI has none.
#[must_use]
#[inline]
pub fn host_matches(parts: &Parts, expected: &str) -> bool {
    if let Some(authority) = parts.uri.authority() {
        let authority = authority.as_str();
        return is_http_authority(authority) && authority.eq_ignore_ascii_case(expected);
    }

    let mut values = parts.headers.get_all(HOST).iter();
    let Some(value) = values.next() else {
        return false;
    };
    if values.next().is_some() {
        return false;
    }
    let Ok(value) = value.to_str() else {
        return false;
    };
    is_http_authority(value) && value.eq_ignore_ascii_case(expected)
}

/// Checks one concrete request `Content-Type`, including legal parameters.
#[must_use]
#[inline]
pub fn content_type_matches(headers: &HeaderMap, expected: &str) -> bool {
    let value = match single_content_type(headers) {
        Ok(value) => value,
        Err(ContentTypeCardinality::Missing) => return false,
        Err(ContentTypeCardinality::Multiple(count)) => {
            debug_assert!(count > 1, "multiple Content-Type values must have a count above one");
            return false;
        }
    };
    parse_content_type(value.as_bytes()).is_some_and(|actual| {
        expected.split_once('/').is_some_and(|(expected_type, expected_subtype)| {
            actual.top_level.eq_ignore_ascii_case(expected_type.as_bytes())
                && actual.subtype.eq_ignore_ascii_case(expected_subtype.as_bytes())
        })
    })
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
#[must_use]
#[inline]
pub fn accepts(headers: &HeaderMap, produced: &str) -> bool {
    let Some((produced_type, produced_subtype)) = produced.split_once('/') else {
        return false;
    };
    let mut values = headers.get_all(ACCEPT).iter();
    let Some(first) = values.next() else {
        return true;
    };

    let mut best = None;
    if !parse_accept(first.as_bytes(), produced_type.as_bytes(), produced_subtype.as_bytes(), &mut best) {
        return false;
    }
    for value in values {
        if !parse_accept(value.as_bytes(), produced_type.as_bytes(), produced_subtype.as_bytes(), &mut best) {
            return false;
        }
    }
    best.is_some_and(|(_, quality)| quality != 0)
}

/// Replaces the handler response's `Content-Type` with its route declaration.
pub fn set_produced_content_type<B>(response: &mut Response<B>, produced: &'static str) {
    response.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static(produced));
}

pub(super) fn parse_content_type(value: &[u8]) -> Option<ParsedMediaType<'_>> {
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
    Some(ParsedMediaType { top_level, subtype })
}

fn parse_accept(value: &[u8], produced_type: &[u8], produced_subtype: &[u8], best: &mut Option<(u8, u16)>) -> bool {
    let mut cursor = Cursor::new(value);
    loop {
        cursor.skip_ows();
        while cursor.consume(b',') {
            cursor.skip_ows();
        }
        if cursor.is_empty() {
            return true;
        }
        if !parse_media_range(&mut cursor, produced_type, produced_subtype, best) {
            return false;
        }
        cursor.skip_ows();
        if cursor.is_empty() {
            return true;
        }
        if !cursor.consume(b',') {
            return false;
        }
    }
}

fn parse_media_range(cursor: &mut Cursor<'_>, produced_type: &[u8], produced_subtype: &[u8], best: &mut Option<(u8, u16)>) -> bool {
    let Some(top_level) = cursor.token() else {
        return false;
    };
    if !cursor.consume(b'/') {
        return false;
    }
    let Some(subtype) = cursor.token() else {
        return false;
    };

    let type_wildcard = top_level == b"*";
    let subtype_wildcard = subtype == b"*";
    if type_wildcard && !subtype_wildcard {
        return false;
    }
    let type_matches = type_wildcard || top_level.eq_ignore_ascii_case(produced_type);
    let subtype_matches = subtype_wildcard || subtype.eq_ignore_ascii_case(produced_subtype);
    let mut has_media_parameter = false;
    let mut quality = 1000;
    let mut saw_quality = false;

    cursor.skip_ows();
    while cursor.consume(b';') {
        cursor.skip_ows();
        let Some(name) = cursor.token() else {
            return false;
        };
        cursor.skip_ows();
        let has_value = cursor.consume(b'=');
        if has_value {
            cursor.skip_ows();
        }

        if name.eq_ignore_ascii_case(b"q") {
            if saw_quality || !has_value {
                return false;
            }
            let Some(value) = cursor.token() else {
                return false;
            };
            let Some(parsed) = parse_quality(value) else {
                return false;
            };
            quality = parsed;
            saw_quality = true;
        } else {
            if !saw_quality {
                has_media_parameter = true;
            }
            if has_value && !cursor.parameter_value() {
                return false;
            }
            if !has_value && !saw_quality {
                return false;
            }
        }
        cursor.skip_ows();
    }

    if type_matches && subtype_matches && !has_media_parameter {
        let specificity = if type_wildcard {
            0
        } else if subtype_wildcard {
            1
        } else {
            2
        };
        match best {
            Some((best_specificity, best_quality)) if *best_specificity > specificity => {}
            Some((best_specificity, best_quality)) if *best_specificity == specificity => {
                *best_quality = (*best_quality).max(quality);
            }
            _ => *best = Some((specificity, quality)),
        }
    }
    true
}

fn parse_quality(value: &[u8]) -> Option<u16> {
    let (&whole, fraction) = value.split_first()?;
    if whole != b'0' && whole != b'1' {
        return None;
    }
    if fraction.is_empty() {
        return Some(if whole == b'1' { 1000 } else { 0 });
    }
    let digits = fraction.strip_prefix(b".")?;
    if digits.len() > 3 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if whole == b'1' {
        return digits.iter().all(|digit| *digit == b'0').then_some(1000);
    }
    let mut quality = 0;
    let mut scale = 100;
    for digit in digits {
        quality += u16::from(*digit - b'0') * scale;
        scale /= 10;
    }
    Some(quality)
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

const fn is_tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
        )
}

const fn is_quoted_text_byte(byte: u8) -> bool {
    matches!(byte, b'\t' | b' ' | b'!' | b'#'..=b'[' | b']'..=b'~' | 0x80..=0xff)
}

const fn is_quoted_pair_byte(byte: u8) -> bool {
    matches!(byte, b'\t' | b' ' | b'!'..=b'~' | 0x80..=0xff)
}

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

fn valid_authority_port(suffix: &str) -> bool {
    suffix.is_empty() || suffix.strip_prefix(':').is_some_and(valid_port)
}

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

fn valid_reg_name(value: &str) -> bool {
    valid_percent_encoded_sequence(value.as_bytes(), |byte| {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'.' | b'_' | b'~' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'='
            )
    })
}

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

    fn headers(name: http::header::HeaderName, value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(name, HeaderValue::from_static(value));
        headers
    }
}
