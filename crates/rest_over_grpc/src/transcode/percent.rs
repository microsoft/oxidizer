// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Minimal percent-decoding for path-variable and query-parameter values.
//!
//! Path variables and query parameters arrive percent-encoded on the wire (a
//! space as `%20`, an encoded slash as `%2F`, and — in a query string — a `+`
//! for a space). The reference `google.api.http` transcoders decode these
//! before binding them into the request message, so this module does the same.
//! Both helpers return [`Cow::Borrowed`] unchanged when there is nothing to
//! decode, keeping the common all-ASCII case allocation-free.

use std::borrow::Cow;

/// Decodes `%XX` escapes in a path-segment value.
///
/// A `+` is left as a literal `+`: path segments do not use the
/// `application/x-www-form-urlencoded` `+`-for-space convention.
pub(crate) fn decode_path(value: &str) -> Cow<'_, str> {
    decode(value, false)
}

/// Decodes a query-parameter value: `%XX` escapes plus `+` as a space (the
/// `application/x-www-form-urlencoded` convention query strings follow).
pub(crate) fn decode_query(value: &str) -> Cow<'_, str> {
    decode(value, true)
}

/// Returns `true` if `value` contains anything the corresponding decoder would
/// rewrite, so the caller can keep a zero-copy fast path when it would not.
pub(crate) fn needs_decoding(value: &str, plus_as_space: bool) -> bool {
    value.bytes().any(|b| b == b'%' || (plus_as_space && b == b'+'))
}

fn decode(value: &str, plus_as_space: bool) -> Cow<'_, str> {
    if !needs_decoding(value, plus_as_space) {
        return Cow::Borrowed(value);
    }

    let mut out = Vec::with_capacity(value.len());
    // Iterate by byte (not index) so there is no manual counter to advance — a
    // decoded `%XX` consumes the two lookahead bytes by adopting the advanced
    // iterator.
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        match byte {
            b'%' => {
                // A valid `%XX` triple decodes to one byte; a malformed or
                // truncated escape is emitted verbatim (the lookahead bytes stay
                // for the next iteration).
                let mut lookahead = bytes.clone();
                match (lookahead.next(), lookahead.next()) {
                    (Some(hi), Some(lo)) => match hex_pair(hi, lo) {
                        Some(decoded) => {
                            out.push(decoded);
                            bytes = lookahead;
                        }
                        None => out.push(b'%'),
                    },
                    _ => out.push(b'%'),
                }
            }
            b'+' if plus_as_space => out.push(b' '),
            other => out.push(other),
        }
    }

    // Percent-encoded bytes reassemble into UTF-8 (e.g. `%C3%A9` → `é`); an
    // invalid sequence degrades to the replacement character rather than
    // failing the whole request.
    match String::from_utf8(out) {
        Ok(text) => Cow::Owned(text),
        Err(error) => Cow::Owned(String::from_utf8_lossy(error.as_bytes()).into_owned()),
    }
}

/// Combines two hex digits into a byte, returning `None` if either is not a hex
/// digit. Uses `(hi << 4) + lo` (the nibbles never overlap) so the combination
/// is arithmetic that a test can distinguish, not an equivalent bitwise `|`.
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
        assert!(matches!(decode_path("plain-value"), Cow::Borrowed("plain-value")));
        assert!(matches!(decode_query("plain-value"), Cow::Borrowed("plain-value")));
        // A `+` is literal in a path, so the path decoder still borrows.
        assert!(matches!(decode_path("a+b"), Cow::Borrowed("a+b")));
    }

    #[test]
    fn decodes_percent_escapes() {
        assert_eq!(decode_path("a%20b"), "a b");
        assert_eq!(decode_path("shelves%2F7"), "shelves/7");
        assert_eq!(decode_query("science%20fiction"), "science fiction");
    }

    #[test]
    fn query_decodes_plus_as_space_but_path_does_not() {
        assert_eq!(decode_query("a+b"), "a b");
        assert_eq!(decode_path("a+b"), "a+b");
    }

    #[test]
    fn decodes_multibyte_utf8() {
        // `%C3%A9` is the UTF-8 encoding of `é`.
        assert_eq!(decode_path("caf%C3%A9"), "café");
    }

    #[test]
    fn decodes_lowercase_hex_digits() {
        // Lowercase `a`-`f` hex must decode the same as uppercase.
        assert_eq!(decode_path("%2f"), "/");
        assert_eq!(decode_path("caf%c3%a9"), "café");
    }

    #[test]
    fn path_keeps_a_literal_plus_even_when_decoding() {
        // A `%` forces the decoding loop; a literal `+` in a path must survive it
        // (only a query treats `+` as a space).
        assert_eq!(decode_path("a+%20b"), "a+ b");
        assert_eq!(decode_query("a+%20b"), "a  b");
    }

    #[test]
    fn leaves_malformed_escapes_verbatim() {
        assert_eq!(decode_path("100%"), "100%");
        assert_eq!(decode_path("50%xy"), "50%xy");
        assert_eq!(decode_path("%2"), "%2");
    }

    #[test]
    fn invalid_utf8_degrades_to_replacement() {
        // `%FF` is not valid UTF-8; it decodes to the replacement character
        // rather than failing.
        assert_eq!(decode_path("%FF"), "\u{FFFD}");
    }
}
