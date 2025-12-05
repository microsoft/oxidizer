// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use core::hash::{Hash, Hasher};
use data_privacy::DataClass;
use std::collections::hash_map::DefaultHasher;

#[test]
fn new_should_create_data_class() {
    let data_class = DataClass::new("taxonomy", "class");
    assert_eq!(data_class.taxonomy(), "taxonomy");
    assert_eq!(data_class.name(), "class");
}

#[test]
fn display_should_format_correctly() {
    let data_class = DataClass::new("taxonomy", "class");
    assert_eq!(format!("{data_class}"), "taxonomy/class");
}

#[test]
fn derived_traits_should_work_as_expected() {
    let data_class1 = DataClass::new("tax", "class");
    let data_class2 = DataClass::new("tax", "class");
    let data_class3 = DataClass::new("tax", "other");
    let data_class4 = DataClass::new("other_tax", "class");

    // Clone
    assert_eq!(data_class1, data_class1.clone());

    // PartialEq, Eq
    assert_eq!(data_class1, data_class2);
    assert_ne!(data_class1, data_class3);
    assert_ne!(data_class1, data_class4);

    // PartialOrd, Ord
    assert!(data_class1 < data_class3);
    assert!(data_class1 > data_class4);
    assert!(data_class3 > data_class4);
    assert_eq!(data_class1.cmp(&data_class2), core::cmp::Ordering::Equal);

    // Hash
    let mut hasher1 = DefaultHasher::new();
    data_class1.hash(&mut hasher1);
    let hash1 = hasher1.finish();

    let mut hasher2 = DefaultHasher::new();
    data_class2.hash(&mut hasher2);
    let hash2 = hasher2.finish();

    let mut hasher3 = DefaultHasher::new();
    data_class3.hash(&mut hasher3);
    let hash3 = hasher3.finish();

    assert_eq!(hash1, hash2);
    assert_ne!(hash1, hash3);
}

#[test]
#[cfg(feature = "serde")]
fn serde_should_serialize_and_deserialize() {
    let data_class = DataClass::new("taxonomy", "class");
    let serialized = serde_json::to_string(&data_class).unwrap();
    let deserialized: DataClass = serde_json::from_str(&serialized).unwrap();
    assert_eq!(data_class, deserialized);
}
