// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Explicitly fallible UTF-8 access borrows field-value storage.

use bytes::Bytes;
use http_headers::{FieldSensitivity, FieldValue};

#[test]
fn try_as_str_borrows_utf8_in_each_owned_representation() {
    for value in [
        FieldValue::from_static("gzip"),
        FieldValue::from_bytes("münich").unwrap(),
        FieldValue::try_from("x".repeat(65)).unwrap(),
        FieldValue::from_shared(Bytes::from_static(b"shared")).unwrap(),
    ] {
        let value = value.with_sensitivity(FieldSensitivity::Sensitive);
        let text = value.try_as_str().unwrap();
        assert_eq!(text.as_bytes(), value.as_bytes());
        assert_eq!(text.as_ptr(), value.as_bytes().as_ptr());
        assert!(value.is_sensitive());
    }
}

#[test]
fn try_as_str_reports_invalid_utf8_without_changing_wire_bytes() {
    let value = FieldValue::from_bytes(b"a\xff").unwrap();
    let error = value.try_as_str().unwrap_err();
    assert_eq!(error.valid_up_to(), 1);
    assert_eq!(error.error_len(), Some(1));
    assert_eq!(value.as_bytes(), b"a\xff");
}
