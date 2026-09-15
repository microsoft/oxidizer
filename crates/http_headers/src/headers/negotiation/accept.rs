// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::shared::{
    ListValues, QuotedItems, check_quoted_value, check_quoted_values, invalid, invalid_syntax, parse_parameter, try_plain_items,
    validate_quality,
};
use crate::sink::{FieldSink, InsertError};
use crate::source::{FieldLines, FieldSource};
use crate::{DecodeError, DecodeErrorKind, Field, FieldName, FieldValue, FieldValueRef, validate};

/// Owned value for the `Accept` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 12.5.1].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::AcceptOwned::try_from("text/html, application/json")?;
/// assert_eq!(value.items().count(), 2);
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Accept: text/html, application/xhtml+xml, */*;q=0.9` combines exact and
/// wildcard media ranges with a quality weight.
///
/// # Relaxed decoding
///
/// [`DecodeMode::Relaxed`](crate::DecodeMode::Relaxed) permits spaces or tabs around the quality
/// parameter's `=`, leading-dot quality values, and more than three
/// fractional digits. All media-range, wildcard, parameter-ordering, quoting,
/// and list rules remain strict, and the original bytes are preserved.
///
/// [RFC 9110 section 12.5.1]: https://www.rfc-editor.org/rfc/rfc9110#section-12.5.1
pub struct AcceptOwned {
    values: ListValues,
}

/// Borrowed value for the `Accept` header.
/// # Examples
///
/// ```rust
/// use http_headers::headers::{Accept, AcceptView};
/// use http_headers::source::{FieldLines, FieldSource};
/// use http_headers::{Field, FieldName};
///
/// struct Source;
///
/// impl FieldSource for Source {
///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
///         (name == &FieldName::Accept)
///             .then(|| FieldLines::single(name, b"text/html;q=0.8, application/json"))
///     }
/// }
///
/// let value: AcceptView<'_> = Accept::view(&Source)?.expect("header is present");
/// let items = value.items().collect::<Vec<_>>();
/// assert_eq!(items, [&b"text/html;q=0.8"[..], &b"application/json"[..]]);
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct AcceptView<'a> {
    values: FieldLines<'a>,
}

super::shared::list_header!(
    Accept,
    AcceptOwned,
    AcceptView,
    "Accept",
    "Defined by [RFC 9110 section 12.5.1](https://www.rfc-editor.org/rfc/rfc9110#section-12.5.1).",
    &FieldName::Accept,
    validate_accept,
    validate_accept_relaxed,
    check_quoted_values,
    check_quoted_value,
    quoted
);

fn validate_accept(bytes: &[u8]) -> Result<(), DecodeError> {
    validate_accept_with(bytes, false)
}

fn validate_accept_relaxed(bytes: &[u8]) -> Result<(), DecodeError> {
    validate_accept_with(bytes, true)
}

fn validate_accept_with(bytes: &[u8], relaxed: bool) -> Result<(), DecodeError> {
    if validate_accept_plain(bytes, relaxed)? {
        return Ok(());
    }
    validate_accept_quoted(bytes, relaxed)
}

fn validate_accept_plain(bytes: &[u8], relaxed: bool) -> Result<bool, DecodeError> {
    let mut first = true;
    let mut quality_seen = false;
    try_plain_items(bytes, b';', false, |segment| {
        if first {
            first = false;
            return validate_media_range(segment);
        }
        validate_accept_parameter(segment, &mut quality_seen, relaxed)
    })
}

fn validate_accept_quoted(bytes: &[u8], relaxed: bool) -> Result<(), DecodeError> {
    let mut parameters = QuotedItems::semicolon(bytes, &FieldName::Accept);
    let media_range = parameters.next().expect("semicolon iteration always yields a first item")?;
    validate_media_range(media_range)?;
    let mut quality_seen = false;
    for parameter in parameters {
        let (name, value, compact) = parse_parameter(parameter?, quality_seen, &FieldName::Accept)?;
        if validate::eq_ignore_ascii_case(name, b"q") {
            if quality_seen {
                return Err(invalid_syntax(&FieldName::Accept));
            }
            if !relaxed && !compact {
                return Err(invalid_syntax(&FieldName::Accept));
            }
            let quality = value.expect("quality parameters require a value");
            validate_quality(quality, &FieldName::Accept, relaxed)?;
            quality_seen = true;
        } else if !compact {
            return Err(invalid_syntax(&FieldName::Accept));
        }
    }
    Ok(())
}

fn validate_accept_parameter(bytes: &[u8], quality_seen: &mut bool, relaxed: bool) -> Result<(), DecodeError> {
    if !*quality_seen && bytes.len() >= 2 && bytes[0].eq_ignore_ascii_case(&b'q') && bytes[1] == b'=' {
        validate_quality(&bytes[2..], &FieldName::Accept, relaxed)?;
        *quality_seen = true;
        return Ok(());
    }
    let (name, value, compact) = parse_parameter(bytes, *quality_seen, &FieldName::Accept)?;
    if validate::eq_ignore_ascii_case(name, b"q") {
        if *quality_seen || (!relaxed && !compact) {
            return Err(invalid_syntax(&FieldName::Accept));
        }
        let quality = value.expect("quality parameters require a value");
        validate_quality(quality, &FieldName::Accept, relaxed)?;
        *quality_seen = true;
    } else if !compact {
        return Err(invalid_syntax(&FieldName::Accept));
    }
    Ok(())
}

/// Validates one media range.
///
/// The range is split at its first slash and each half is checked as a token.
/// A slash is not a token byte, so a second one already fails the subtype
/// check, and the scan that tells a repeated slash apart from an ordinary bad
/// byte runs only when the range is being rejected anyway.
fn validate_media_range(bytes: &[u8]) -> Result<(), DecodeError> {
    let mut halves = bytes.splitn(2, |byte| *byte == b'/');
    let type_ = halves.next().expect("splitting always yields a first half");
    let Some(subtype) = halves.next() else {
        return Err(invalid_syntax(&FieldName::Accept));
    };

    if !validate::token(type_) || !validate::token(subtype) {
        if subtype.contains(&b'/') {
            return Err(invalid_syntax(&FieldName::Accept));
        }
        return Err(invalid(&FieldName::Accept, DecodeErrorKind::InvalidToken));
    }

    if type_ == b"*" && subtype != b"*" {
        return Err(invalid_syntax(&FieldName::Accept));
    }

    Ok(())
}

#[cfg(test)]
#[expect(
    clippy::assertions_on_result_states,
    reason = "the tests classify many parser outcomes without needing their success values"
)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::super::accept_scan::scan_accept_line;
    use super::super::shared::{WELL_KNOWN_ACCEPT, invalid, invalid_syntax, try_plain_items};
    use super::{
        validate_accept, validate_accept_parameter, validate_accept_quoted, validate_accept_relaxed, validate_accept_with,
        validate_media_range,
    };
    use crate::{DecodeError, DecodeErrorKind, FieldName, validate};

    #[test]
    fn quoted_parameters_enforce_media_range_and_quality_rules() {
        validate_accept(b"text/plain").expect("strict plain media range");
        validate_accept_relaxed(b"text/plain; q = .5").expect("relaxed quality");
        validate_accept_with(b"text/plain;level=\"one\"", false).expect("quoted extension parameter");
        assert!(validate_accept_quoted(b"text/plain;level=\"one\";q=0.5", false).is_ok());
        assert_eq!(
            validate_accept_quoted(b"\"unterminated", false)
                .expect_err("unterminated first item")
                .kind(),
            DecodeErrorKind::UnterminatedQuote
        );
        assert_eq!(
            validate_accept_quoted(b"", false).expect_err("missing media range").kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            validate_accept_quoted(b"text/plain;level=\"unterminated", false)
                .expect_err("unterminated parameter")
                .kind(),
            DecodeErrorKind::UnterminatedQuote
        );
        assert_eq!(
            validate_accept_quoted(b"text/plain;q=0.5;Q=0.4", false)
                .expect_err("duplicate quality")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            validate_accept_quoted(b"text/plain;q = 0.5", false)
                .expect_err("strict whitespace")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert!(validate_accept_quoted(b"text/plain;charset = \"utf-8\"", false).is_err());
        assert!(validate_accept_quoted(b"text/plain;charset = \"utf-8\"", true).is_err());
    }

    #[test]
    fn plain_parameters_and_media_ranges_reject_structural_errors() {
        let mut quality_seen = false;
        assert!(validate_accept_parameter(b"level=one", &mut quality_seen, false).is_ok());
        assert!(!quality_seen);
        assert!(validate_accept_parameter(b"q=0.8", &mut quality_seen, false).is_ok());
        assert!(quality_seen);
        assert_eq!(
            validate_accept_parameter(b"q=0.7", &mut quality_seen, false)
                .expect_err("duplicate quality")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let mut quality_seen = false;
        assert_eq!(
            validate_accept_parameter(b"q = 0.8", &mut quality_seen, false)
                .expect_err("strict whitespace")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert!(validate_accept_parameter(b"charset = utf-8", &mut quality_seen, false).is_err());
        assert!(validate_accept_parameter(b"charset = utf-8", &mut quality_seen, true).is_err());
        assert!(validate_accept_parameter(b"q = .8", &mut quality_seen, true).is_ok());

        for malformed in [b"text".as_slice(), b"text/plain/more", b"te xt/plain", b"*/plain"] {
            assert!(validate_media_range(malformed).is_err(), "{malformed:?}");
        }
        assert!(validate_media_range(b"*/*").is_ok());
        assert!(validate_media_range(b"text/*").is_ok());
    }

    /// Validates a whole line the way the general parser does, without the
    /// whole-line recognizer in front of it.
    fn general_parser_accepts(bytes: &[u8], relaxed: bool) -> bool {
        let validator = if relaxed { validate_accept_relaxed } else { validate_accept };
        matches!(try_plain_items(bytes, b',', true, validator), Ok(true))
    }

    /// Builds every string up to `length` bytes over `alphabet`.
    fn exhaustive(alphabet: &[u8], length: usize) -> Vec<Vec<u8>> {
        let mut all = vec![Vec::new()];
        let mut frontier = vec![Vec::new()];
        for _ in 0..length {
            let mut next = Vec::new();
            for prefix in &frontier {
                for byte in alphabet {
                    let mut candidate = prefix.clone();
                    candidate.push(*byte);
                    next.push(candidate);
                }
            }
            all.extend_from_slice(&next);
            frontier = next;
        }
        all
    }

    /// Builds every concatenation of up to `count` grammar fragments.
    fn fragment_lines(count: usize) -> Vec<Vec<u8>> {
        const FRAGMENTS: &[&str] = &[
            "a/b",
            "*/*",
            "a/*",
            "*/a",
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
            ";x",
            "; q=0.9",
            " ;q=0.9",
            ";\tq=0.9",
            ";q=0.9 ",
            "; ",
            " , ",
            ",",
            " ",
            "\t",
            "/",
            ";",
            "=",
            "\"",
            "\\",
            "a",
            "0.",
            ";q=0.",
        ];

        let mut lines = vec![Vec::new()];
        for _ in 0..count {
            let mut next = Vec::new();
            for prefix in &lines {
                for fragment in FRAGMENTS {
                    let mut candidate = prefix.clone();
                    candidate.extend_from_slice(fragment.as_bytes());
                    next.push(candidate);
                }
            }
            lines.extend_from_slice(&next);
        }
        lines
    }

    fn assert_recognition_is_sound(lines: &[Vec<u8>]) -> usize {
        let mut recognized = 0;
        for line in lines {
            if !scan_accept_line(line) {
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
        let recognized = assert_recognition_is_sound(&exhaustive(b"a*/;,=qQ01. ", 4));
        assert!(
            recognized > 100,
            "the corpus must actually exercise the recognizer, saw {recognized}"
        );
    }

    #[test]
    fn recognized_fragment_lines_satisfy_the_member_grammar() {
        let recognized = assert_recognition_is_sound(&fragment_lines(3));
        assert!(
            recognized > 100,
            "the corpus must actually exercise the recognizer, saw {recognized}"
        );
    }

    #[test]
    fn realistic_lines_are_recognized() {
        const LINES: &[&str] = &[
            "*/*",
            "text/html",
            "application/json",
            "text/html, application/json",
            "text/html;q=0.9",
            "text/html, */*;q=0.8",
            "application/json, text/plain;q=0.9, */*;q=0.1",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
        ];

        for line in LINES {
            assert!(
                scan_accept_line(line.as_bytes()),
                "the recognizer must settle the common line {line:?}"
            );
        }
    }

    /// Validates a media range the way the original rule-at-a-time walk did.
    fn media_range_reference(bytes: &[u8]) -> Result<(), DecodeError> {
        let Some(slash) = bytes.iter().position(|byte| *byte == b'/') else {
            return Err(invalid_syntax(&FieldName::Accept));
        };
        if bytes[slash + 1..].contains(&b'/') {
            return Err(invalid_syntax(&FieldName::Accept));
        }
        let type_ = &bytes[..slash];
        let subtype = &bytes[slash + 1..];
        if !validate::token(type_) || !validate::token(subtype) {
            return Err(invalid(&FieldName::Accept, DecodeErrorKind::InvalidToken));
        }
        if type_ == b"*" && subtype != b"*" {
            return Err(invalid_syntax(&FieldName::Accept));
        }
        Ok(())
    }

    fn media_ranges() -> Vec<Vec<u8>> {
        let mut ranges: Vec<Vec<u8>> = [
            "",
            "/",
            "//",
            "*/*",
            "*/plain",
            "text/*",
            "text",
            "text/",
            "/plain",
            "text/plain",
            "text/plain/extra",
            "a/b/c",
            "*",
            "**/*",
            "*/**",
            "text//plain",
            " text/plain",
            "text/plain ",
            "text /plain",
            "text/ plain",
        ]
        .iter()
        .map(|range| range.as_bytes().to_vec())
        .collect();

        for byte in 0_u16..=255 {
            let byte = u8::try_from(byte).unwrap_or(0);
            ranges.push(vec![byte]);
            ranges.push(vec![b't', byte, b'/', b'p']);
            ranges.push(vec![b't', b'/', byte, b'p']);
            ranges.push(vec![byte, b'/', b'*']);
            ranges.push(vec![b'*', b'/', byte]);
        }
        ranges
    }

    #[test]
    fn single_pass_media_range_matches_the_rule_at_a_time_walk() {
        for range in media_ranges() {
            assert_eq!(
                format!("{:?}", validate_media_range(&range)),
                format!("{:?}", media_range_reference(&range)),
                "media range {:?} must validate identically",
                String::from_utf8_lossy(&range)
            );
        }
    }

    #[test]
    fn every_well_known_line_also_satisfies_the_member_grammar() {
        let rejected: Vec<_> = WELL_KNOWN_ACCEPT
            .iter()
            .filter(|line| validate_accept(line).is_err())
            .map(|line| String::from_utf8_lossy(line).into_owned())
            .collect();
        assert!(
            rejected.is_empty(),
            "recognized lines must satisfy the grammar they skip: {rejected:?}"
        );
    }
}
