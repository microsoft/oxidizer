// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Safe accelerated primitives for built-in and custom header parsers.
//!
//! This module is a private implementation detail: every built-in header
//! parser validates through these functions, but they are not part of the
//! crate's public API.

/// Returns whether `bytes` is a non-empty RFC 9110 token.
#[cfg(any(
    test,
    feature = "headers-cache-control",
    feature = "headers-content-type",
    feature = "headers-negotiation",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-websocket",
))]
#[must_use]
#[inline]
pub(super) fn token(bytes: &[u8]) -> bool {
    http_headers_simd::is_token(bytes)
}

/// Returns whether `bytes` is an RFC 9110 `token68` value.
///
/// A value contains one or more alphanumeric or `-._~+/` bytes followed only
/// by optional `=` padding.
#[cfg(any(test, feature = "headers-authorization"))]
#[must_use]
#[inline]
pub(super) fn token68(bytes: &[u8]) -> bool {
    http_headers_simd::is_token68(bytes)
}

/// Returns whether one byte is permitted in an RFC 9110 token.
#[must_use]
#[inline]
pub(super) const fn token_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'!' | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'\''
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'^'
            | b'_'
            | b'`'
            | b'|'
            | b'~'
            | b'0'..=b'9'
            | b'A'..=b'Z'
            | b'a'..=b'z'
    )
}

/// Returns whether every byte is permitted in an HTTP field value.
///
/// Empty values are permitted. Carriage return, line feed, other controls,
/// and DEL are rejected.
#[must_use]
#[inline]
pub(super) fn field_value(bytes: &[u8]) -> bool {
    http_headers_simd::is_field_value(bytes)
}

/// Compares byte strings using ASCII case-insensitive equality.
///
/// Non-ASCII bytes compare exactly.
#[cfg(any(
    test,
    feature = "headers-content-type",
    feature = "headers-negotiation",
    feature = "headers-range",
    feature = "headers-security",
))]
#[must_use]
#[inline]
pub(super) fn eq_ignore_ascii_case(left: &[u8], right: &[u8]) -> bool {
    http_headers_simd::eq_ignore_ascii_case(left, right)
}

/// Finds the first comma, semicolon, quote, backslash, space, or tab.
#[must_use]
#[inline]
pub(super) fn find_interesting(bytes: &[u8]) -> Option<usize> {
    http_headers_simd::find_interesting(bytes)
}

/// Removes optional HTTP whitespace from both ends of a byte string.
#[must_use]
#[inline]
pub(super) fn trim_ows(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(|byte| matches!(byte, b' ' | b'\t')) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(|byte| matches!(byte, b' ' | b'\t')) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

/// Parses a nonempty unsigned decimal integer with checked arithmetic.
///
/// Returns `None` for an empty value, a non-digit, or arithmetic overflow.
#[cfg(any(
    test,
    feature = "headers-cache-control",
    feature = "headers-cors",
    feature = "headers-range",
    feature = "headers-security",
))]
#[must_use]
#[inline]
pub(super) fn decimal_u64(bytes: &[u8]) -> Option<u64> {
    // Nineteen digits is the widest decimal `u64` always holds, so a shorter
    // input needs no per-digit overflow check and the accumulation collapses
    // to a multiply-add.
    if bytes.is_empty() || bytes.len() > 19 {
        return wide_decimal_u64(bytes);
    }
    let mut value = 0_u64;
    for byte in bytes.iter().copied() {
        let digit = byte.wrapping_sub(b'0');
        if digit > 9 {
            return None;
        }
        value = value * 10 + u64::from(digit);
    }
    Some(value)
}

/// Parses the decimals long enough to need an overflow check on every digit.
#[cfg(any(
    test,
    feature = "headers-cache-control",
    feature = "headers-cors",
    feature = "headers-range",
    feature = "headers-security",
))]
#[cold]
#[inline(never)]
fn wide_decimal_u64(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() {
        return None;
    }
    bytes.iter().try_fold(0_u64, |value, byte| {
        value
            .checked_mul(10)?
            .checked_add(u64::from(byte.checked_sub(b'0').filter(|digit| *digit <= 9)?))
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn token_accepts_and_rejects() {
        assert!(token(b"gzip"));
        assert!(!token(b"not a token"));
    }

    #[test]
    fn token68_accepts_and_rejects() {
        assert!(token68(b"YWxpY2U6c2VjcmV0=="));
        assert!(!token68(b"data=more"));
    }

    #[test]
    fn token_byte_accepts_and_rejects() {
        assert!(token_byte(b'a'));
        assert!(!token_byte(b' '));
    }

    #[test]
    fn token_byte_agrees_with_token_for_every_byte() {
        for byte in u8::MIN..=u8::MAX {
            assert_eq!(token_byte(byte), token(&[byte]));
        }
    }

    #[test]
    fn field_value_accepts_and_rejects() {
        assert!(field_value(b"text/plain"));
        assert!(!field_value(b"line\r\nbreak"));
    }

    #[test]
    fn eq_ignore_ascii_case_compares() {
        assert!(eq_ignore_ascii_case(b"gzip", b"GZIP"));
        assert!(!eq_ignore_ascii_case(b"gzip", b"br"));
    }

    #[test]
    fn find_interesting_locates_delimiters() {
        assert_eq!(find_interesting(b"token,next"), Some(5));
        assert_eq!(find_interesting(b"token"), None);
    }

    #[test]
    fn trim_ows_strips_whitespace() {
        assert_eq!(trim_ows(b" \tvalue\t "), b"value");
        assert_eq!(trim_ows(b"\rvalue\n"), b"\rvalue\n");
        assert_eq!(trim_ows(b"\t \t"), b"");
    }

    #[test]
    fn decimal_u64_parses_and_rejects() {
        assert_eq!(decimal_u64(b"3600"), Some(3600));
        assert_eq!(decimal_u64(b"-1"), None);
        assert_eq!(decimal_u64(b"0"), Some(0));
        assert_eq!(decimal_u64(b"00123"), Some(123));
        assert_eq!(decimal_u64(u64::MAX.to_string().as_bytes()), Some(u64::MAX));
        assert_eq!(decimal_u64(b"12x"), None);
        assert_eq!(decimal_u64(b"18446744073709551616"), None);
    }
}
