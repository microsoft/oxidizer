// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Retained negotiation views must not re-enter a stateful field source.

#![cfg(feature = "headers-negotiation")]
#![expect(clippy::unwrap_used, reason = "test failures provide sufficient context")]

use std::cell::Cell;
use std::iter;

use http_headers::headers::{Accept, AcceptEncoding, AcceptLanguage, QualityView};
use http_headers::source::{FieldLines, FieldSource, MAX_CUSTOM_FIELD_BYTES, MAX_CUSTOM_FIELD_LINES, MAX_CUSTOM_LIST_ITEMS};
use http_headers::{DecodeError, DecodeErrorKind, DecodeMode, Field, FieldName, FieldSensitivity, FieldValueRef};

struct SwitchingSource {
    name: &'static FieldName,
    lines: [FieldValueRef<'static>; 2],
    changed: Cell<bool>,
    calls: Cell<usize>,
}

impl FieldSource for SwitchingSource {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        assert_eq!(name, self.name);
        self.calls.set(self.calls.get() + 1);
        if self.changed.get() {
            Some(FieldLines::single(name, b"\r"))
        } else {
            FieldLines::from_borrowed(name, &self.lines)
        }
    }
}

struct BorrowedSource<'a>(&'a [FieldValueRef<'a>]);

impl FieldSource for BorrowedSource<'_> {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        FieldLines::from_borrowed(name, self.0)
    }
}

#[test]
fn retained_views_do_not_reenter_stateful_custom_sources() {
    macro_rules! retained {
        ($header:ty, $first:expr, $strict:expr, $relaxed:expr) => {
            for (mode, second, quality_wire) in [
                (DecodeMode::Strict, $strict.as_slice(), b"0.125".as_slice()),
                (
                    DecodeMode::Relaxed,
                    $relaxed.as_slice(),
                    b".12500000000000000000001".as_slice(),
                ),
            ] {
                let source = SwitchingSource {
                    name: <$header>::name(),
                    lines: [
                        FieldValueRef::new($first).with_sensitivity(FieldSensitivity::Sensitive),
                        FieldValueRef::new(second),
                    ],
                    changed: Cell::new(false),
                    calls: Cell::new(0),
                };
                let checked = FieldLines::from_borrowed(source.name, &source.lines).unwrap();
                let expected_members = checked.comma_items().collect::<Result<Vec<_>, _>>().unwrap();
                let view = <$header>::view_with(&source, mode).unwrap().unwrap();
                assert_eq!(source.calls.get(), 1);
                source.changed.set(true);

                let quality = QualityView::parse(quality_wire, mode).unwrap();
                let expected = [(QualityView::ONE, None), (QualityView::ONE, None), (quality, Some(quality))];
                for _ in 0..8 {
                    assert_eq!(
                        view.entries()
                            .map(|entry| (entry.quality(), entry.explicit_quality()))
                            .collect::<Vec<_>>(),
                        expected
                    );
                    assert_eq!(view.items().collect::<Vec<_>>(), expected_members);
                    assert!(view.values().next().unwrap().is_sensitive());
                    assert_eq!(source.calls.get(), 1);
                }
                let error = <$header>::view_with(&source, mode).unwrap_err();
                assert_eq!(error, DecodeError::new(source.name, DecodeErrorKind::InvalidSyntax));
                assert_eq!(source.calls.get(), 2);
                assert_eq!(view.entries().count(), 3);
                assert_eq!(source.calls.get(), 2);
            }
        };
    }

    retained!(
        Accept,
        b"text/plain, , text/plain,",
        b"text/html;p=\"a,b;\\\"\xff\";q=0.125;flag",
        b"text/html;p=\"a,b;\\\"\xff\"; q = .12500000000000000000001 ;flag"
    );
    retained!(
        AcceptEncoding,
        b"GZIP, , GZIP,",
        b"identity;q=0.125",
        b"identity; q = .12500000000000000000001"
    );
    retained!(AcceptLanguage, b"en-US, , en-US,", b"*;q=0.125", b"*; q = .12500000000000000000001");
}

