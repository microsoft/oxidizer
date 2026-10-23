// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Credentials decoding preserves singleton and whitespace semantics.

#![cfg(all(feature = "http", feature = "headers-cors"))]

use http::{HeaderMap, HeaderValue};
use http_headers::headers::{AccessControlAllowCredentials, AccessControlAllowCredentialsOwned};
use http_headers::{DecodeErrorKind, DecodeMode, Field};

#[test]
fn owned_and_borrowed_accept_only_true_with_optional_whitespace() {
    for wire in ["true", " true", "true\t", " \ttrue\t "] {
        let mut map = HeaderMap::new();
        map.insert(http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static(wire));
        for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
            let view = AccessControlAllowCredentials::view_with(&map, mode).unwrap().unwrap();
            assert_eq!(view.as_str(), "true");
            assert_eq!(view.as_field_value(), "true");
            assert_eq!(
                AccessControlAllowCredentials::owned_with(&map, mode).unwrap(),
                Some(AccessControlAllowCredentialsOwned::allow())
            );
        }
    }

    for wire in ["", " \t ", "True", "tRue", "trUe", "truE", "tru", "truee", " truee ", "true,false"] {
        let mut map = HeaderMap::new();
        map.insert(http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static(wire));
        for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
            assert_eq!(
                AccessControlAllowCredentials::view_with(&map, mode).unwrap_err().kind(),
                DecodeErrorKind::InvalidSyntax
            );
            assert_eq!(
                AccessControlAllowCredentials::owned_with(&map, mode).unwrap_err().kind(),
                DecodeErrorKind::InvalidSyntax
            );
        }
    }
}

#[test]
fn owned_and_borrowed_distinguish_absence_and_duplicate_values() {
    let mut map = HeaderMap::new();
    for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
        assert_eq!(AccessControlAllowCredentials::view_with(&map, mode).unwrap(), None);
        assert_eq!(AccessControlAllowCredentials::owned_with(&map, mode).unwrap(), None);
    }

    map.append(http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
    map.append(http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
    for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
        assert_eq!(
            AccessControlAllowCredentials::view_with(&map, mode).unwrap_err().kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
        assert_eq!(
            AccessControlAllowCredentials::owned_with(&map, mode).unwrap_err().kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
    }
}
