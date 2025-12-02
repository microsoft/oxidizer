// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use data_privacy::{DataClass, IntoDataClass, RedactedDisplay, RedactionEngine};
use data_privacy_macros::{classified, taxonomy};

#[taxonomy(test)]
enum TestTaxonomy {
    Personal,
    Sensitive,
    Insensitive,
    UnknownSensitivity,
}

#[classified(TestTaxonomy::Personal)]
struct Personal(String);
impl Personal {
    fn new(payload: String) -> Self {
        Self(payload)
    }
}

#[classified(TestTaxonomy::Sensitive)]
struct Sensitive(String);
impl Sensitive {
    fn new(payload: String) -> Self {
        Self(payload)
    }
}

#[classified(TestTaxonomy::Insensitive)]
struct Insensitive(String);
impl Insensitive {
    fn new(payload: String) -> Self {
        Self(payload)
    }
}

#[classified(TestTaxonomy::UnknownSensitivity)]
struct UnknownSensitivity(String);
impl UnknownSensitivity {
    fn new(payload: String) -> Self {
        Self(payload)
    }
}

fn create_test_redactor(mode: SimpleRedactorMode) -> SimpleRedactor {
    SimpleRedactor::with_mode(mode)
}

fn collect_output<C>(engine: &RedactionEngine, value: &C) -> String
where
    C: RedactedDisplay,
{
    let mut output = String::new();
    engine
        .redacted_display(value, &mut output)
        .expect("redacted_display should succeed in tests");
    output
}

fn collect_output_as_class(engine: &RedactionEngine, data_class: impl IntoDataClass, value: &str) -> String {
    let mut output = String::new();
    engine
        .redact(data_class.into_data_class(), value, &mut output)
        .expect("redact should succeed in tests");
    output
}

#[test]
fn test_redact_uses_specific_redactor_for_registered_class() {
    let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
    let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

    let engine = RedactionEngine::builder()
        .add_class_redactor(TestTaxonomy::Sensitive, asterisk_redactor)
        .set_fallback_redactor(fallback_redactor)
        .build();

    let sensitive_data = Sensitive::new("secret".to_string());
    let result = collect_output(&engine, &sensitive_data);

    assert_eq!(result, "******"); // Should be asterisks, not erased
}

#[test]
fn test_redact_uses_fallback_for_unregistered_class() {
    let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
    let fallback_redactor = create_test_redactor(SimpleRedactorMode::Replace('X'));

    let mut redactors = Redactors::default();
    redactors.insert(TestTaxonomy::Sensitive, asterisk_redactor);
    redactors.set_fallback(fallback_redactor);

    let engine = RedactionEngine::new(redactors);

    let unknown_data = UnknownSensitivity::new("john@example.com".to_string());
    let result = collect_output(&engine, &unknown_data);

    assert_eq!(result, "XXXXXXXXXXXXXXXX"); // Should use fallback redactor
}

#[test]
fn test_redact_as_class_uses_specific_redactor() {
    let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
    let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

    let mut redactors = Redactors::default();
    redactors.insert(TestTaxonomy::Sensitive, asterisk_redactor);
    redactors.set_fallback(fallback_redactor);

    let engine = RedactionEngine::new(redactors);

    let result = collect_output_as_class(&engine, TestTaxonomy::Sensitive, "confidential");

    assert_eq!(result, "************"); // Should use asterisk redactor
}

#[test]
fn test_redact_as_class_uses_fallback_for_unknown_class() {
    let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
    let fallback_redactor = create_test_redactor(SimpleRedactorMode::Replace('?'));

    let mut redactors = Redactors::default();
    redactors.insert(TestTaxonomy::Sensitive.data_class(), asterisk_redactor);
    redactors.set_fallback(fallback_redactor);

    let engine = RedactionEngine::new(redactors);

    let unknown_class = DataClass::new("unknown", "test");
    let result = collect_output_as_class(&engine, unknown_class, "data");

    assert_eq!(result, "????"); // Should use fallback redactor
}

#[test]
fn test_redact_with_multiple_redactors() {
    let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
    let hash_redactor = create_test_redactor(SimpleRedactorMode::Replace('#'));
    let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

    let mut redactors = Redactors::default();
    redactors.insert(TestTaxonomy::Sensitive.data_class(), asterisk_redactor);
    redactors.insert(TestTaxonomy::Personal.data_class(), hash_redactor);
    redactors.set_fallback(fallback_redactor);

    let engine = RedactionEngine::new(redactors);

    let sensitive_data = Sensitive::new("secret".to_string());
    let personal_data = Personal::new("email".to_string());

    let sensitive_result = collect_output(&engine, &sensitive_data);
    let personal_result = collect_output(&engine, &personal_data);

    assert_eq!(sensitive_result, "******");
    assert_eq!(personal_result, "#####");
}

