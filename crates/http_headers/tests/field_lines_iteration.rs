// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Borrowed field-line iteration preserves raw lines and their metadata.

use http_headers::source::FieldLines;
use http_headers::{FieldName, FieldSensitivity, FieldValue, FieldValueRef};

#[test]
fn borrowed_iteration_matches_repeated_without_parsing_or_validating() {
    for bytes in [b"".as_slice(), b"a, b", b"raw\r\n"] {
        let lines = FieldLines::single(&FieldName::SetCookie, bytes);
        let mut iter = (&lines).into_iter();
        assert_eq!(iter.size_hint(), (1, Some(1)));
        assert_eq!(iter.next().unwrap().as_bytes(), bytes);
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert!(iter.next().is_none());
        assert!(iter.next().is_none());
        assert_eq!((&lines).into_iter().collect::<Vec<_>>(), lines.repeated().collect::<Vec<_>>());
        assert_eq!(lines.iter().collect::<Vec<_>>(), lines.repeated().collect::<Vec<_>>());
    }
}

#[test]
fn owned_and_borrowed_storage_keep_line_boundaries_sensitivity_and_lifetimes() {
    let values = [
        FieldValue::from_static("a=1, b=2").with_sensitivity(FieldSensitivity::Sensitive),
        FieldValue::from_static(""),
        FieldValue::from_static("c=3"),
    ];
    let expected = [b"a=1, b=2".as_slice(), b"", b"c=3"];
    let iter = {
        let lines = FieldLines::from_slice(&FieldName::SetCookie, &values).unwrap();
        lines.iter()
    };
    let copied: Vec<_> = iter.collect();
    assert_eq!(copied.iter().map(|value| value.as_bytes()).collect::<Vec<_>>(), expected);
    assert_eq!(
        copied.iter().map(|value| value.is_sensitive()).collect::<Vec<_>>(),
        [true, false, false]
    );
    for (value, original) in copied.iter().zip(&values) {
        assert_eq!(value.as_bytes().as_ptr(), original.as_bytes().as_ptr());
    }

    let refs: Vec<_> = values.iter().map(FieldValue::as_field_value_ref).collect();
    let lines = FieldLines::from_borrowed(&FieldName::SetCookie, &refs).unwrap();
    let mut seen = Vec::new();
    for value in &lines {
        seen.push((value.as_bytes(), value.is_sensitive()));
    }
    assert_eq!(seen, [(expected[0], true), (expected[1], false), (expected[2], false)]);
    assert_eq!((&lines).into_iter().map(FieldValueRef::as_bytes).collect::<Vec<_>>(), expected);
    assert_eq!(
        lines
            .iter()
            .map(|value| (value.as_bytes(), value.is_sensitive()))
            .collect::<Vec<_>>(),
        seen
    );
}

#[cfg(feature = "http")]
#[test]
fn http_line_iteration_preserves_repetition_and_sensitivity() {
    let mut map = http::HeaderMap::new();
    let mut first = http::HeaderValue::from_static("a=1, b=2");
    first.set_sensitive(true);
    map.append(http::header::SET_COOKIE, first);
    map.append(http::header::SET_COOKIE, http::HeaderValue::from_static(""));
    let lines = FieldLines::from_http(&FieldName::SetCookie, map.get_all(http::header::SET_COOKIE)).unwrap();
    let values: Vec<_> = (&lines).into_iter().collect();
    assert_eq!(
        values.iter().map(|value| value.as_bytes()).collect::<Vec<_>>(),
        [b"a=1, b=2".as_slice(), b""]
    );
    assert_eq!(values.iter().map(|value| value.is_sensitive()).collect::<Vec<_>>(), [true, false]);
    assert_eq!(values, lines.repeated().collect::<Vec<_>>());
    assert_eq!(values, lines.iter().collect::<Vec<_>>());
    assert_eq!(lines.iter().map(FieldValueRef::is_sensitive).collect::<Vec<_>>(), [true, false]);
}
