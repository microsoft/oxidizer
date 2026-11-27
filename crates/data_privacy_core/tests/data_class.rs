// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Data class identifiers, formatting and comparison.

use data_privacy_core::{DataClass, IntoDataClass};

#[test]
fn new_stores_taxonomy_and_name() {
    let dc = DataClass::new("contoso", "customer_id");
    assert_eq!(dc.taxonomy(), "contoso");
    assert_eq!(dc.name(), "customer_id");
}

#[test]
fn display_uses_slash_separator() {
    let dc = DataClass::new("contoso", "customer_id");
    assert_eq!(dc.to_string(), "contoso/customer_id");
}

#[test]
fn as_ref_returns_self() {
    let dc = DataClass::new("contoso", "customer_id");
    let dc_ref: &DataClass = dc.as_ref();
    assert_eq!(dc_ref, &dc);
}

#[test]
fn into_data_class_identity() {
    let dc = DataClass::new("contoso", "customer_id");
    let dc2 = dc.clone().into_data_class();
    assert_eq!(dc, dc2);
}

#[test]
fn valid_identifiers() {
    for identifier in ["foo", "_bar", "Baz123", "_", "a"] {
        let taxonomy = DataClass::new(identifier, "name");
        assert_eq!(taxonomy.taxonomy(), identifier);
        let name = DataClass::new("taxonomy", identifier);
        assert_eq!(name.name(), identifier);
    }
}

#[test]
fn invalid_identifiers() {
    for identifier in ["", "1abc", "a-b", "a b"] {
        let taxonomy_panic = std::panic::catch_unwind(|| DataClass::new(identifier, "name")).unwrap_err();
        assert_eq!(
            taxonomy_panic.downcast_ref::<&str>().unwrap(),
            &"taxonomy is not a valid identifier"
        );
        let name_panic = std::panic::catch_unwind(|| DataClass::new("taxonomy", identifier)).unwrap_err();
        assert_eq!(name_panic.downcast_ref::<&str>().unwrap(), &"name is not a valid identifier");
    }
}

#[test]
fn data_class_equality_and_ordering() {
    let a = DataClass::new("a", "x");
    let b = DataClass::new("b", "x");
    assert!(a < b);
    assert_eq!(a, a.clone());
}

#[test]
fn data_class_debug() {
    let dc = DataClass::new("t", "n");
    let dbg = format!("{dc:?}");
    assert!(dbg.contains("DataClass"));
}

#[test]
fn data_class_hash() {
    use std::collections::HashSet;
    let mut set = HashSet::new();
    set.insert(DataClass::new("t", "n"));
    assert!(set.contains(&DataClass::new("t", "n")));
}
