// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::redactors::Redactors;
use crate::{DataClass, RedactedDebug, RedactedDisplay, RedactedToString};
use core::fmt::Debug;
use std::fmt::{Display, Formatter, Write};
use std::sync::Arc;

/// Lets you apply redaction to classified data.
///
/// You use [`RedactionEngineBuilder`](crate::RedactionEngineBuilder) to create an instance of this type.
/// The builder lets you configure exactly which redactor to use to redact individual data classes encountered
/// while producing telemetry.
///
/// ## Example
///
/// ```rust
/// use core::fmt::Write;
/// use data_privacy::{classified, RedactionEngineBuilder, Redactor, SimpleRedactor, SimpleRedactorMode, taxonomy};
///
/// // The taxonomy defines the different data classes we will use in our application.
/// #[taxonomy(simple)]
/// enum SimpleTaxonomy {
///    Sensitive,
///    ExtremelySensitive,
/// }
///
/// // A struct holding some data classified as sensitive.
/// #[classified(SimpleTaxonomy::Sensitive)]
/// struct Name(String);
///
/// struct Person {
///     name: Name,     // a bit of sensitive data we should not leak in logs
///     age: u32,
/// }
///
/// let person = Person {
///     name: Name("John Doe".to_string()),
///     age: 30,
/// };
///
/// let asterisk_redactor = SimpleRedactor::new();
/// let erasing_redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Erase);
///
/// // Create the redaction engine. This is typically done once when the application starts.
/// let engine = RedactionEngineBuilder::new()
///     .add_class_redactor(&SimpleTaxonomy::Sensitive.data_class(), asterisk_redactor)
///     .set_fallback_redactor(erasing_redactor)
///     .build();
///
/// let mut output_buffer = String::new();
/// _ = engine.redacted_display(&person.name, &mut output_buffer);
///
/// // check that the data in the output buffer has indeed been redacted as expected.
/// assert_eq!(output_buffer, "********");
/// ```
#[derive(Clone, Default)]
pub struct RedactionEngine {
    redactors: Arc<Redactors>,
}

impl RedactionEngine {
    #[must_use]
    pub(crate) fn new(mut redactors: Redactors) -> Self {
        redactors.shrink();
        Self {
            redactors: Arc::new(redactors),
        }
    }

    /// Redacts a value implementing [`RedactedDebug`], sending the results to the output sink.
    ///
    /// # Errors
    ///
    /// This function should return [`Err`] if, and only if, the provided [`Formatter`] returns [`Err`]. String redaction is considered an infallible operation;
    /// this function only returns a [`std::fmt::Result`] because writing to the underlying stream might fail and it must provide a way to propagate the fact that an error
    /// has occurred back up the stack.
    pub fn redacted_debug(&self, value: &impl RedactedDebug, output: &mut impl Write) -> core::fmt::Result {
        struct DebugFormatter<'a, RD>
        where
            RD: RedactedDebug + ?Sized,
        {
            engine: &'a RedactionEngine,
            value: &'a RD,
        }

        impl<RD> Debug for DebugFormatter<'_, RD>
        where
            RD: RedactedDebug + ?Sized,
        {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                self.value.fmt(self.engine, f)
            }
        }

