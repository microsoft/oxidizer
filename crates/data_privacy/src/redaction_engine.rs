// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::redactors::Redactors;
use crate::{Classified, DataClass};
use core::fmt::{Debug, Display};
use std::io::{Cursor, Write};
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
/// use std::fmt::Write;
///
/// use data_privacy::common_taxonomy::{CommonTaxonomy, Sensitive};
/// use data_privacy::{RedactionEngineBuilder, Redactor, SimpleRedactor, SimpleRedactorMode};
///
/// struct Person {
///     name: Sensitive<String>, // a bit of sensitive data we should not leak in logs
///     age: u32,
/// }
///
/// fn try_out() {
///     let person = Person {
///         name: "John Doe".to_string().into(),
///         age: 30,
///     };
///
///     let asterisk_redactor = SimpleRedactor::new();
///     let erasing_redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Erase);
///
///     // Create the redaction engine. This is typically done once when the application starts.
///     let engine = RedactionEngineBuilder::new()
///         .add_class_redactor(&CommonTaxonomy::Sensitive.data_class(), asterisk_redactor)
///         .set_fallback_redactor(erasing_redactor)
///         .build();
///
///     let mut output_buffer = String::new();
///
///     engine.redacted_display(&person.name, |s| output_buffer.write_str(s).unwrap());
///
///     // check that the data in the output buffer has indeed been redacted as expected.
///     assert_eq!(output_buffer, "********");
/// }
/// #
/// # fn main() {
/// #     try_out();
/// # }
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

    /// Redacts the output of a classified value's [`Debug`] trait.
    ///
    /// Given a classified value whose payload implements the [`Debug`] trait, this method
    /// redacts the output of that trait using the redactor registered for the data class of the value.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Converting from u64 to usize, value is known to be <= 128"
    )]
    pub fn redacted_debug<C>(&self, value: &C, output: impl FnMut(&str))
    where
        C: Classified,
        C::Payload: Debug,
    {
        value.visit(|v| {
            let mut local_buf = [0u8; 128];
            let amount = {
                let mut cursor = Cursor::new(&mut local_buf[..]);
                if write!(&mut cursor, "{v:?}").is_ok() {
                    cursor.position() as usize
                } else {
                    local_buf.len() + 1 // force fallback case on write errors
                }
            };

            if amount <= local_buf.len() {
                // SAFETY: We know the buffer contains valid UTF-8 because the Debug impl can only write valid UTF-8.
                let s = unsafe { core::str::from_utf8_unchecked(&local_buf[..amount]) };

                self.redact(&value.data_class(), s, output);
            } else {
                // If the value is too large to fit in the buffer, we fall back to using the Debug format directly.
                self.redact(&value.data_class(), format!("{v:?}"), output);
            }
        });
    }

    /// Redacts the output of a classified value's [`Display`] trait.
    ///
    /// Given a classified value whose payload implements the [`Display`] trait, this method
    /// redacts the output of that trait using the redactor registered for the data class of the value.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Converting from u64 to usize, value is known to be <= 128"
    )]
    pub fn redacted_display<C>(&self, value: &C, output: impl FnMut(&str))
    where
        C: Classified,
        C::Payload: Display,
    {
        value.visit(|v| {
            let mut local_buf = [0u8; 128];
            let amount = {
                let mut cursor = Cursor::new(&mut local_buf[..]);
                if write!(&mut cursor, "{v}").is_ok() {
                    cursor.position() as usize
                } else {
                    local_buf.len() + 1 // force fallback case on write errors
                }
            };

            if amount <= local_buf.len() {
                // SAFETY: We know the buffer contains valid UTF-8 because the Display impl can only write valid UTF-8.
                let s = unsafe { core::str::from_utf8_unchecked(&local_buf[..amount]) };

                self.redact(&value.data_class(), s, output);
            } else {
                // If the value is too large to fit in the buffer, we fall back to using the Display format directly.
                self.redact(&value.data_class(), format!("{v}"), output);
            }
        });
    }

    /// Redacts the output of a classified value's [`ToString`] trait and returns it as a `String`.
    ///
    /// Given a classified value whose payload implements the [`ToString`] trait, this method
    /// redacts the output of that trait using the redactor registered for the data class of the value.
    #[must_use]
    pub fn to_redacted_string<C>(&self, value: &C) -> String
    where
        C: Classified,
        C::Payload: ToString,
    {
        let mut output = String::new();
        self.redact(&value.data_class(), value.as_declassified().to_string(), |s| output.push_str(s));
        output
    }

    /// Redacts a string with an explicit data classification, sending the results to the output callback.
    pub fn redact(&self, data_class: &DataClass, value: impl AsRef<str>, mut output: impl FnMut(&str)) {
        let redactor = self.redactors.get_or_fallback(data_class);
        redactor.redact(data_class, value.as_ref(), &mut output);
    }

    /// The exact length of the redacted output if it is a constant.
    ///
    /// This can be used as a hint to optimize buffer allocations.
    #[must_use]
    pub fn exact_len(&self, data_class: &DataClass) -> Option<usize> {
        let redactor = self.redactors.get_or_fallback(data_class);
        redactor.exact_len()
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
    use crate::common_taxonomy::{CommonTaxonomy, Insensitive, Sensitive, UnknownSensitivity};
    use crate::{RedactionEngineBuilder, SimpleRedactor, SimpleRedactorMode, taxonomy};
    use core::fmt::Write;
    use data_privacy_macros::classified;

    #[taxonomy(test, serde = false)]
    enum TestTaxonomy {
        Personal,
    }

    fn create_test_redactor(mode: SimpleRedactorMode) -> SimpleRedactor {
        SimpleRedactor::with_mode(mode)
    }

    fn collect_output<C>(engine: &RedactionEngine, value: &C) -> String
    where
        C: Classified,
        C::Payload: Display,
    {
        let mut output = String::new();
        engine.redacted_display(value, |s| output.push_str(s));
        output
    }

    fn collect_output_as_class(engine: &RedactionEngine, data_class: &DataClass, value: &str) -> String {
        let mut output = String::new();
        engine.redact(data_class, value, |s| output.push_str(s));
        output
    }

    #[test]
    fn test_new_creates_engine_with_redactors() {
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

        let mut redactors = Redactors::default();
        redactors.set_fallback(fallback_redactor);

        let engine = RedactionEngine::new(redactors);

        // Test that the engine was created successfully. One redactor is the passthrough for Insensitive which is set by default.
        assert_eq!(engine.redactors.len(), 1);
    }

    #[test]
    fn test_redact_uses_specific_redactor_for_registered_class() {
        let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

        let mut redactors = Redactors::default();
        redactors.insert(CommonTaxonomy::Sensitive.data_class(), asterisk_redactor);
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
        redactors.insert(CommonTaxonomy::Sensitive.data_class(), asterisk_redactor);
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
        redactors.insert(CommonTaxonomy::Sensitive.data_class(), asterisk_redactor);
        redactors.set_fallback(fallback_redactor);

        let engine = RedactionEngine::new(redactors);

        let result = collect_output_as_class(&engine, &Sensitive::<()>::data_class(), "confidential");

        assert_eq!(result, "************"); // Should use asterisk redactor
    }

    #[test]
    fn test_redact_as_class_uses_fallback_for_unknown_class() {
        let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Replace('?'));

        let mut redactors = Redactors::default();
        redactors.insert(CommonTaxonomy::Sensitive.data_class(), asterisk_redactor);
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
        redactors.insert(CommonTaxonomy::Sensitive.data_class(), asterisk_redactor);
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
        redactors.insert(CommonTaxonomy::Sensitive.data_class(), insert_redactor);
        redactors.insert(CommonTaxonomy::UnknownSensitivity.data_class(), passthrough_redactor);
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
        assert_eq!(unclassified_result, "account123"); // Set by default by redactors
        assert_eq!(personal_result, ""); // Uses fallback (erase)
    }

    #[test]
    fn test_redact_with_empty_string() {
        let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

        let mut redactors = Redactors::default();
        redactors.insert(CommonTaxonomy::Sensitive.data_class(), asterisk_redactor);
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
        redactors.insert(CommonTaxonomy::Sensitive.data_class(), asterisk_redactor);
        redactors.set_fallback(fallback_redactor);

        let engine = RedactionEngine::new(redactors);

        let result = collect_output_as_class(&engine, &CommonTaxonomy::Sensitive.data_class(), "");

        assert_eq!(result, ""); // Empty string should remain empty
    }

    #[test]
    fn test_multiple_output_calls() {
        let passthrough_redactor = create_test_redactor(SimpleRedactorMode::Passthrough);
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

        let mut redactors = Redactors::default();
        redactors.insert(CommonTaxonomy::Sensitive.data_class(), passthrough_redactor);
        redactors.set_fallback(fallback_redactor);

        let engine = RedactionEngine::new(redactors);

        let sensitive_data = Sensitive::new("hello world".to_string());
        let mut call_count = 0;
        let mut total_output = String::new();

        engine.redacted_display(&sensitive_data, |s| {
            call_count += 1;
            total_output.push_str(s);
        });

        assert_eq!(call_count, 1);
        assert_eq!(total_output, "hello world");
    }

    struct Person {
        name: Sensitive<String>, // a bit of sensitive data we should not leak in logs
    }

    #[classified(CommonTaxonomy::Sensitive)]
    struct Datum(String);

    #[test]
    fn test_basic() {
        let person = Person {
            name: "John Doe".to_string().into(),
        };

        let asterisk_redactor = SimpleRedactor::new();
        let erasing_redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Erase);

        let engine = RedactionEngineBuilder::new()
            .add_class_redactor(&CommonTaxonomy::Sensitive.data_class(), asterisk_redactor)
            .set_fallback_redactor(erasing_redactor)
            .build();

        let mut output_buffer = String::new();

        engine.redacted_display(&person.name, |s| output_buffer.write_str(s).unwrap());

        assert_eq!(None, engine.exact_len(&CommonTaxonomy::Sensitive.data_class()));
        assert_eq!(output_buffer, "********");

        output_buffer.clear();
        engine.redacted_debug(&person.name, |s| output_buffer.write_str(s).unwrap());
        assert_eq!(output_buffer, "**********");

        let d = Datum("A piece of data".to_string());
        output_buffer.clear();
        engine.redacted_debug(&d, |s| output_buffer.write_str(s).unwrap());
        assert_eq!(output_buffer, "*****************");
    }

    #[test]
    fn test_simple() {
        let person = Person {
            name: "John Doe".to_string().into(),
        };

        let tagging_redactor = SimpleRedactor::with_mode(SimpleRedactorMode::PassthroughAndTag);
        let erasing_redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Erase);

        let engine = RedactionEngineBuilder::new()
            .add_class_redactor(&CommonTaxonomy::Sensitive.data_class(), tagging_redactor)
            .set_fallback_redactor(erasing_redactor)
            .build();

        let mut output_buffer = String::new();

        engine.redacted_display(&person.name, |s| output_buffer.write_str(s).unwrap());

        assert_eq!(None, engine.exact_len(&CommonTaxonomy::Sensitive.data_class()));
        assert_eq!(output_buffer, "<common/sensitive:John Doe>");

        output_buffer.clear();
        engine.redacted_debug(&person.name, |s| output_buffer.write_str(s).unwrap());
        assert_eq!(output_buffer, "<common/sensitive:\"John Doe\">");
    }

    #[test]
    fn test_debug_trait_implementation() {
        let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
        let hash_redactor = create_test_redactor(SimpleRedactorMode::Replace('#'));
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

        let mut redactors = Redactors::default();
        redactors.insert(CommonTaxonomy::Sensitive.data_class(), asterisk_redactor);
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

        // Should be an empty debug list
        assert_eq!(debug_output, r#"[DataClass { taxonomy: "common", name: "insensitive" }]"#);
    }

    #[test]
    fn test_exact_len_returns_correct_value_for_selected_redactor_type() {
        // Create different redactor types with known exact_len behavior
        let erase_redactor = create_test_redactor(SimpleRedactorMode::Erase);
        let replace_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
        let passthrough_redactor = create_test_redactor(SimpleRedactorMode::Passthrough);
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Insert("REDACTED".into()));

        let mut redactors = Redactors::default();
        redactors.insert(CommonTaxonomy::Sensitive.data_class(), erase_redactor);
        redactors.insert(CommonTaxonomy::Insensitive.data_class(), replace_redactor);
        redactors.insert(TestTaxonomy::Personal.data_class(), passthrough_redactor);
        redactors.set_fallback(fallback_redactor);

        let engine = RedactionEngine::new(redactors);

        // Test exact_len for Erase mode - should return Some(0)
        let erase_len = engine.exact_len(&Sensitive::<()>::data_class());
        assert_eq!(erase_len, Some(0), "Erase redactor should return Some(0)");

        // Test exact_len for Replace mode - should return None (depends on input length)
        let replace_len = engine.exact_len(&Insensitive::<()>::data_class());
        assert_eq!(replace_len, None, "Replace redactor should return None");

        // Test exact_len for Passthrough mode - should return None (depends on input length)
        let passthrough_len = engine.exact_len(&TestTaxonomy::Personal.data_class());
        assert_eq!(passthrough_len, None, "Passthrough redactor should return None");

        // Test exact_len for fallback redactor (Insert mode) - should return None
        let unknown_class = UnknownSensitivity::<()>::data_class();
        let fallback_len = engine.exact_len(&unknown_class);
        assert_eq!(fallback_len, None, "Insert redactor should return None");

        // Verify the actual behavior matches the exact_len hint
        let sensitive_data = Sensitive::new("test".to_string());
        let erase_result = collect_output(&engine, &sensitive_data);
        assert_eq!(erase_result.len(), erase_len.unwrap_or(0));

        let unknown_data = UnknownSensitivity::new("test".to_string());
        let fallback_result = collect_output(&engine, &unknown_data);
        // For Insert mode, the output is always "REDACTED" regardless of input
        assert_eq!(fallback_result, "REDACTED");
    }

    #[test]
    fn test_long_strings() {
        let engine = RedactionEngineBuilder::new()
            .add_class_redactor(
                &CommonTaxonomy::Sensitive.data_class(),
                SimpleRedactor::with_mode(SimpleRedactorMode::PassthroughAndTag),
            )
            .build();

        let long_string = "a".repeat(148);
        let classified_long_string: Sensitive<String> = long_string.clone().into();

        let mut output_buffer = String::new();
        engine.redacted_debug(&classified_long_string, |s| {
            output_buffer.push_str(s);
        });

        let expected_debug_output = format!("<common/sensitive:\"{long_string}\">");
        assert_eq!(output_buffer, expected_debug_output);

        output_buffer.clear();
        engine.redacted_display(&classified_long_string, |s| {
            output_buffer.push_str(s);
        });

        let expected_display_output = format!("<common/sensitive:{long_string}>");
        assert_eq!(output_buffer, expected_display_output);

        let result_string = engine.to_redacted_string(&classified_long_string);

        let expected_to_string_output = format!("<common/sensitive:{long_string}>");
        assert_eq!(result_string, expected_to_string_output);
    }

    #[test]
    fn test_default_creates_engine_with_passthrough_insecure_and_tagged_erase_fallback() {
        let engine = RedactionEngine::default();

        // Should have one passthrough redactor for Insensitive data class
        assert_eq!(engine.redactors.len(), 1);

        engine
            .redactors
            .get(&CommonTaxonomy::Insensitive.data_class())
            .expect("Should have a redactor for Insensitive data class");

        // Should use the tagged erase fallback for any other data class
        let test_data = Sensitive::new("secret data".to_string());
        let result = collect_output(&engine, &test_data);

        // Default fallback should be SimpleRedactor with Erase mode (empty string)
        assert_eq!(result, "*");

        // Test with unknown sensitivity as well
        let unknown_data = UnknownSensitivity::new("some data".to_string());
        let result = collect_output(&engine, &unknown_data);
        assert_eq!(result, "*");

        // Test of Insensitive data passthrough
        let insensitive_data = Insensitive::new("public data".to_string());
        let result = collect_output(&engine, &insensitive_data);
        assert_eq!(result, "public data"); // Should passthrough without redaction and tag
    }
}
