// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::shared::{ListValues, check_quoted_value, check_quoted_values, validate_weighted_token};
use crate::sink::{FieldSink, InsertError};
use crate::source::{FieldLines, FieldSource};
use crate::{DecodeError, Field, FieldName, FieldValue, FieldValueRef};

/// Owned value for the `Accept-Language` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 12.5.4].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::AcceptLanguageOwned::try_from("en, fr;q=0.7")?;
/// assert_eq!(value.items().count(), 2);
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Accept-Language: en-US, en;q=0.9, fr;q=0.7` expresses ordered preferences;
/// `Accept-Language: *` accepts any language.
///
/// # Relaxed decoding
///
/// [`DecodeMode::Relaxed`](crate::DecodeMode::Relaxed) permits spaces or tabs around the quality
/// parameter's `=`, leading-dot quality values, and more than three
/// fractional digits. Language-range syntax, wildcard syntax, parameter
/// count, quoting, and list structure remain strict, and the original bytes
/// are preserved.
///
/// [RFC 9110 section 12.5.4]: https://www.rfc-editor.org/rfc/rfc9110#section-12.5.4
pub struct AcceptLanguageOwned {
    values: ListValues,
}

/// Borrowed value for the `Accept-Language` header.
/// # Examples
///
/// ```rust
/// use http_headers::headers::{AcceptLanguage, AcceptLanguageView};
/// use http_headers::source::{FieldLines, FieldSource};
/// use http_headers::{Field, FieldName};
///
/// struct Source;
///
/// impl FieldSource for Source {
///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
///         (name == &FieldName::AcceptLanguage)
///             .then(|| FieldLines::single(name, b"en-US, fr;q=0.7"))
///     }
/// }
///
/// let value: AcceptLanguageView<'_> = AcceptLanguage::view(&Source)?.expect("header is present");
/// let items = value.items().collect::<Vec<_>>();
/// assert_eq!(items, [&b"en-US"[..], &b"fr;q=0.7"[..]]);
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct AcceptLanguageView<'a> {
    values: FieldLines<'a>,
}

super::shared::list_header!(
    AcceptLanguage,
    AcceptLanguageOwned,
    AcceptLanguageView,
    "Accept-Language",
    "Defined by [RFC 9110 section 12.5.4](https://www.rfc-editor.org/rfc/rfc9110#section-12.5.4).",
    &FieldName::AcceptLanguage,
    validate_accept_language,
    validate_accept_language_relaxed,
    check_quoted_values,
    check_quoted_value,
    quoted
);

fn validate_accept_language(bytes: &[u8]) -> Result<(), DecodeError> {
    validate_weighted_token(bytes, &FieldName::AcceptLanguage, valid_language_range, false)
}

fn validate_accept_language_relaxed(bytes: &[u8]) -> Result<(), DecodeError> {
    validate_weighted_token(bytes, &FieldName::AcceptLanguage, valid_language_range, true)
}

fn valid_language_range(bytes: &[u8]) -> bool {
    if bytes == b"*" {
        return true;
    }
    let mut subtags = bytes.split(|byte| *byte == b'-');
    let primary = subtags.next().expect("slice splitting yields a first subtag");
    (1..=8).contains(&primary.len())
        && primary.iter().all(u8::is_ascii_alphabetic)
        && subtags.all(|subtag| (1..=8).contains(&subtag.len()) && subtag.iter().all(u8::is_ascii_alphanumeric))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::super::shared::{WELL_KNOWN_ACCEPT_LANGUAGE, try_plain_items};
    use super::super::weighted_token_scan::corpus::{exhaustive, fragment_lines};
    use super::super::weighted_token_scan::scan_accept_language_line;
    use super::{valid_language_range, validate_accept_language, validate_accept_language_relaxed};
    use crate::DecodeErrorKind;

    #[test]
    fn language_ranges_enforce_primary_and_subtag_boundaries() {
        validate_accept_language(b"en-US;q=0.8").expect("strict language weight");
        validate_accept_language_relaxed(b"fr; q = .1234").expect("relaxed language weight");
        assert_eq!(
            validate_accept_language(b"en_US")
                .expect_err("underscore is not a language-range separator")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
        for valid in [b"*".as_slice(), b"en", b"en-US", b"abcdefgh-12345678"] {
            assert!(valid_language_range(valid), "{valid:?}");
        }
        for invalid in [b"".as_slice(), b"-en", b"abcdefghi", b"en-", b"en-123456789", b"en-US!"] {
            assert!(!valid_language_range(invalid), "{invalid:?}");
        }
    }

    #[test]
    fn every_well_known_line_also_satisfies_the_member_grammar() {
        let rejected: Vec<_> = WELL_KNOWN_ACCEPT_LANGUAGE
            .iter()
            .filter(|line| validate_accept_language(line).is_err())
            .map(|line| String::from_utf8_lossy(line).into_owned())
            .collect();
        assert!(
            rejected.is_empty(),
            "recognized lines must satisfy the grammar they skip: {rejected:?}"
        );
    }

    fn general_parser_accepts(bytes: &[u8], relaxed: bool) -> bool {
        let validator = if relaxed {
            validate_accept_language_relaxed
        } else {
            validate_accept_language
        };
        matches!(try_plain_items(bytes, b',', true, validator), Ok(true))
    }

    fn assert_recognition_is_sound(lines: &[Vec<u8>]) -> usize {
        let mut recognized = 0;
        for line in lines {
            if !scan_accept_language_line(line) {
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
        let recognized = assert_recognition_is_sound(&exhaustive(b"a-*;,=qQ01. ", 4));
        assert!(
            recognized > 100,
            "the corpus must actually exercise the recognizer, saw {recognized}"
        );
    }

    #[test]
    fn recognized_fragment_lines_satisfy_the_member_grammar() {
        const FRAGMENTS: &[&str] = &[
            "en",
            "en-US",
            "*",
            "en-",
            "-en",
            "en_US",
            "abcdefghi",
            "en-abcdefghi",
            ";q=0.9",
            ";q=1",
            ";q=0",
            ";q=",
            ";q=1.5",
            ";q=0.1234",
            ";q=1.000",
            ";Q=0.123",
            ";x=y",
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
            "en",
            "en-US",
            "en-US,en;q=0.9",
            "en-US,en;q=0.9,fr;q=0.8",
            "en-US,en;q=0.9,fr-FR;q=0.8,fr;q=0.7",
            "fr-CH, fr;q=0.9, en;q=0.8, de;q=0.7, *;q=0.5",
            "zh-Hans-CN,zh-Hans;q=0.9,en-US;q=0.8,en;q=0.7",
        ];

        for line in LINES {
            assert!(
                scan_accept_language_line(line.as_bytes()),
                "the recognizer must settle the common line {line:?}"
            );
        }
    }
}
