// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! RFC 6797 section 6.1 permits empty directives between semicolons.

#![cfg(feature = "headers-security")]

use std::time::Duration;

use http_headers::headers::{StrictTransportSecurity, StrictTransportSecurityOwned};
use http_headers::{DecodeErrorKind, FieldValue, FieldValueRef, SingleValueField};

#[test]
fn empty_directives_are_valid_when_max_age_is_present() {
    for (wire, preload) in [
        (";max-age=60", false),
        ("max-age=60;", false),
        ("max-age=60;;preload", true),
        (" \t; \tmax-age=60 ; ; preload ; \t", true),
        (";max-age=60;;future=\"a;b\";", false),
    ] {
        let owned = StrictTransportSecurityOwned::try_from(wire).unwrap();
        assert_eq!(owned.max_age(), Duration::from_mins(1));
        assert_eq!(owned.preload(), preload);
        assert_eq!(owned.as_field_value().as_bytes(), wire.as_bytes());
        let view = StrictTransportSecurity::decode_view(FieldValueRef::new(wire.as_bytes())).unwrap();
        assert_eq!(view.max_age(), owned.max_age());
        assert_eq!(view.preload(), preload);
        assert_eq!(view.as_field_value().as_bytes(), wire.as_bytes());
        assert_eq!(
            <StrictTransportSecurity as SingleValueField>::decode_owned(FieldValue::from_str(wire).unwrap()).unwrap(),
            owned
        );
        let directives = owned.directives().map(|item| item.unwrap().as_bytes()).collect::<Vec<_>>();
        let expected: &[&[u8]] = if preload {
            &[b"max-age=60", b"preload"]
        } else if wire.contains("future") {
            &[b"max-age=60", b"future=\"a;b\""]
        } else {
            &[b"max-age=60"]
        };
        assert_eq!(directives, expected);
    }
}

#[test]
fn empty_directives_do_not_supply_the_required_max_age() {
    for wire in ["", ";", " ; \t; ", ";;preload;"] {
        assert_eq!(
            StrictTransportSecurityOwned::try_from(wire).unwrap_err().kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }
}
