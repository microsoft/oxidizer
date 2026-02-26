// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]
#![cfg(feature = "rapidhash")]

use data_privacy::rapidhash_redactor::{REDACTED_LEN, RapidHashRedactor};
use data_privacy::{DataClass, Redactor};
use rapidhash::v3::RapidSecrets;

fn get_test_redactor() -> RapidHashRedactor {
    let secrets = RapidSecrets::seed(1234);
    RapidHashRedactor::with_secrets(secrets)
}

#[test]
fn test_redact_produces_consistent_output() {
    let redactor = get_test_redactor();
    let data_class = DataClass::new("test_taxonomy", "test_class");
    let input = "sensitive_data";

    let mut output1 = String::new();
    let mut output2 = String::new();

    redactor.redact(&data_class, input, &mut output1).unwrap();
    redactor.redact(&data_class, input, &mut output2).unwrap();

    assert_eq!(output1, output2);
    assert_eq!(output1.len(), REDACTED_LEN);
}

#[test]
fn test_redact_output_is_hex_string() {
    let redactor = get_test_redactor();
    let data_class = DataClass::new("test_taxonomy", "test_class");
    let input = "test_input";

    let mut output = String::new();
    redactor.redact(&data_class, input, &mut output).unwrap();

    assert_eq!(output.len(), REDACTED_LEN);
    assert!(output.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(output.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
}

#[test]
fn test_different_inputs_produce_different_outputs() {
    let redactor = get_test_redactor();
    let data_class = DataClass::new("test_taxonomy", "test_class");

    let mut output1 = String::new();
    let mut output2 = String::new();

    redactor.redact(&data_class, "input1", &mut output1).unwrap();
    redactor.redact(&data_class, "input2", &mut output2).unwrap();

    assert_ne!(output1, output2);
}

#[test]
fn test_different_secrets_produce_different_outputs() {
    let redactor1 = get_test_redactor();
    let custom_secrets = RapidSecrets::seed(6789);
    let redactor2 = RapidHashRedactor::with_secrets(custom_secrets);
    let data_class = DataClass::new("test_taxonomy", "test_class");
    let input = "same_input";

    let mut output1 = String::new();
    let mut output2 = String::new();

    redactor1.redact(&data_class, input, &mut output1).unwrap();
    redactor2.redact(&data_class, input, &mut output2).unwrap();

    assert_ne!(output1, output2);
}

#[test]
fn test_empty_string_input() {
    let redactor = get_test_redactor();
    let data_class = DataClass::new("test_taxonomy", "test_class");

    let mut output = String::new();
    redactor.redact(&data_class, "", &mut output).unwrap();

    assert_eq!(output.len(), REDACTED_LEN);
    assert!(output.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_unicode_input() {
    let redactor = get_test_redactor();
    let data_class = DataClass::new("test_taxonomy", "test_class");
    let input = "こんにちは世界"; // "Hello World" in Japanese

    let mut output = String::new();
    redactor.redact(&data_class, input, &mut output).unwrap();

    assert_eq!(output.len(), REDACTED_LEN);
    assert!(output.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_clone_produces_identical_redactor() {
    let custom_secrets = RapidSecrets::seed(2468);
    let original = RapidHashRedactor::with_secrets(custom_secrets);
    let cloned = original.clone();

    assert_eq!(original, cloned);

    let data_class = DataClass::new("test_taxonomy", "test_class");
    let input = "test_input";

    let mut output1 = String::new();
    let mut output2 = String::new();

    original.redact(&data_class, input, &mut output1).unwrap();
    cloned.redact(&data_class, input, &mut output2).unwrap();

    assert_eq!(output1, output2);
}

#[test]
fn test_data_class_does_not_affect_output() {
    let redactor = get_test_redactor();
    let data_class1 = DataClass::new("test_taxonomy", "class1");
    let data_class2 = DataClass::new("test_taxonomy", "class2");
    let input = "test_input";

    let mut output1 = String::new();
    let mut output2 = String::new();

    redactor.redact(&data_class1, input, &mut output1).unwrap();
    redactor.redact(&data_class2, input, &mut output2).unwrap();

    // The data_class parameter is ignored in the redaction process
    assert_eq!(output1, output2);
}
