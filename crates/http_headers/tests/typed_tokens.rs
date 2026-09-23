// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Semantic method and selection-field APIs across owned and borrowed headers.

#![cfg(feature = "headers-negotiation")]

use std::collections::HashSet;

use http_headers::headers::{Allow, AllowOwned, FieldNameView, MethodView, Vary, VaryEntryView, VaryOwned};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{DecodeErrorKind, FieldName, FieldSensitivity, FieldValue, FieldValueRef, InvalidFieldName};

struct Source<'a> {
    name: &'static FieldName,
    values: &'a [FieldValue],
}

impl FieldSource for Source<'_> {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (name == self.name).then(|| FieldLines::from_slice(name, self.values)).flatten()
    }
}

#[test]
fn allow_preserves_case_extensions_order_and_empty_members() {
    let mut first = FieldValue::from_static("GET, , get, *");
    first.set_sensitivity(FieldSensitivity::Sensitive);
    let values = [first, FieldValue::from_static("CUSTOM, GET")];
    let source = Source {
        name: &FieldName::Allow,
        values: &values,
    };
    let view = Allow::view(&source).unwrap().unwrap();
    let owned = Allow::owned(&source).unwrap().unwrap();
    let expected = ["GET", "get", "*", "CUSTOM", "GET"];
    assert_eq!(view.methods().map(MethodView::as_str).collect::<Vec<_>>(), expected);
    assert_eq!(owned.methods().map(MethodView::as_str).collect::<Vec<_>>(), expected);
    assert_eq!(
        owned.values().map(FieldValueRef::as_bytes).collect::<Vec<_>>(),
        [b"GET, , get, *".as_slice(), b"CUSTOM, GET"]
    );
    assert!(owned.values().next().unwrap().is_sensitive());
    let methods: HashSet<_> = view.methods().collect();
    assert_eq!(methods.len(), 4);
    assert!(methods.contains(&MethodView::GET));
    assert_ne!(MethodView::GET, MethodView::new("get").unwrap());
    assert_eq!(MethodView::new("*").unwrap().as_str(), "*");
}

#[test]
fn method_construction_and_typed_allow_construction() {
    let standards = [
        MethodView::GET,
        MethodView::HEAD,
        MethodView::POST,
        MethodView::PUT,
        MethodView::DELETE,
        MethodView::CONNECT,
        MethodView::OPTIONS,
        MethodView::TRACE,
        MethodView::PATCH,
    ];
    let owned = AllowOwned::from_methods(standards);
    assert_eq!(owned.methods().collect::<Vec<_>>(), standards);
    assert_eq!(
        owned.values().next().unwrap().as_bytes(),
        b"GET, HEAD, POST, PUT, DELETE, CONNECT, OPTIONS, TRACE, PATCH"
    );
    let extension = MethodView::try_from("custom-method").unwrap();
    assert_eq!(extension.as_bytes(), b"custom-method");
    assert_eq!(extension.as_ref(), "custom-method");
    assert_eq!(extension.to_string(), "custom-method");
    assert_eq!(format!("{extension:?}"), "MethodView(\"custom-method\")");
    let empty = AllowOwned::from_methods([]);
    assert_eq!(empty.methods().count(), 0);
    assert_eq!(empty.values().len(), 1);
    assert_eq!(empty.values().next().unwrap().as_bytes(), b"");
    for invalid in ["", "bad method", "GET,POST", "méthode", "GET\n"] {
        let error = MethodView::new(invalid).unwrap_err();
        assert_eq!(error.to_string(), "invalid HTTP method");
    }
}

#[test]
fn vary_names_compare_and_hash_without_losing_wire_spelling() {
    let values = [
        FieldValue::from_static("Accept-Encoding, , Origin"),
        FieldValue::from_static("accept-encoding, X-Custom"),
    ];
    let source = Source {
        name: &FieldName::Vary,
        values: &values,
    };
    let view = Vary::view(&source).unwrap().unwrap();
    let owned = Vary::owned(&source).unwrap().unwrap();
    assert!(!view.contains_wildcard());
    assert!(!owned.contains_wildcard());
    assert_eq!(view.entries().collect::<Vec<_>>(), owned.entries().collect::<Vec<_>>());
    assert_eq!(
        view.entries().map(VaryEntryView::as_str).collect::<Vec<_>>(),
        ["Accept-Encoding", "Origin", "accept-encoding", "X-Custom"]
    );
    let names: HashSet<_> = view.entries().map(|entry| entry.field_name().unwrap()).collect();
    assert_eq!(names.len(), 3);
    assert!(names.contains(&FieldNameView::new("ACCEPT-ENCODING").unwrap()));
    assert_eq!(
        FieldNameView::new("X-Custom").unwrap().try_to_field_name().unwrap().as_str(),
        "x-custom"
    );
    assert_eq!(
        FieldNameView::new("ACCEPT").unwrap().try_to_field_name().unwrap(),
        FieldName::Accept
    );
    assert_ne!(VaryOwned::try_from("Origin").unwrap(), VaryOwned::try_from("origin").unwrap());
}

