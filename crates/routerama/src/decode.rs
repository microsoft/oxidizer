// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;

/// Percent-decodes a raw path capture, using *path* semantics.
///
/// - Borrows the input unchanged when it contains no `%`.
/// - `+` is treated as a literal `+` (path semantics), **not** as a space; that
///   is a form-encoding rule and does not apply to URI path components.
/// - Returns [`None`] on a malformed escape (a `%` not followed by two hex
///   digits) or if the decoded bytes are not valid UTF-8.
pub(crate) fn decode(input: &str) -> Option<Cow<'_, str>> {
    let Some(first) = input.find('%') else {
        return Some(Cow::Borrowed(input));
    };

    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..first]);

    let mut rest = &bytes[first..];
    while let Some((&byte, tail)) = rest.split_first() {
        if byte == b'%' {
            let hi = hex_value(*tail.first()?)?;
            let lo = hex_value(*tail.get(1)?)?;
            out.push(hi * 16 + lo);
            rest = &tail[2..];
        } else {
            out.push(byte);
            rest = tail;
        }
    }

    String::from_utf8(out).ok().map(Cow::Owned)
}

/// Maps an ASCII hex digit to its value, or [`None`] if it is not a hex digit.
fn hex_value(byte: u8) -> Option<u8> {
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
    fn borrows_when_no_escape() {
        assert!(matches!(decode("hello"), Some(Cow::Borrowed("hello"))));
    }

    #[test]
    fn decodes_space_and_slash() {
        assert_eq!(decode("hello%20world").as_deref(), Some("hello world"));
        assert_eq!(decode("a%2Fb").as_deref(), Some("a/b"));
    }

    #[test]
    fn decodes_lowercase_hex_digits() {
        assert_eq!(decode("a%2fb").as_deref(), Some("a/b"));
        assert_eq!(decode("%c3%a9").as_deref(), Some("é"));
    }

    #[test]
    fn plus_is_literal() {
        assert!(matches!(decode("a+b"), Some(Cow::Borrowed("a+b"))));
    }

    #[test]
    fn decodes_multibyte_utf8() {
        assert_eq!(decode("%E2%9C%93").as_deref(), Some("\u{2713}"));
    }

    #[test]
    fn rejects_malformed_escapes() {
        assert_eq!(decode("%zz"), None);
        assert_eq!(decode("%2"), None);
        assert_eq!(decode("trailing%"), None);
    }

    #[test]
    fn rejects_invalid_utf8() {
        assert_eq!(decode("%FF"), None);
    }
}
