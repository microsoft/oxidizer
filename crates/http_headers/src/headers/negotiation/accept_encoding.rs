// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::shared::{ListValues, check_quoted_value, check_quoted_values, validate_weighted_token};
use crate::sink::{FieldSink, InsertError};
use crate::source::{FieldLines, FieldSource};
use crate::{DecodeError, Field, FieldName, FieldValue, FieldValueRef, validate};

/// Owned value for the `Accept-Encoding` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 12.5.3].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::AcceptEncodingOwned::try_from("gzip, br")?;
/// assert_eq!(value.items().count(), 2);
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Accept-Encoding: gzip, deflate, br` lists codings.
/// `Accept-Encoding: gzip;q=1.0, *;q=0` adds quality and wildcard preferences.
///
/// # Relaxed decoding
///
/// [`DecodeMode::Relaxed`](crate::DecodeMode::Relaxed) permits spaces or tabs around the quality
/// parameter's `=`, leading-dot quality values, and more than three
/// fractional digits. Content-coding tokens, wildcard syntax, parameter
/// count, quoting, and list structure remain strict, and the original bytes
/// are preserved.
///
/// [RFC 9110 section 12.5.3]: https://www.rfc-editor.org/rfc/rfc9110#section-12.5.3
pub struct AcceptEncodingOwned {
    values: ListValues,
}

/// Borrowed value for the `Accept-Encoding` header.
/// # Examples
///
/// ```rust
/// use http_headers::headers::{AcceptEncoding, AcceptEncodingView};
/// use http_headers::source::{FieldLines, FieldSource};
/// use http_headers::{Field, FieldName};
///
/// struct Source;
///
/// impl FieldSource for Source {
///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
///         (name == &FieldName::AcceptEncoding)
///             .then(|| FieldLines::single(name, b"gzip, br;q=0.8"))
///     }
/// }
///
/// let value: AcceptEncodingView<'_> = AcceptEncoding::view(&Source)?.expect("header is present");
/// let items = value.items().collect::<Vec<_>>();
/// assert_eq!(items, [&b"gzip"[..], &b"br;q=0.8"[..]]);
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct AcceptEncodingView<'a> {
    values: FieldLines<'a>,
}

super::shared::list_header!(
    AcceptEncoding,
    AcceptEncodingOwned,
    AcceptEncodingView,
    "Accept-Encoding",
    "Defined by [RFC 9110 section 12.5.3](https://www.rfc-editor.org/rfc/rfc9110#section-12.5.3).",
    &FieldName::AcceptEncoding,
    validate_accept_encoding,
    validate_accept_encoding_relaxed,
    check_quoted_values,
    check_quoted_value,
    quoted
);

fn validate_accept_encoding(bytes: &[u8]) -> Result<(), DecodeError> {
    validate_weighted_token(bytes, &FieldName::AcceptEncoding, validate::token, false)
}

fn validate_accept_encoding_relaxed(bytes: &[u8]) -> Result<(), DecodeError> {
    validate_weighted_token(bytes, &FieldName::AcceptEncoding, validate::token, true)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::super::shared::{WELL_KNOWN_ACCEPT_ENCODING, try_plain_items};
    use super::super::weighted_token_scan::corpus::{exhaustive, fragment_lines};
    use super::super::weighted_token_scan::scan_accept_encoding_line;
    use super::{validate_accept_encoding, validate_accept_encoding_relaxed};
    use crate::DecodeErrorKind;

    #[test]
    fn strict_and_relaxed_encoding_weights_use_the_shared_grammar() {
        validate_accept_encoding(b"gzip;q=0.5").expect("strict encoding weight");
        validate_accept_encoding_relaxed(b"br; q = .1234").expect("relaxed encoding weight");
        assert_eq!(
            validate_accept_encoding(b"bad encoding")
                .expect_err("encoding must be a token")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
    }

    #[test]
    fn every_well_known_line_also_satisfies_the_member_grammar() {
        let rejected: Vec<_> = WELL_KNOWN_ACCEPT_ENCODING
            .iter()
            .filter(|line| validate_accept_encoding(line).is_err())
            .map(|line| String::from_utf8_lossy(line).into_owned())
            .collect();
        assert!(
            rejected.is_empty(),
            "recognized lines must satisfy the grammar they skip: {rejected:?}"
        );
    }

    fn general_parser_accepts(bytes: &[u8], relaxed: bool) -> bool {
        let validator = if relaxed {
            validate_accept_encoding_relaxed
        } else {
            validate_accept_encoding
        };
        matches!(try_plain_items(bytes, b',', true, validator), Ok(true))
    }

    fn assert_recognition_is_sound(lines: &[Vec<u8>]) -> usize {
        let mut recognized = 0;
        for line in lines {
            if !scan_accept_encoding_line(line) {
                continue;
            }
            recognized += 1;
            let shown = String::from_utf8_lossy(line);
            assert!(
                general_parser_accepts(line, false),
                "recognized line {shown:?} must satisfy the strict member grammar"
            );
            assert!(
                general_parser_accepts(line, true),
                "recognized line {shown:?} must satisfy the relaxed member grammar"
            );
        }
        recognized
    }

    #[test]
    fn recognized_short_lines_satisfy_the_member_grammar() {
        let recognized = assert_recognition_is_sound(&exhaustive(b"a*;,=qQ01. ", 4));
        assert!(
            recognized > 100,
            "the corpus must actually exercise the recognizer, saw {recognized}"
        );
    }

    #[test]
    fn recognized_fragment_lines_satisfy_the_member_grammar() {
        const FRAGMENTS: &[&str] = &[
            "gzip",
            "br",
            "*",
            ";q=0.9",
            ";q=1",
            ";q=0",
            ";q=",
            ";q=1.5",
            ";q=0.1234",
            ";q=1.000",
            ";q=1.001",
            ";Q=0.123",
            ";x=y",
            ";q=0.5;q=0.5",
            "; q=0.9",
            " ;q=0.9",
            ";\tq=0.9",
            ";q=0.9 ",
            "; ",
            " , ",
            ",",
            " ",
            "\t",
            ";",
            "=",
            "\"",
            "\\",
            "/",
            "a",
            "0.",
        ];

        let recognized = assert_recognition_is_sound(&fragment_lines(FRAGMENTS, 3));
        assert!(
            recognized > 100,
            "the corpus must actually exercise the recognizer, saw {recognized}"
        );
    }

    #[test]
    fn realistic_lines_are_recognized() {
        const LINES: &[&str] = &[
            "*",
            "gzip",
            "gzip, deflate",
            "gzip, deflate, br",
            "gzip, deflate, br, zstd",
            "gzip, deflate, br, zstd;q=0.9",
            "gzip; q=0.9, deflate ;q=0.8",
            "gzip;q=1.0, identity;q=0.5, *;q=0",
            "br;q=1.0, gzip;q=0.8, *;q=0.1",
        ];

        for line in LINES {
            assert!(
                scan_accept_encoding_line(line.as_bytes()),
                "the recognizer must settle the common line {line:?}"
            );
        }
    }
}