#[test]
fn vary_wildcards_remain_visible_across_field_lines() {
    for wire in ["*", "*, Origin", "Origin, *", "Origin, *, ACCEPT"] {
        let values = [FieldValue::from_static("X-First"), FieldValue::from_str(wire).unwrap()];
        let source = Source {
            name: &FieldName::Vary,
            values: &values,
        };
        let view = Vary::view(&source).unwrap().unwrap();
        let owned = Vary::owned(&source).unwrap().unwrap();
        assert!(view.contains_wildcard());
        assert!(owned.contains_wildcard());
        assert_eq!(view.entries().filter(|entry| entry.is_wildcard()).count(), 1);
        assert_eq!(view.entries().find(|entry| entry.is_wildcard()).unwrap().field_name(), None);
        assert_eq!(owned.values().nth(1).unwrap().as_bytes(), wire.as_bytes());
    }
    let name = FieldNameView::new("Origin").unwrap();
    let rebuilt = VaryOwned::from_entries([VaryEntryView::from_field_name(name), VaryEntryView::WILDCARD]);
    assert_eq!(rebuilt.values().next().unwrap().as_bytes(), b"Origin, *");
    assert_eq!(VaryEntryView::WILDCARD.to_string(), "*");
    assert_eq!(VaryEntryView::from_field_name(name).to_string(), "Origin");
    assert!(VaryEntryView::from_field_name(FieldNameView::new("*").unwrap()).is_wildcard());
    assert!(VaryOwned::wildcard().contains_wildcard());
    let names = VaryOwned::from_field_names([name, name]);
    assert_eq!(names.entries().count(), 2);
    let empty = VaryOwned::from_field_names([]);
    assert_eq!(empty.entries().count(), 0);
    assert!(!empty.contains_wildcard());
}

#[test]
fn vary_contains_wildcard_requires_a_complete_wildcard_member() {
    for wire in ["Origin", "Origin, X-*", "**", " ,\t,"] {
        let values = [FieldValue::from_str(wire).unwrap()];
        let source = Source {
            name: &FieldName::Vary,
            values: &values,
        };
        assert!(!Vary::view(&source).unwrap().unwrap().contains_wildcard());
        assert!(!Vary::owned(&source).unwrap().unwrap().contains_wildcard());
        assert_eq!(values[0].as_bytes(), wire.as_bytes());
    }
}

#[test]
fn field_name_views_keep_validation_and_materialization_separate() {
    let name = FieldNameView::try_from("X-Foo").unwrap();
    assert!(name.eq_ignore_ascii_case("x-foo"));
    assert!(!name.eq_ignore_ascii_case("x-bar"));
    assert_eq!(name.as_ref(), "X-Foo");
    assert_eq!(name.as_bytes(), b"X-Foo");
    assert_eq!(name.to_string(), "X-Foo");
    assert_eq!(format!("{name:?}"), "FieldNameView(\"X-Foo\")");
    for invalid in ["", "bad name", "name:", "naïve", "x\n"] {
        assert_eq!(FieldNameView::new(invalid).unwrap_err(), InvalidFieldName);
    }
    let long = "x".repeat(65_536);
    let name = FieldNameView::new(&long).unwrap();
    assert_eq!(name.as_str(), long);
    assert_eq!(name.try_to_field_name().unwrap_err(), InvalidFieldName);
    let value = VaryOwned::try_from(long.as_str()).unwrap();
    assert_eq!(value.entries().next().unwrap().field_name().unwrap(), name);
}

#[test]
fn malformed_members_fail_decoding_instead_of_disappearing() {
    for name in [&FieldName::Allow, &FieldName::Vary] {
        let values = [FieldValue::from_static("GET"), FieldValue::from_static("bad token")];
        let source = Source { name, values: &values };
        let error = if name == &FieldName::Allow {
            Allow::view(&source).unwrap_err()
        } else {
            Vary::view(&source).unwrap_err()
        };
        assert_eq!(error.kind(), DecodeErrorKind::InvalidToken);
    }
}

#[cfg(feature = "http")]
#[test]
fn http_conversions_and_round_trips_preserve_typed_members() {
    use http::HeaderMap;

    assert_eq!(MethodView::GET.try_to_method().unwrap(), http::Method::GET);
    assert_eq!(MethodView::new("CUSTOM").unwrap().try_to_method().unwrap().as_str(), "CUSTOM");
    assert_eq!(
        FieldNameView::new("X-Custom").unwrap().try_to_http_header_name().unwrap().as_str(),
        "x-custom"
    );
    let mut map = HeaderMap::new();
    Allow::insert(&mut map, AllowOwned::from_methods([MethodView::GET, MethodView::HEAD])).unwrap();
    assert_eq!(
        Allow::view(&map).unwrap().unwrap().methods().collect::<Vec<_>>(),
        [MethodView::GET, MethodView::HEAD]
    );
    Vary::insert(
        &mut map,
        VaryOwned::from_field_names([FieldNameView::new("Accept-Encoding").unwrap()]),
    )
    .unwrap();
    assert_eq!(
        Vary::view(&map).unwrap().unwrap().entries().next().unwrap().as_str(),
        "Accept-Encoding"
    );
}
