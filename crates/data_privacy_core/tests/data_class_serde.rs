// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Data class serialization and deserialization.

#![cfg(feature = "serde")]

use data_privacy_core::DataClass;

#[test]
fn test_serialize() {
    let dc = DataClass::new("contoso", "customer_identifier");
    let serialized = serde_json::to_string(&dc).unwrap();
    assert_eq!(serialized, "\"contoso/customer_identifier\"");
}

#[test]
fn test_deserialize_valid() {
    let serialized = "\"contoso/customer_identifier\"";
    let dc: DataClass = serde_json::from_str(serialized).unwrap();
    assert_eq!(dc.taxonomy(), "contoso");
    assert_eq!(dc.name(), "customer_identifier");
}

#[test]
fn test_deserialize_invalid_format_no_slash() {
    let serialized = "\"contoso_customer_identifier\"";
    let err = serde_json::from_str::<DataClass>(serialized).unwrap_err();
    assert!(err.to_string().contains("expecting taxonomy/name format"));
}

#[test]
fn test_deserialize_invalid_format_empty_taxonomy() {
    let serialized = "\"/customer_identifier\"";
    let err = serde_json::from_str::<DataClass>(serialized).unwrap_err();
    assert!(err.to_string().contains("invalid taxonomy identifier"));
}

#[test]
fn test_deserialize_invalid_format_empty_name() {
    let serialized = "\"contoso/\"";
    let err = serde_json::from_str::<DataClass>(serialized).unwrap_err();
    assert!(err.to_string().contains("invalid name identifier"));
}

#[test]
fn test_deserialize_invalid_taxonomy() {
    let serialized = "\"a-b/c\"";
    let err = serde_json::from_str::<DataClass>(serialized).unwrap_err();
    assert!(err.to_string().contains("invalid taxonomy identifier"));
}

#[test]
fn test_deserialize_invalid_name() {
    let serialized = "\"a/b-c\"";
    let err = serde_json::from_str::<DataClass>(serialized).unwrap_err();
    assert!(err.to_string().contains("invalid name identifier"));
}

#[test]
fn test_deserialize_wrong_type_triggers_expecting() {
    let err = serde_json::from_str::<DataClass>("42").unwrap_err();
    assert!(err.to_string().contains("a string in taxonomy/name format"));
}
