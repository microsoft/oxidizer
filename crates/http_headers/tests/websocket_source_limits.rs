// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Source limits and error precedence across WebSocket fallback paths.

#![cfg(feature = "headers-websocket")]

use std::iter;

use http_headers::headers::{SecWebSocketExtensions, SecWebSocketProtocol, SecWebSocketVersion};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{DecodeError, DecodeErrorKind, DecodeMode, Field, FieldName, FieldValueRef};

struct RawSource<'a> {
    name: &'static FieldName,
    values: Vec<FieldValueRef<'a>>,
    single: bool,
}

impl FieldSource for RawSource<'_> {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        if name != self.name {
            return None;
        }
        if self.single {
            Some(FieldLines::single(name, self.values[0].as_bytes()))
        } else {
            FieldLines::from_borrowed(name, &self.values)
        }
    }
}

fn check<F: Field>(values: &[&[u8]], view: Result<bool, DecodeError>, owned: Result<bool, DecodeError>) {
    let mut source = RawSource {
        name: F::name(),
        values: values.iter().map(|bytes| FieldValueRef::new(bytes)).collect(),
        single: false,
    };
    for single in [false, true] {
        if single && values.len() != 1 {
            continue;
        }
        source.single = single;
        for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
            assert_eq!(
                F::view_with(&source, mode).map(|value| value.is_some()),
                view,
                "{}: borrowed, single={single}, mode={mode:?}",
                F::name()
            );
            assert_eq!(
                F::owned_with(&source, mode).map(|value| value.is_some()),
                owned,
                "{}: owned, single={single}, mode={mode:?}",
                F::name()
            );
        }
    }
}

fn check_limits<F: Field>(item: &[u8], members_at_limit: usize) {
    let invalid = Err(DecodeError::new(F::name(), DecodeErrorKind::InvalidSyntax));
    let limit = Err(DecodeError::new(F::name(), DecodeErrorKind::SourceLimitExceeded));
    let mut bytes = item.to_vec();
    bytes.resize(65_536, b' ');
    check::<F>(&[&bytes], Ok(true), Ok(true));
    bytes.push(b' ');
    check::<F>(&[&bytes], limit, limit);

    let mut lines = vec![item; 128];
    check::<F>(&lines, Ok(true), Ok(true));
    lines.push(item);
    check::<F>(&lines, limit, limit);

    let mut members = iter::repeat_n(item, members_at_limit).collect::<Vec<_>>().join(&b',');
    check::<F>(&[&members], Ok(true), Ok(true));
    members.push(b',');
    members.extend_from_slice(item);
    check::<F>(&[&members], limit, limit);

    // Source validation precedes even a malformed first grammar item.
    check::<F>(&[b"/", &bytes], limit, limit);
    check::<F>(&[b"/", &members], limit, limit);
    for invalid_bytes in [b"\r".as_slice(), b"\n", b"\0", b"\x1f", b"\x7f"] {
        check::<F>(&[invalid_bytes], invalid, invalid);
        check::<F>(&[b"\"unterminated", invalid_bytes], invalid, invalid);
    }
}

#[test]
fn websocket_custom_source_limits_cover_bare_and_quoted_paths() {
    check_limits::<SecWebSocketVersion>(b"13", 1_024);
    check_limits::<SecWebSocketProtocol>(b"chat", 1_024);
    check_limits::<SecWebSocketExtensions>(b"permessage-deflate", 1_024);
    // Each parameter adds a semicolon; the initial extension also consumes an item.
    check_limits::<SecWebSocketExtensions>(b"x; p=\"ab\"", 1_023);
}

#[test]
fn websocket_extension_parameter_budget_covers_quoted_values() {
    let mut bytes = b"x".to_vec();
    for _ in 1..1_024 {
        bytes.extend_from_slice(b";p=\"a\\b\"");
    }
    check::<SecWebSocketExtensions>(&[&bytes], Ok(true), Ok(true));
    bytes.extend_from_slice(b";p=\"a\\b\"");
    let limit = Err(DecodeError::new(
        &FieldName::SecWebSocketExtensions,
        DecodeErrorKind::SourceLimitExceeded,
    ));
    check::<SecWebSocketExtensions>(&[&bytes], limit, limit);
}

#[test]
fn websocket_fallback_errors_preserve_kind_and_physical_line_index() {
    let version_syntax = Err(DecodeError::new(&FieldName::SecWebSocketVersion, DecodeErrorKind::InvalidSyntax));
    let version_quote = Err(DecodeError::new(&FieldName::SecWebSocketVersion, DecodeErrorKind::UnterminatedQuote).at_value(1));
    check::<SecWebSocketVersion>(&[b"256", b"\"13"], version_syntax, version_syntax);
    check::<SecWebSocketVersion>(&[b"13", b"\"13"], version_quote, version_quote);
    check::<SecWebSocketVersion>(&[b"13", b"\"13", b"13\""], version_quote, version_quote);
    check::<SecWebSocketVersion>(&[b"13", b"\"13\""], version_syntax, version_syntax);

    let protocol_token = Err(DecodeError::new(&FieldName::SecWebSocketProtocol, DecodeErrorKind::InvalidToken));
    let protocol_quote = Err(DecodeError::new(&FieldName::SecWebSocketProtocol, DecodeErrorKind::UnterminatedQuote).at_value(1));
    check::<SecWebSocketProtocol>(&[b"/", b"\"chat"], protocol_token, protocol_token);
    check::<SecWebSocketProtocol>(&[b"chat", b"\"chat"], protocol_quote, protocol_token);
    check::<SecWebSocketProtocol>(&[b"chat", b"\"chat", b"chat\""], protocol_quote, protocol_token);

    let extension_token = Err(DecodeError::new(&FieldName::SecWebSocketExtensions, DecodeErrorKind::InvalidToken));
    let extension_quote = Err(DecodeError::new(&FieldName::SecWebSocketExtensions, DecodeErrorKind::UnterminatedQuote).at_value(1));
    check::<SecWebSocketExtensions>(&[b"/", b"x;p=\"a"], extension_token, extension_token);
    check::<SecWebSocketExtensions>(&[b"x", b"x;p=\"a"], extension_quote, extension_quote);
    check::<SecWebSocketExtensions>(&[b"x;p=\"ab\"", b"x;p=\"a"], extension_quote, extension_quote);
    check::<SecWebSocketExtensions>(&[b"x", b"\"x"], extension_quote, extension_token);
}

#[cfg(feature = "http")]
#[test]
fn http_websocket_versions_are_not_subject_to_custom_source_limits() {
    let mut map = http::HeaderMap::new();
    for _ in 0..128 {
        map.append(http::header::SEC_WEBSOCKET_VERSION, http::HeaderValue::from_static("13"));
    }
    map.append(http::header::SEC_WEBSOCKET_VERSION, http::HeaderValue::from_static("255"));
    for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
        let borrowed = SecWebSocketVersion::view_with(&map, mode).unwrap().unwrap();
        let owned = SecWebSocketVersion::owned_with(&map, mode).unwrap().unwrap();
        assert_eq!(borrowed.versions().collect::<Vec<_>>(), [13, 255]);
        assert_eq!(owned.versions().collect::<Vec<_>>(), [13, 255]);
    }
}
