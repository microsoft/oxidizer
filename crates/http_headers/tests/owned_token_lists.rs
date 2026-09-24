// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Typed list construction across inline and shared field-value sizes.

#![cfg(feature = "headers-negotiation")]

use http_headers::headers::{AllowOwned, FieldNameView, MethodView, VaryEntryView, VaryOwned};

#[test]
fn allow_construction_preserves_methods_across_storage_boundaries() {
    for length in [63, 64, 65, 129] {
        let extension = "m".repeat(length - 10);
        let methods = [MethodView::GET, MethodView::new(&extension).unwrap(), MethodView::GET];
        let expected = format!("GET, {extension}, GET");
        assert_eq!(expected.len(), length);

        let value = AllowOwned::from_methods(methods);
        assert_eq!(value.values().len(), 1);
        let line = value.values().next().unwrap();
        assert_eq!(line.as_bytes(), expected.as_bytes());
        assert!(!line.is_sensitive());
        assert_eq!(value.methods().collect::<Vec<_>>(), methods);

        let retained = value.clone();
        drop(value);
        assert_eq!(retained.values().next().unwrap().as_bytes(), expected.as_bytes());
        assert_eq!(retained.methods().collect::<Vec<_>>(), methods);
    }
}

#[test]
fn vary_construction_preserves_wildcards_names_and_spelling_across_storage_boundaries() {
    for length in [63, 64, 65, 129] {
        let custom_name = format!("X-{}", "a".repeat(length - 21));
        let custom = FieldNameView::new(&custom_name).unwrap();
        let origin = FieldNameView::new("Origin").unwrap();
        let entries = [
            VaryEntryView::from_field_name(origin),
            VaryEntryView::WILDCARD,
            VaryEntryView::from_field_name(custom),
            VaryEntryView::from_field_name(origin),
        ];
        let expected = format!("Origin, *, {custom_name}, Origin");
        assert_eq!(expected.len(), length);

        let value = VaryOwned::from_entries(entries);
        assert_eq!(value.values().len(), 1);
        let line = value.values().next().unwrap();
        assert_eq!(line.as_bytes(), expected.as_bytes());
        assert!(!line.is_sensitive());
        assert!(value.contains_wildcard());
        assert_eq!(value.entries().collect::<Vec<_>>(), entries);

        let retained = value.clone();
        drop(value);
        assert_eq!(retained.values().next().unwrap().as_bytes(), expected.as_bytes());
        assert_eq!(retained.entries().collect::<Vec<_>>(), entries);
        assert!(retained.contains_wildcard());

        let names = VaryOwned::from_field_names([custom, origin, custom]);
        assert_eq!(
            names.values().next().unwrap().as_bytes(),
            format!("{custom_name}, Origin, {custom_name}").as_bytes()
        );
        assert!(!names.contains_wildcard());
    }
}