#[test]
fn test_redact_with_different_redactor_modes() {
    let insert_redactor = create_test_redactor(SimpleRedactorMode::Insert("[REDACTED]".into()));
    let passthrough_redactor = create_test_redactor(SimpleRedactorMode::Passthrough);
    let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

    let mut redactors = Redactors::default();
    redactors.insert(TestTaxonomy::Sensitive.data_class(), insert_redactor);
    redactors.insert(TestTaxonomy::UnknownSensitivity.data_class(), passthrough_redactor);
    redactors.set_fallback(fallback_redactor);

    let engine = RedactionEngine::new(redactors);

    let sensitive_data = Sensitive::new("secret".to_string());
    let unknown_data = UnknownSensitivity::new("public".to_string());
    let unclassified_data = Insensitive::new("account123".to_string());
    let personal_data = Personal::new("username".to_string());

    let sensitive_result = collect_output(&engine, &sensitive_data);
    let unknown_result = collect_output(&engine, &unknown_data);
    let unclassified_result = collect_output(&engine, &unclassified_data);
    let personal_result = collect_output(&engine, &personal_data);

    assert_eq!(sensitive_result, "[REDACTED]");
    assert_eq!(unknown_result, "public");
    assert_eq!(unclassified_result, "");
    assert_eq!(personal_result, ""); // Uses fallback (erase)
}

#[test]
fn test_redact_with_empty_string() {
    let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
    let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

    let mut redactors = Redactors::default();
    redactors.insert(TestTaxonomy::Sensitive.data_class(), asterisk_redactor);
    redactors.set_fallback(fallback_redactor);

    let engine = RedactionEngine::new(redactors);

    let empty_data = Sensitive::new(String::new());
    let result = collect_output(&engine, &empty_data);

    assert_eq!(result, ""); // Empty string should remain empty
}

#[test]
fn test_redact_as_class_with_empty_string() {
    let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
    let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

    let mut redactors = Redactors::default();
    redactors.insert(TestTaxonomy::Sensitive, asterisk_redactor);
    redactors.set_fallback(fallback_redactor);

    let engine = RedactionEngine::new(redactors);

    let result = collect_output_as_class(&engine, TestTaxonomy::Sensitive, "");

    assert_eq!(result, ""); // Empty string should remain empty
}

struct Person {
    name: Sensitive, // a bit of sensitive data we should not leak in logs
}

#[classified(TestTaxonomy::Sensitive)]
struct Datum(String);

#[test]
fn test_basic() {
    let person = Person {
        name: Sensitive::new("John Doe".to_string()),
    };

    let asterisk_redactor = SimpleRedactor::new();
    let erasing_redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Erase);

    let engine = RedactionEngine::builder()
        .add_class_redactor(TestTaxonomy::Sensitive, asterisk_redactor)
        .set_fallback_redactor(erasing_redactor)
        .build();

    let mut output_buffer = String::new();

    engine.redacted_display(&person.name, &mut output_buffer).unwrap();
    assert_eq!(output_buffer, "********");

    output_buffer.clear();
    engine.redacted_debug(&person.name, &mut output_buffer).unwrap();
    assert_eq!(output_buffer, "**********");

    let d = Datum("A piece of data".to_string());
    output_buffer.clear();
    engine.redacted_debug(&d, &mut output_buffer).unwrap();
    assert_eq!(output_buffer, "*****************");
}

#[test]
fn test_simple() {
    let person = Person {
        name: Sensitive::new("John Doe".to_string()),
    };

    let tagging_redactor = SimpleRedactor::with_mode(SimpleRedactorMode::PassthroughAndTag);
    let erasing_redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Erase);

    let engine = RedactionEngine::builder()
        .add_class_redactor(TestTaxonomy::Sensitive, tagging_redactor)
        .set_fallback_redactor(erasing_redactor)
        .build();

    let mut output_buffer = String::new();

    engine.redacted_display(&person.name, &mut output_buffer).unwrap();
    assert_eq!(output_buffer, "<test/sensitive:John Doe>");

    output_buffer.clear();
    engine.redacted_debug(&person.name, &mut output_buffer).unwrap();
    assert_eq!(output_buffer, "<test/sensitive:\"John Doe\">");
}

#[test]
fn test_debug_trait_implementation() {
    let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
    let hash_redactor = create_test_redactor(SimpleRedactorMode::Replace('#'));
    let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

    let mut redactors = Redactors::default();
    redactors.insert(TestTaxonomy::Sensitive.data_class(), asterisk_redactor);
    redactors.insert(TestTaxonomy::Personal.data_class(), hash_redactor);
    redactors.set_fallback(fallback_redactor);

    let engine = RedactionEngine::new(redactors);

    // Test the Debug trait implementation
    let debug_output = format!("{engine:?}");

    // The Debug implementation should show a list of registered data class keys
    // Since HashMap iteration order is not guaranteed, we need to check that both keys are present
    assert!(debug_output.contains("sensitive") || debug_output.contains("Sensitive"));
    assert!(debug_output.contains("personal") || debug_output.contains("Personal"));

    // Should be formatted as a debug list (starts with [ and ends with ])
    assert!(debug_output.starts_with('['));
    assert!(debug_output.ends_with(']'));
}