fn assert_error<F: Field>(lines: &[FieldValueRef<'_>], expected: DecodeErrorKind) {
    let source = BorrowedSource(lines);
    for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
        let expected = DecodeError::new(F::name(), expected);
        assert_eq!(F::view_with(&source, mode).err().unwrap(), expected);
        assert_eq!(F::owned_with(&source, mode).err().unwrap(), expected);
    }
}

fn assert_precedence<F: Field>(good: &str, invalid_token: &[u8]) {
    assert_error::<F>(&[FieldValueRef::new(invalid_token)], DecodeErrorKind::InvalidToken);
    assert_error::<F>(
        &[FieldValueRef::new(invalid_token), FieldValueRef::new(b"\r")],
        DecodeErrorKind::InvalidSyntax,
    );
    assert_error::<F>(
        &[FieldValueRef::new(b"\"unterminated"), FieldValueRef::new(b"\r")],
        DecodeErrorKind::InvalidSyntax,
    );
    let too_many_lines = vec![FieldValueRef::new(b"\"unterminated"); MAX_CUSTOM_FIELD_LINES + 1];
    assert_error::<F>(&too_many_lines, DecodeErrorKind::SourceLimitExceeded);
    let too_many_bytes = vec![b'a'; MAX_CUSTOM_FIELD_BYTES];
    assert_error::<F>(
        &[FieldValueRef::new(b"\"unterminated"), FieldValueRef::new(&too_many_bytes)],
        DecodeErrorKind::SourceLimitExceeded,
    );
    for (count, expected) in [
        (MAX_CUSTOM_LIST_ITEMS - 1, DecodeErrorKind::UnterminatedQuote),
        (MAX_CUSTOM_LIST_ITEMS, DecodeErrorKind::SourceLimitExceeded),
    ] {
        let mut wire = iter::repeat_n(good, count).collect::<Vec<_>>().join(",");
        wire.push_str(",\"unterminated");
        assert_error::<F>(&[FieldValueRef::new(wire.as_bytes())], expected);
    }
}

#[test]
fn source_and_member_error_precedence_is_unchanged() {
    assert_precedence::<Accept>("text/plain", b"text/pl ain");
    assert_precedence::<AcceptEncoding>("gzip", b"bad coding");
    assert_precedence::<AcceptLanguage>("en", b"bad_range");
}

#[cfg(feature = "http")]
#[test]
fn http_views_still_exempt_the_retained_adapter_from_all_custom_budgets() {
    macro_rules! unlimited {
        ($header:ty, $http_name:expr, $member:expr) => {{
            let per_line = 128;
            let line_count = MAX_CUSTOM_FIELD_LINES + 1;
            let wire = iter::repeat_n($member, per_line).collect::<Vec<_>>().join(",");
            assert!(wire.len() * line_count > MAX_CUSTOM_FIELD_BYTES);
            assert!(per_line * line_count > MAX_CUSTOM_LIST_ITEMS);
            let mut map = http::HeaderMap::new();
            for _ in 0..line_count {
                map.append($http_name, http::HeaderValue::from_str(&wire).unwrap());
            }
            let view = <$header>::view(&map).unwrap().unwrap();
            for _ in 0..2 {
                assert_eq!(view.entries().count(), per_line * line_count);
                assert_eq!(view.items().count(), per_line * line_count);
            }
            let custom = vec![FieldValueRef::new(wire.as_bytes()); line_count];
            assert_error::<$header>(&custom, DecodeErrorKind::SourceLimitExceeded);
        }};
    }
    unlimited!(Accept, http::header::ACCEPT, "text/plain;q=1");
    unlimited!(AcceptEncoding, http::header::ACCEPT_ENCODING, "gzip;q=1");
    unlimited!(AcceptLanguage, http::header::ACCEPT_LANGUAGE, "en;q=1");
}
