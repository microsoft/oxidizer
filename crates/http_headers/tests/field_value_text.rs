// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Fallible text access and validated ownership conversions for field values.

use bytes::Bytes;
use http_headers::{FieldSensitivity, FieldValue, FieldValueRef, InvalidFieldValue};

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

#[test]
fn to_str_borrows_utf8_without_validating_the_field_value_grammar() {
    for text in ["", "gzip", "münich", "bad\nvalue"] {
        let value = FieldValueRef::new(text.as_bytes()).with_sensitivity(FieldSensitivity::Sensitive);
        let borrowed = value.to_str().unwrap();
        assert_eq!(borrowed, text);
        assert_eq!(borrowed.as_ptr(), text.as_ptr());
        assert!(value.is_sensitive());
    }

    let value = FieldValueRef::new(b"a\xff");
    let error = value.to_str().unwrap_err();
    assert_eq!(error.valid_up_to(), 1);
    assert_eq!(error.error_len(), Some(1));
    assert_eq!(value.as_bytes(), b"a\xff");
}

#[test]
fn borrowed_to_owned_conversions_reject_invalid_field_bytes() {
    for byte in (0..=0x1f).filter(|&byte| byte != b'\t').chain([0x7f]) {
        let embedded = [b'a', byte, b'b'];
        for wire in [&embedded[1..2], &embedded[..]] {
            for sensitivity in [FieldSensitivity::NonSensitive, FieldSensitivity::Sensitive] {
                let value = FieldValueRef::new(wire).with_sensitivity(sensitivity);
                assert_eq!(value.try_to_field_value().unwrap_err(), InvalidFieldValue);
                assert_eq!(FieldValue::try_from(value).unwrap_err(), InvalidFieldValue);
                assert_eq!(value.as_bytes(), wire);
                assert_eq!(value.is_sensitive(), sensitivity.is_sensitive());
            }
        }
    }
}

#[test]
fn borrowed_to_owned_conversions_preserve_valid_bytes_and_sensitivity() {
    let long = [b'a'; 65];
    for wire in [b"".as_slice(), b"good value", b"\t \x80\xff", "münich".as_bytes(), &long] {
        for sensitivity in [FieldSensitivity::NonSensitive, FieldSensitivity::Sensitive] {
            let value = FieldValueRef::new(wire).with_sensitivity(sensitivity);
            for owned in [value.try_to_field_value().unwrap(), FieldValue::try_from(value).unwrap()] {
                assert_eq!(owned.as_bytes(), wire);
                assert_eq!(owned.is_sensitive(), sensitivity.is_sensitive());
            }
        }
    }
}
