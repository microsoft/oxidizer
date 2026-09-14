// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Version 13 decoding retains complete version-set and source validation.

#![cfg(all(feature = "http", feature = "headers-websocket"))]
#![expect(clippy::unwrap_used, reason = "test failures provide sufficient context")]

use http::{HeaderMap, HeaderValue};
use http_headers::headers::SecWebSocketVersion;
use http_headers::source::{FieldLines, FieldSource, MAX_CUSTOM_FIELD_LINES};
use http_headers::{DecodeErrorKind, DecodeMode, Field, FieldName, FieldValue};

fn assert_versions(source: &impl FieldSource, expected: &[u8]) {
    for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
        let view = SecWebSocketVersion::view_with(source, mode).unwrap().unwrap();
        let owned = SecWebSocketVersion::owned_with(source, mode).unwrap().unwrap();
        assert_eq!(view.versions().collect::<Vec<_>>(), expected);
        assert_eq!(owned, view);
        if let [only] = expected {
            assert_eq!(view.requested(), Ok(*only));
        } else {
            assert_eq!(view.requested().unwrap_err().kind(), DecodeErrorKind::UnexpectedMultipleValues);
        }
    }
}

fn assert_error(source: &impl FieldSource, expected: DecodeErrorKind) {
    for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
        assert_eq!(SecWebSocketVersion::view_with(source, mode).unwrap_err().kind(), expected);
        assert_eq!(SecWebSocketVersion::owned_with(source, mode).unwrap_err().kind(), expected);
    }
}

fn version_map(values: &[&str]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for value in values {
        map.append(
            http::header::SEC_WEBSOCKET_VERSION,
            HeaderValue::from_bytes(value.as_bytes()).unwrap(),
        );
    }
    map
}

#[test]
fn owned_and_borrowed_decode_every_version() {
    for version in 0..=u8::MAX {
        assert_versions(&version_map(&[&version.to_string()]), &[version]);
    }
    assert_versions(&version_map(&[" \t13\t "]), &[13]);
}

#[test]
fn version_thirteen_does_not_hide_later_versions_or_errors() {
    assert_versions(&version_map(&["13", "8"]), &[8, 13]);
    assert_versions(&version_map(&["13", "13"]), &[13]);
    assert_versions(&version_map(&["13, 8, 13"]), &[8, 13]);
    assert_versions(&version_map(&["13", ", 7, 13, ,"]), &[7, 13]);
    assert_versions(&version_map(&["13", "255"]), &[13, 255]);

    for invalid in ["013", "13x", "1300", "256"] {
        assert_error(&version_map(&[invalid]), DecodeErrorKind::InvalidSyntax);
        assert_error(&version_map(&["13", invalid]), DecodeErrorKind::InvalidSyntax);
    }
    assert_error(&version_map(&["13", "\"13"]), DecodeErrorKind::UnterminatedQuote);
    assert_error(&version_map(&[""]), DecodeErrorKind::MissingValue);

    for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
        assert_eq!(SecWebSocketVersion::view_with(&HeaderMap::new(), mode).unwrap(), None);
        assert_eq!(SecWebSocketVersion::owned_with(&HeaderMap::new(), mode).unwrap(), None);
    }
}

struct VersionsSource(Vec<FieldValue>);

impl FieldSource for VersionsSource {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (name == &FieldName::SecWebSocketVersion)
            .then(|| FieldLines::from_slice(name, &self.0))
            .flatten()
    }
}

#[test]
fn version_thirteen_preserves_custom_source_limits() {
    let mut source = VersionsSource(vec![FieldValue::from_static("13")]);
    assert_versions(&source, &[13]);
    source.0.resize(MAX_CUSTOM_FIELD_LINES, FieldValue::from_static("13"));
    assert_versions(&source, &[13]);
    source.0.push(FieldValue::from_static("13"));
    assert_error(&source, DecodeErrorKind::InvalidSyntax);
}
