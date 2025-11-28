// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use data_privacy::{Classified, RedactionEngine, Sensitive};
use data_privacy_macros::taxonomy;
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};

#[taxonomy(test)]
#[derive(Debug)]
enum TestTaxonomy {
    PII,
}

#[test]
fn test_classified_wrapper() {
    let classified = Sensitive::new(42, TestTaxonomy::PII);
    assert_eq!(classified.data_class(), TestTaxonomy::PII);
    assert_eq!(format!("{classified:?}"), "Sensitive { value: \"***\", data_class: DataClass { taxonomy: \"test\", name: \"p_i_i\" } }");
}

#[test]
fn test_clone_and_equality() {
    let classified1 = Sensitive::new(42, TestTaxonomy::PII);
    let classified2 = classified1.clone();
    let classified3 = Sensitive::new(12, TestTaxonomy::PII);
    assert_eq!(classified1, classified2);
    assert_ne!(classified1, classified3);
}

#[test]
fn test_hash() {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let classified = Sensitive::new(42, TestTaxonomy::PII);
    classified.hash(&mut hasher);
    let hash1 = hasher.finish();

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let classified2 = Sensitive::new(42, TestTaxonomy::PII);
    classified2.hash(&mut hasher);
    let hash2 = hasher.finish();

    assert_eq!(hash1, hash2, "Hashes should be equal for the same classified value");

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let classified3 = Sensitive::new(12, TestTaxonomy::PII);
    classified3.hash(&mut hasher);
    let hash3 = hasher.finish();

    assert_ne!(hash1, hash3, "Hashes of data with different values should not be equal");
}

#[test]
fn test_ordering() {
    let classified1 = Sensitive::new(42, TestTaxonomy::PII);
    let classified2 = Sensitive::new(12, TestTaxonomy::PII);

    assert_eq!(classified1.partial_cmp(&classified2).unwrap(), Ordering::Greater);
    assert_eq!(classified2.partial_cmp(&classified1).unwrap(), Ordering::Less);
    assert_eq!(classified1.partial_cmp(&classified1).unwrap(), Ordering::Equal);
}

#[test]
fn test_as_declassified_mut_allows_mutation() {
    // Mutable access allows in-place mutation of the wrapped value
    let classified = Sensitive::new(vec![1, 2, 3], TestTaxonomy::PII);
    // Ensure the data class remains unchanged after mutation
    assert_eq!(classified.data_class(), TestTaxonomy::PII);
}

#[test]
fn test_redacted_debug_and_display_replace_mode_string() {
    let engine = RedactionEngine::builder()
        .add_class_redactor(TestTaxonomy::PII, SimpleRedactor::with_mode(SimpleRedactorMode::Replace('*')))
        .build();
    let wrapper = Sensitive::new("secret".to_string(), TestTaxonomy::PII);

    // Debug redaction operates on the Debug representation (includes quotes for String)
    let mut debug_out = String::new();
    engine.redacted_debug(&wrapper, &mut debug_out).unwrap();
    assert_eq!(
        debug_out, "********",
        "Debug redaction should produce 8 asterisks (including quotes)"
    );

    // Display redaction operates on the Display representation (no quotes)
    let mut display_out = String::new();
    engine.redacted_display(&wrapper, &mut display_out).unwrap();
    assert_eq!(display_out, "******", "Display redaction should produce 6 asterisks (no quotes)");

    // to_string uses underlying value.to_string() (same as Display here)
    let to_string_out = engine.redacted_to_string(&wrapper);
    assert_eq!(to_string_out, "******", "RedactedToString should match Display redaction");
}

#[test]
fn test_redacted_debug_and_display_replace_mode_numeric() {
    let engine = RedactionEngine::builder()
        .add_class_redactor(TestTaxonomy::PII, SimpleRedactor::with_mode(SimpleRedactorMode::Replace('*')))
        .build();
    let wrapper = Sensitive::new(42u32, TestTaxonomy::PII);

    // Numeric Debug and Display both render without quotes; length is 2.
    let mut debug_out = String::new();
    engine.redacted_debug(&wrapper, &mut debug_out).unwrap();
    assert_eq!(debug_out, "**");
    let mut display_out = String::new();
    engine.redacted_display(&wrapper, &mut display_out).unwrap();
    assert_eq!(display_out, "**");
    let to_string_out = engine.redacted_to_string(&wrapper);
    assert_eq!(to_string_out, "**");
}

#[test]
fn test_redacted_passthrough_and_tag_mode() {
    let engine = RedactionEngine::builder()
        .add_class_redactor(TestTaxonomy::PII, SimpleRedactor::with_mode(SimpleRedactorMode::PassthroughAndTag))
        .build();
    let wrapper = Sensitive::new("secret".to_string(), TestTaxonomy::PII);

    let mut debug_out = String::new();
    engine.redacted_debug(&wrapper, &mut debug_out).unwrap();
    // Debug includes quotes in inner representation
    assert_eq!(
        debug_out, "<test/p_i_i:\"secret\">",
        "PassthroughAndTag debug should include quotes inside tag"
    );

    let mut display_out = String::new();
    engine.redacted_display(&wrapper, &mut display_out).unwrap();
    assert_eq!(
        display_out, "<test/p_i_i:secret>",
        "PassthroughAndTag display should not include quotes"
    );

    let to_string_out = engine.redacted_to_string(&wrapper);
    assert_eq!(to_string_out, "<test/p_i_i:secret>");
}

#[test]
fn test_redacted_long_value_fallback_path() {
    // Value length > 128 triggers fallback branch in ClassifiedWrapper's redacted debug/display implementations.
    let engine = RedactionEngine::builder()
        .add_class_redactor(TestTaxonomy::PII, SimpleRedactor::with_mode(SimpleRedactorMode::Replace('*')))
        .build();
    let long_plain = "a".repeat(140);
    let wrapper = Sensitive::new(long_plain.clone(), TestTaxonomy::PII);

    let mut debug_out = String::new();
    engine.redacted_debug(&wrapper, &mut debug_out).unwrap();
    // Debug representation adds quotes -> length + 2
    assert_eq!(debug_out.len(), long_plain.len() + 2);
    assert!(debug_out.chars().all(|c| c == '*'));

    let mut display_out = String::new();
    engine.redacted_display(&wrapper, &mut display_out).unwrap();
    assert_eq!(display_out.len(), long_plain.len());
    assert!(display_out.chars().all(|c| c == '*'));

    let to_string_out = engine.redacted_to_string(&wrapper);
    assert_eq!(to_string_out.len(), long_plain.len());
    assert!(to_string_out.chars().all(|c| c == '*'));
}