        let d = DebugFormatter { engine: self, value };
        write!(output, "{d:?}")
    }

    /// Redacts a value implementing [`RedactedDisplay`], sending the results to the output sink.
    ///
    /// # Errors
    ///
    /// This function should return [`Err`] if, and only if, the provided [`Formatter`] returns [`Err`]. String redaction is considered an infallible operation;
    /// this function only returns a [`std::fmt::Result`] because writing to the underlying stream might fail and it must provide a way to propagate the fact that an error
    /// has occurred back up the stack.
    pub fn redacted_display(&self, value: &impl RedactedDisplay, output: &mut impl Write) -> core::fmt::Result {
        struct DisplayFormatter<'a, RD>
        where
            RD: RedactedDisplay + ?Sized,
        {
            engine: &'a RedactionEngine,
            value: &'a RD,
        }

        impl<RD> Display for DisplayFormatter<'_, RD>
        where
            RD: RedactedDisplay + ?Sized,
        {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                self.value.fmt(self.engine, f)
            }
        }

        let d = DisplayFormatter { engine: self, value };
        write!(output, "{d}")
    }

    /// Redacts a value implementing [`RedactedToString`] and returns the redacted string.
    pub fn redacted_to_string(&self, value: &impl RedactedToString) -> String {
        value.to_string(self)
    }

    /// Redacts a string with an explicit data classification, sending the results to the output sink.
    ///
    /// # Errors
    ///
    /// This function should return [`Err`] if, and only if, the provided [`Formatter`] returns [`Err`]. String redaction is considered an infallible operation;
    /// this function only returns a [`std::fmt::Result`] because writing to the underlying stream might fail and it must provide a way to propagate the fact that an error
    /// has occurred back up the stack.
    pub fn redact(&self, data_class: &DataClass, value: impl AsRef<str>, output: &mut impl Write) -> core::fmt::Result {
        let redactor = self.redactors.get_or_fallback(data_class);
        redactor.redact(data_class, value.as_ref(), output)
    }
}

impl Debug for RedactionEngine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.redactors.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RedactionEngineBuilder, SimpleRedactor, SimpleRedactorMode, taxonomy};
    use data_privacy_macros::classified;

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
        engine.redacted_display(value, &mut output).unwrap();
        output
    }

    fn collect_output_as_class(engine: &RedactionEngine, data_class: &DataClass, value: &str) -> String {
        let mut output = String::new();
        engine.redact(data_class, value, &mut output).unwrap();
        output
    }

    #[test]
    fn test_new_creates_engine_with_redactors() {
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

        let mut redactors = Redactors::default();
        redactors.set_fallback(fallback_redactor);

        let engine = RedactionEngine::new(redactors);

        // Test that the engine was created successfully, with no registered redactors.
        assert_eq!(engine.redactors.len(), 0);
    }

    #[test]
    fn test_redact_uses_specific_redactor_for_registered_class() {
        let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

        let mut redactors = Redactors::default();
        redactors.insert(TestTaxonomy::Sensitive.data_class(), asterisk_redactor);
        redactors.set_fallback(fallback_redactor);

        let engine = RedactionEngine::new(redactors);

        let sensitive_data = Sensitive::new("secret".to_string());
        let result = collect_output(&engine, &sensitive_data);

        assert_eq!(result, "******"); // Should be asterisks, not erased
    }

    #[test]
    fn test_redact_uses_fallback_for_unregistered_class() {
        let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Replace('X'));

        let mut redactors = Redactors::default();
        redactors.insert(TestTaxonomy::Sensitive.data_class(), asterisk_redactor);
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
        redactors.insert(TestTaxonomy::Sensitive.data_class(), asterisk_redactor);
        redactors.set_fallback(fallback_redactor);

        let engine = RedactionEngine::new(redactors);

        let result = collect_output_as_class(&engine, &TestTaxonomy::Sensitive.data_class(), "confidential");

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
        let result = collect_output_as_class(&engine, &unknown_class, "data");

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
        redactors.insert(TestTaxonomy::Sensitive.data_class(), asterisk_redactor);
        redactors.set_fallback(fallback_redactor);

        let engine = RedactionEngine::new(redactors);

        let result = collect_output_as_class(&engine, &TestTaxonomy::Sensitive.data_class(), "");

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

        let engine = RedactionEngineBuilder::new()
            .add_class_redactor(&TestTaxonomy::Sensitive.data_class(), asterisk_redactor)
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

        let engine = RedactionEngineBuilder::new()
            .add_class_redactor(&TestTaxonomy::Sensitive.data_class(), tagging_redactor)
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
        let engine = RedactionEngineBuilder::new()
            .add_class_redactor(
                &TestTaxonomy::Sensitive.data_class(),
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
}
