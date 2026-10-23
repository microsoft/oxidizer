// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Content-Type equality and hashing independent of construction and metadata.

#![cfg(feature = "headers-content-type")]

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};

use http_headers::headers::{ContentType, ContentTypeOwned};
use http_headers::{FieldSensitivity, FieldValue};

use self::common::TestMap;

mod common;

const COMMON: [(&str, &str); 6] = [
    ("application", "json"),
    ("application", "octet-stream"),
    ("text", "html"),
    ("text", "plain"),
    ("text", "css"),
    ("application", "javascript"),
];

fn fingerprint(value: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[test]
fn every_common_constructor_matches_raw_parsing_reconstruction_and_hash_set_lookup() {
    for (type_, subtype) in COMMON {
        let wire = format!("{type_}/{subtype}");
        let field = FieldValue::from_str(&wire).unwrap();
        let constructed = ContentTypeOwned::new(type_, subtype).unwrap();
        let mut map = TestMap::default();
        ContentType::insert(&mut map, constructed.clone()).unwrap();
        let mut values = HashSet::from([constructed.clone()]);
        for candidate in [
            ContentTypeOwned::try_from(wire.as_str()).unwrap(),
            ContentTypeOwned::try_from(wire.clone()).unwrap(),
            ContentTypeOwned::try_from(field.clone()).unwrap(),
            ContentTypeOwned::try_from(constructed.clone().into_field_value()).unwrap(),
            ContentType::owned(&map).unwrap().unwrap(),
        ] {
            assert_eq!(candidate, constructed);
            assert_eq!(candidate.type_().unwrap(), type_);
            assert_eq!(candidate.subtype().unwrap(), subtype);
            assert_eq!(candidate.parameters().count(), 0);
            assert_eq!(candidate.clone().into_field_value(), field);
            assert_eq!(fingerprint(&candidate), fingerprint(&constructed));
            assert_eq!(fingerprint(&candidate), fingerprint(&field));
            assert!(values.contains(&candidate));
            assert!(!values.insert(candidate));
        }
        assert_eq!(values.len(), 1);
        #[cfg(feature = "serde")]
        {
            let json = serde_json::to_string(&constructed).unwrap();
            let decoded: ContentTypeOwned = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, constructed);
            assert_eq!(fingerprint(&decoded), fingerprint(&constructed));
            assert!(values.contains(&decoded));
            assert_eq!(decoded.into_field_value(), field);
        }
    }
}

#[test]
fn json_convenience_constructors_have_the_same_wire_identity() {
    let constructed = ContentTypeOwned::new("application", "json").unwrap();
    for value in [ContentTypeOwned::json(), ContentType::json()] {
        assert_eq!(value, constructed);
        assert_eq!(fingerprint(&value), fingerprint(&constructed));
        assert_eq!(value.into_field_value().as_bytes(), b"application/json");
    }
}

#[test]
fn content_type_identity_ignores_sensitivity_without_discarding_the_marker() {
    for (type_, subtype) in COMMON {
        let constructed = ContentTypeOwned::new(type_, subtype).unwrap();
        let sensitive_field = constructed.clone().into_field_value().with_sensitivity(FieldSensitivity::Sensitive);
        let sensitive = ContentTypeOwned::try_from(sensitive_field.clone()).unwrap();
        assert_eq!(sensitive, constructed);
        assert_eq!(fingerprint(&sensitive), fingerprint(&constructed));
        assert_eq!(fingerprint(&sensitive), fingerprint(&sensitive_field));
        assert!(sensitive.into_field_value().is_sensitive());
        assert!(!constructed.into_field_value().is_sensitive());
    }
}

#[test]
fn content_type_identity_keeps_case_parameters_order_and_quoting_distinct() {
    for (left, right) in [
        ("application/json", "Application/json"),
        ("application/json", "application/JSON"),
        ("application/json", "application/javascript"),
        ("text/plain; charset=utf-8", "text/plain;charset=utf-8"),
        ("text/plain; charset=utf-8", "text/plain; charset=UTF-8"),
        ("text/plain;a=1;b=2", "text/plain;b=2;a=1"),
        ("text/plain;x=\"a\"", "text/plain;x=a"),
        ("application/json", "application/json; charset=utf-8"),
    ] {
        let left_field = FieldValue::from_str(left).unwrap();
        let right_field = FieldValue::from_str(right).unwrap();
        let left = ContentTypeOwned::try_from(left_field.clone()).unwrap();
        let right = ContentTypeOwned::try_from(right_field.clone()).unwrap();
        assert_ne!(left, right);
        assert_eq!(fingerprint(&left), fingerprint(&left_field));
        assert_eq!(fingerprint(&right), fingerprint(&right_field));
        let values = HashSet::from([left.clone(), right.clone()]);
        assert_eq!(values.len(), 2);
        assert!(values.contains(&left));
        assert!(values.contains(&right));
        #[cfg(feature = "serde")]
        for original in [&left, &right] {
            let json = serde_json::to_string(original).unwrap();
            let decoded: ContentTypeOwned = serde_json::from_str(&json).unwrap();
            assert_eq!(&decoded, original);
            assert_eq!(fingerprint(&decoded), fingerprint(original));
            assert!(values.contains(&decoded));
        }
    }
}