#[test]
fn test_debug_trait_with_default_redactors() {
    let redactors = Redactors::default();

    let engine = RedactionEngine::new(redactors);

    // Test the Debug trait implementation with no redactors
    let debug_output = format!("{engine:?}");

    assert_eq!(debug_output, "[]");
}

#[test]
fn test_long_strings() {
    let engine = RedactionEngine::builder()
        .add_class_redactor(
            TestTaxonomy::Sensitive,
            SimpleRedactor::with_mode(SimpleRedactorMode::PassthroughAndTag),
        )
        .build();

    let long_string = "a".repeat(148);
    let classified_long_string = Sensitive::new(long_string.clone());

    let mut output_buffer = String::new();
    engine.redacted_debug(&classified_long_string, &mut output_buffer).unwrap();

    let expected_debug_output = format!("<test/sensitive:\"{long_string}\">");
    assert_eq!(output_buffer, expected_debug_output);

    output_buffer.clear();
    engine.redacted_display(&classified_long_string, &mut output_buffer).unwrap();

    let expected_display_output = format!("<test/sensitive:{long_string}>");
    assert_eq!(output_buffer, expected_display_output);

    let result_string = engine.redacted_to_string(&classified_long_string);

    let expected_to_string_output = format!("<test/sensitive:{long_string}>");
    assert_eq!(result_string, expected_to_string_output);
}

fn test_redaction(engine: &RedactionEngine, data_class: &DataClass, input: &str, expected: &str) {
    let mut output = String::new();
    engine
        .redact(data_class, input, &mut output)
        .expect("redact should succeed in tests");
    assert_eq!(output, expected);
}

#[test]
fn new_creates_builder_with_default_values() {
    let engine = RedactionEngine::builder().build();
    test_redaction(&engine, &DataClass::new("test_taxonomy", "test_class"), "sensitive data", "*");

    let engine = RedactionEngine::builder().build();
    test_redaction(&engine, &DataClass::new("test_taxonomy", "test_class"), "sensitive data", "*");
}

#[test]
fn add_multiple_class_redactors() {
    let redactor1 = SimpleRedactor::with_mode(SimpleRedactorMode::Insert("XX".into()));
    let redactor2 = SimpleRedactor::with_mode(SimpleRedactorMode::Insert("YY".into()));

    let data_class1 = DataClass::new("taxonomy", "class1");
    let data_class2 = DataClass::new("taxonomy", "class2");
    let data_class3 = DataClass::new("taxonomy", "class3");

    let engine = RedactionEngine::builder()
        .add_class_redactor(data_class1.clone(), redactor1)
        .add_class_redactor(data_class2.clone(), redactor2)
        .build();

    test_redaction(&engine, &data_class1, "sensitive data", "XX");
    test_redaction(&engine, &data_class2, "sensitive data", "YY");
    test_redaction(&engine, &data_class3, "sensitive data", "*");
}

#[test]
fn set_fallback_redactor_overwrites_default() {
    let redactor1 = SimpleRedactor::with_mode(SimpleRedactorMode::Insert("XX".into()));
    let redactor2 = SimpleRedactor::with_mode(SimpleRedactorMode::Insert("YY".into()));
    let redactor3 = SimpleRedactor::with_mode(SimpleRedactorMode::Insert("ZZ".into()));

    let data_class1 = DataClass::new("taxonomy", "class1");
    let data_class2 = DataClass::new("taxonomy", "class2");
    let data_class3 = DataClass::new("taxonomy", "class3");

    let engine = RedactionEngine::builder()
        .add_class_redactor(data_class1.clone(), redactor1)
        .add_class_redactor(data_class2.clone(), redactor2)
        .set_fallback_redactor(redactor3)
        .build();

    test_redaction(&engine, &data_class1, "sensitive data", "XX");
    test_redaction(&engine, &data_class2, "sensitive data", "YY");
    test_redaction(&engine, &data_class3, "sensitive data", "ZZ");
}

#[test]
fn debug_trait_implementation() {
    let redactor1 = SimpleRedactor::with_mode(SimpleRedactorMode::Insert("XX".into()));
    let redactor2 = SimpleRedactor::with_mode(SimpleRedactorMode::Insert("YY".into()));

    let data_class1 = DataClass::new("taxonomy", "class1");
    let data_class2 = DataClass::new("taxonomy", "class2");

    let engine = RedactionEngine::builder()
        .add_class_redactor(data_class1, redactor1)
        .add_class_redactor(data_class2, redactor2)
        .build();

    let debug_output = format!("{engine:?}");

    // The debug output should contain both data classes
    assert!(debug_output.contains("class1"));
    assert!(debug_output.contains("class2"));
    assert!(debug_output.contains("taxonomy"));

    // Test default builder debug output
    let default_builder = RedactionEngine::builder().build();
    let default_builder_debug_output = format!("{default_builder:?}");
    assert_eq!(default_builder_debug_output, "[]");
}
