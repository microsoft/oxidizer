// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public support API integration coverage.

#![cfg(feature = "headers-all")]

#[cfg(feature = "http")]
use http_headers::headers::{LocationOwned, UserAgent, UserAgentOwned};
use http_headers::sink::{EncodedValues, InsertError};
use http_headers::{DecodeError, DecodeErrorKind, FieldName, FieldValue};

#[test]
fn encoded_values_and_errors_expose_stable_public_behavior() {
    let adopted = EncodedValues::from_vec(vec![FieldValue::from_static("a")]);
    assert_eq!(adopted.len(), 1);
    assert!(!adopted.is_empty());

    let encoded: EncodedValues = [FieldValue::from_static("first"), FieldValue::from_static("second")]
        .into_iter()
        .collect();
    assert_eq!(encoded.len(), 2);
    assert!(!format!("{encoded:?}").contains("first"));
    assert_eq!(
        encoded.into_iter().collect::<Vec<_>>(),
        [FieldValue::from_static("first"), FieldValue::from_static("second"),]
    );

    let cases = [
        (DecodeErrorKind::MissingValue, "missing value"),
        (DecodeErrorKind::UnexpectedMultipleValues, "unexpected multiple values"),
        (DecodeErrorKind::InvalidSyntax, "invalid syntax"),
        (DecodeErrorKind::InvalidUtf8, "invalid UTF-8"),
        (DecodeErrorKind::InvalidToken, "invalid token"),
        (DecodeErrorKind::InvalidNumber, "invalid number"),
        (DecodeErrorKind::UnterminatedQuote, "unterminated quoted string"),
        (DecodeErrorKind::CacheTypeMismatch, "internal cache type mismatch"),
    ];
    for (kind, expected) in cases {
        assert_eq!(kind.to_string(), expected);
    }
    let error = DecodeError::new(&FieldName::ContentType, DecodeErrorKind::InvalidSyntax).at_value(2);
    assert_eq!(error.value_index(), Some(2));
    assert_eq!(error.to_string(), "invalid content-type header: invalid syntax at value 2");
    assert_eq!(InsertError.to_string(), "field could not be encoded or stored");
}

#[test]
#[cfg(feature = "http")]
fn location_and_user_agent_use_public_construction_and_decode() {
    for value in [
        "https://example.com/people",
        "../people/tim?tab=1#profile",
        "#profile",
        "",
        "https://[v1.address]/",
    ] {
        LocationOwned::try_from(value).expect("valid URI-reference");
    }
    for value in ["/bad%2", "/café", "http://[::1", "/a[b]", "1abc:def"] {
        assert_eq!(
            LocationOwned::try_from(value).expect_err("invalid URI-reference must fail").kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }
    let signed = LocationOwned::try_from("/download?signature=secret").expect("valid URI-reference");
    let debug = format!("{signed:?}");
    assert!(!debug.contains("secret"));
    assert!(debug.contains("redacted"));

    let mut map = http::HeaderMap::new();
    UserAgent::insert(&mut map, UserAgentOwned::try_from("example-client/1.0").expect("valid user agent")).expect("map has capacity");
    assert_eq!(
        UserAgent::view(&map).expect("valid header").expect("present").as_str(),
        Ok("example-client/1.0")
    );
    assert_eq!(
        UserAgentOwned::try_from(" \t").expect_err("blank user agent must fail").kind(),
        DecodeErrorKind::InvalidSyntax
    );
}
