// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Strict percent decoding for path variables and query parameters.
//!
//! Unchanged input is borrowed. Malformed escapes and invalid UTF-8 are
//! rejected.

use std::borrow::Cow;

/// Decodes `%XX` escapes in a path-segment value.
///
/// A `+` is left as a literal `+`: path segments do not use the
/// `application/x-www-form-urlencoded` `+`-for-space convention.
pub(crate) fn decode_path(value: &str) -> Option<Cow<'_, str>> {
    decode(value, false, false)
}

/// Decodes a multi-segment path-variable value while preserving encoded
/// slashes, as required by the default `google.api.http` reserved-expansion
/// semantics.
pub(crate) fn decode_reserved_path(value: &str) -> Option<Cow<'_, str>> {
    decode(value, false, true)
}

/// Decodes a query-parameter value: `%XX` escapes plus `+` as a space (the
/// `application/x-www-form-urlencoded` convention query strings follow).
pub(crate) fn decode_query(value: &str) -> Option<Cow<'_, str>> {
    decode(value, true, false)
}

/// Returns whether decoding would inspect an escape or query-space marker.
pub(crate) fn needs_decoding(value: &str, plus_as_space: bool) -> bool {
    value.bytes().any(|b| b == b'%' || (plus_as_space && b == b'+'))
}

fn decode(value: &str, plus_as_space: bool, preserve_slash: bool) -> Option<Cow<'_, str>> {
    if !needs_decoding(value, plus_as_space) {
        return Some(Cow::Borrowed(value));
    }

    let mut out = Vec::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        match byte {
            b'%' => {
                let mut lookahead = bytes.clone();
                let hi = lookahead.next()?;
                let lo = lookahead.next()?;
                let decoded = hex_pair(hi, lo)?;
                if preserve_slash && decoded == b'/' {
                    out.extend_from_slice(&[b'%', hi, lo]);
                } else {
                    out.push(decoded);
                }
                bytes = lookahead;
            }
            b'+' if plus_as_space => out.push(b' '),
            other => out.push(other),
        }
    }

    String::from_utf8(out).ok().map(Cow::Owned)
}

fn hex_pair(hi: u8, lo: u8) -> Option<u8> {
    Some((hex_val(hi)? << 4) + hex_val(lo)?)
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borrows_when_nothing_to_decode() {
        assert!(matches!(decode_path("plain-value"), Some(Cow::Borrowed("plain-value"))));
        assert!(matches!(decode_query("plain-value"), Some(Cow::Borrowed("plain-value"))));
        assert!(matches!(decode_path("a+b"), Some(Cow::Borrowed("a+b"))));
    }

    #[test]
    fn decodes_percent_escapes() {
        assert_eq!(decode_path("a%20b").as_deref(), Some("a b"));
        assert_eq!(decode_path("shelves%2F7").as_deref(), Some("shelves/7"));
        assert_eq!(decode_query("science%20fiction").as_deref(), Some("science fiction"));
    }

    #[test]
    fn query_decodes_plus_as_space_but_path_does_not() {
        assert_eq!(decode_query("a+b").as_deref(), Some("a b"));
        assert_eq!(decode_path("a+b").as_deref(), Some("a+b"));
    }

    #[test]
    fn decodes_multibyte_utf8() {
        assert_eq!(decode_path("caf%C3%A9").as_deref(), Some("café"));
    }

    #[test]
    fn decodes_lowercase_hex_digits() {
        assert_eq!(decode_path("%2f").as_deref(), Some("/"));
        assert_eq!(decode_path("caf%c3%a9").as_deref(), Some("café"));
    }

    #[test]
    fn path_keeps_a_literal_plus_even_when_decoding() {
        assert_eq!(decode_path("a+%20b").as_deref(), Some("a+ b"));
        assert_eq!(decode_query("a+%20b").as_deref(), Some("a  b"));
    }

    #[test]
    fn rejects_malformed_escapes() {
        assert!(decode_path("100%").is_none());
        assert!(decode_path("50%xy").is_none());
        assert!(decode_path("%2").is_none());
    }

    #[test]
    fn rejects_invalid_utf8() {
        assert!(decode_path("%FF").is_none());
    }

    #[test]
    fn reserved_expansion_preserves_encoded_slashes() {
        assert_eq!(
            decode_reserved_path("shelves%2F7%20featured").as_deref(),
            Some("shelves%2F7 featured")
        );
    }
}
