// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt::{Debug, Display};
use std::collections::HashMap;
use std::io::{Cursor, Write};

use crate::{Classified, DataClass, Redactor, SimpleRedactor, SimpleRedactorMode};

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
///     engine.display_redacted(&person.name, |s| output_buffer.write_str(s).unwrap());
///
///     // check that the data in the output buffer has indeed been redacted as expected.
///     assert_eq!(output_buffer, "********");
/// }
/// #
/// # fn main() {
/// #     try_out();
/// # }
/// ```
pub struct RedactionEngine {
    redactors: HashMap<DataClass, Box<dyn Redactor + Send + Sync>>,
    fallback: Box<dyn Redactor + Send + Sync>,
}

impl RedactionEngine {
    #[must_use]
    pub(crate) fn new(
        mut redactors: HashMap<DataClass, Box<dyn Redactor + Send + Sync>>,
        fallback: Box<dyn Redactor + Send + Sync>,
    ) -> Self {
        redactors.shrink_to_fit();

        Self { redactors, fallback }
    }

    /// Redacts the output of a classified value's [`Debug`] trait.
    ///
    /// Given a classified value whose payload implements the [`Debug`] trait, this method will
    /// redact the output of that trait using the redactor registered for the data class of the value.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Converting from u64 to usize, value is known to be <= 128"
    )]
    pub fn debug_redacted<C, T>(&self, value: &C, output: impl FnMut(&str))
    where
        C: Classified<T>,
        T: Debug,
    {
        value.visit(|v| {
            let mut local_buf = [0u8; 128];
            let (written, amount) = {
                let mut cursor = Cursor::new(&mut local_buf[..]);
                (write!(&mut cursor, "{v:?}").is_ok(), cursor.position() as usize)
            };

            if written {
                // SAFETY: We know the buffer contains valid UTF-8 because the Debug impl can only write valid UTF-8.
                let s = unsafe { core::str::from_utf8_unchecked(&local_buf[..amount]) };

                self.redact(&value.data_class(), s, output);
            } else {
                // If the value is too large to fit in the buffer, we fall back to using the debug format directly.
                self.redact(&value.data_class(), format!("{v:?}"), output);
            }
        });
    }

    /// Redacts the output of a classified value's [`Display`] trait.
    ///
    /// Given a classified value whose payload implements the [`Display`] trait, this method will
    /// redact the output of that trait using the redactor registered for the data class of the value.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Converting from u64 to usize, value is known to be <= 128"
    )]
    pub fn display_redacted<C, T>(&self, value: &C, output: impl FnMut(&str))
    where
        C: Classified<T>,
        T: Display,
    {
        value.visit(|v| {
            let mut local_buf = [0u8; 128];
            let (written, amount) = {
                let mut cursor = Cursor::new(&mut local_buf[..]);
                (write!(&mut cursor, "{v}").is_ok(), cursor.position() as usize)
            };

            if written {
                // SAFETY: We know the buffer contains valid UTF-8 because the Debug impl can only write valid UTF-8.
                let s = unsafe { core::str::from_utf8_unchecked(&local_buf[..amount]) };

                self.redact(&value.data_class(), s, output);
            } else {
                // If the value is too large to fit in the buffer, we fall back to using the debug format directly.
                self.redact(&value.data_class(), format!("{v}"), output);
            }
        });
    }

    /// Redacts a string with an explicit data classification, sending the results to the output callback.
    pub fn redact(&self, data_class: &DataClass, value: impl AsRef<str>, mut output: impl FnMut(&str)) {
        let redactor = self.redactors.get(data_class).unwrap_or(&self.fallback);
        redactor.redact(data_class, value.as_ref(), &mut output);
    }

    /// The exact length of the redacted output if it is a constant.
    ///
    /// This can be used as a hint to optimize buffer allocations.
    #[must_use]
    pub fn exact_len(&self, data_class: &DataClass) -> Option<usize> {
        let redactor = self.redactors.get(data_class).unwrap_or(&self.fallback);
        redactor.exact_len()
    }
}

impl Default for RedactionEngine {
    fn default() -> Self {
        Self::new(HashMap::new(), Box::new(SimpleRedactor::with_mode(SimpleRedactorMode::Erase)))
    }
}

impl Debug for RedactionEngine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.redactors.keys()).finish()
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write;

    use super::*;
    use crate::common_taxonomy::{CommonTaxonomy, Insensitive, Sensitive, UnknownSensitivity};
    use crate::{RedactionEngineBuilder, taxonomy};

    #[taxonomy(test, serde = false)]
    enum TestTaxonomy {
        Personal,
    }

    fn create_test_redactor(mode: SimpleRedactorMode) -> SimpleRedactor {
        SimpleRedactor::with_mode(mode)
    }

    fn collect_output<C, T>(engine: &RedactionEngine, value: &C) -> String
    where
        C: Classified<T>,
        T: Display,
    {
        let mut output = String::new();
        engine.display_redacted(value, |s| output.push_str(s));
        output
    }

    fn collect_output_as_class(engine: &RedactionEngine, data_class: &DataClass, value: &str) -> String {
        let mut output = String::new();
        engine.redact(data_class, value, |s| output.push_str(s));
        output
    }

    #[test]
    fn test_new_creates_engine_with_redactors() {
        let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

        let mut redactors = HashMap::<DataClass, Box<dyn Redactor + Send + Sync>>::new();
        _ = redactors.insert(Sensitive::<()>::data_class(), Box::new(asterisk_redactor));

        let engine = RedactionEngine::new(redactors, Box::new(fallback_redactor));

        // Test that the engine was created successfully
        assert_eq!(engine.redactors.len(), 1);
    }

    #[test]
    fn test_redact_uses_specific_redactor_for_registered_class() {
        let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

        let mut redactors = HashMap::<DataClass, Box<dyn Redactor + Send + Sync>>::new();
        _ = redactors.insert(Sensitive::<()>::data_class(), Box::new(asterisk_redactor));

        let engine = RedactionEngine::new(redactors, Box::new(fallback_redactor));

        let sensitive_data = Sensitive::new("secret".to_string());
        let result = collect_output(&engine, &sensitive_data);

        assert_eq!(result, "******"); // Should be asterisks, not erased
    }

    #[test]
    fn test_redact_uses_fallback_for_unregistered_class() {
        let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Replace('X'));

        let mut redactors = HashMap::<DataClass, Box<dyn Redactor + Send + Sync>>::new();
        _ = redactors.insert(Sensitive::<()>::data_class(), Box::new(asterisk_redactor));

        let engine = RedactionEngine::new(redactors, Box::new(fallback_redactor));

        let unknown_data = UnknownSensitivity::new("john@example.com".to_string());
        let result = collect_output(&engine, &unknown_data);

        assert_eq!(result, "XXXXXXXXXXXXXXXX"); // Should use fallback redactor
    }

    #[test]
    fn test_redact_as_class_uses_specific_redactor() {
        let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

        let mut redactors = HashMap::<DataClass, Box<dyn Redactor + Send + Sync>>::new();
        _ = redactors.insert(Sensitive::<()>::data_class(), Box::new(asterisk_redactor));

        let engine = RedactionEngine::new(redactors, Box::new(fallback_redactor));

        let result = collect_output_as_class(&engine, &Sensitive::<()>::data_class(), "confidential");

        assert_eq!(result, "************"); // Should use asterisk redactor
    }

    #[test]
    fn test_redact_as_class_uses_fallback_for_unknown_class() {
        let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Replace('?'));

        let mut redactors = HashMap::<DataClass, Box<dyn Redactor + Send + Sync>>::new();
        _ = redactors.insert(Sensitive::<()>::data_class(), Box::new(asterisk_redactor));

        let engine = RedactionEngine::new(redactors, Box::new(fallback_redactor));

        let unknown_class = DataClass::new("unknown", "test");
        let result = collect_output_as_class(&engine, &unknown_class, "data");

        assert_eq!(result, "????"); // Should use fallback redactor
    }

    #[test]
    fn test_redact_with_multiple_redactors() {
        let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
        let hash_redactor = create_test_redactor(SimpleRedactorMode::Replace('#'));
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

        let mut redactors = HashMap::<DataClass, Box<dyn Redactor + Send + Sync>>::new();
        _ = redactors.insert(Sensitive::<()>::data_class(), Box::new(asterisk_redactor));
        _ = redactors.insert(TestTaxonomy::Personal.data_class(), Box::new(hash_redactor));

        let engine = RedactionEngine::new(redactors, Box::new(fallback_redactor));

        let sensitive_data = Sensitive::new("secret".to_string());
        let personal_data = Personal::new("email".to_string());

        let sensitive_result = collect_output(&engine, &sensitive_data);
        let personal_result = collect_output(&engine, &personal_data);

        assert_eq!(sensitive_result, "******");
        assert_eq!(personal_result, "#####");
    }

    #[test]
    fn test_redact_with_different_redactor_modes() {
        let insert_redactor = create_test_redactor(SimpleRedactorMode::Insert("[REDACTED]".to_string()));
        let passthrough_redactor = create_test_redactor(SimpleRedactorMode::Passthrough);
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

        let mut redactors = HashMap::<DataClass, Box<dyn Redactor + Send + Sync>>::new();
        _ = redactors.insert(Sensitive::<()>::data_class(), Box::new(insert_redactor));
        _ = redactors.insert(UnknownSensitivity::<()>::data_class(), Box::new(passthrough_redactor));

        let engine = RedactionEngine::new(redactors, Box::new(fallback_redactor));

        let sensitive_data = Sensitive::new("secret".to_string());
        let unknown_data = UnknownSensitivity::new("public".to_string());
        let unclassified_data = Insensitive::new("account123".to_string());

        let sensitive_result = collect_output(&engine, &sensitive_data);
        let unknown_result = collect_output(&engine, &unknown_data);
        let unclassified_result = collect_output(&engine, &unclassified_data);

        assert_eq!(sensitive_result, "[REDACTED]");
        assert_eq!(unknown_result, "public");
        assert_eq!(unclassified_result, ""); // Uses fallback (erase)
    }

    #[test]
    fn test_redact_with_empty_string() {
        let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

        let mut redactors = HashMap::<DataClass, Box<dyn Redactor + Send + Sync>>::new();
        _ = redactors.insert(Sensitive::<()>::data_class(), Box::new(asterisk_redactor));

        let engine = RedactionEngine::new(redactors, Box::new(fallback_redactor));

        let empty_data = Sensitive::new(String::new());
        let result = collect_output(&engine, &empty_data);

        assert_eq!(result, ""); // Empty string should remain empty
    }

    #[test]
    fn test_redact_as_class_with_empty_string() {
        let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

        let mut redactors = HashMap::<DataClass, Box<dyn Redactor + Send + Sync>>::new();
        _ = redactors.insert(Sensitive::<()>::data_class(), Box::new(asterisk_redactor));

        let engine = RedactionEngine::new(redactors, Box::new(fallback_redactor));

        let result = collect_output_as_class(&engine, &CommonTaxonomy::Sensitive.data_class(), "");

        assert_eq!(result, ""); // Empty string should remain empty
    }

    #[test]
    fn test_multiple_output_calls() {
        let passthrough_redactor = create_test_redactor(SimpleRedactorMode::Passthrough);
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

        let mut redactors = HashMap::<DataClass, Box<dyn Redactor + Send + Sync>>::new();
        _ = redactors.insert(Sensitive::<()>::data_class(), Box::new(passthrough_redactor));

        let engine = RedactionEngine::new(redactors, Box::new(fallback_redactor));

        let sensitive_data = Sensitive::new("hello world".to_string());
        let mut call_count = 0;
        let mut total_output = String::new();

        engine.display_redacted(&sensitive_data, |s| {
            call_count += 1;
            total_output.push_str(s);
        });

        assert_eq!(call_count, 1);
        assert_eq!(total_output, "hello world");
    }

    struct Person {
        name: Sensitive<String>, // a bit of sensitive data we should not leak in logs
    }

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

        engine.display_redacted(&person.name, |s| output_buffer.write_str(s).unwrap());

        assert_eq!(None, engine.exact_len(&CommonTaxonomy::Sensitive.data_class()));
        assert_eq!(output_buffer, "********");

        output_buffer.clear();
        engine.debug_redacted(&person.name, |s| output_buffer.write_str(s).unwrap());
        assert_eq!(output_buffer, "**********");
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

        engine.display_redacted(&person.name, |s| output_buffer.write_str(s).unwrap());

        assert_eq!(None, engine.exact_len(&CommonTaxonomy::Sensitive.data_class()));
        assert_eq!(output_buffer, "<common/sensitive:John Doe>");

        output_buffer.clear();
        engine.debug_redacted(&person.name, |s| output_buffer.write_str(s).unwrap());
        assert_eq!(output_buffer, "<common/sensitive:\"John Doe\">");
    }

    #[test]
    fn test_debug_trait_implementation() {
        let asterisk_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
        let hash_redactor = create_test_redactor(SimpleRedactorMode::Replace('#'));
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);

        let mut redactors = HashMap::<DataClass, Box<dyn Redactor + Send + Sync>>::new();
        _ = redactors.insert(Sensitive::<()>::data_class(), Box::new(asterisk_redactor));
        _ = redactors.insert(TestTaxonomy::Personal.data_class(), Box::new(hash_redactor));

        let engine = RedactionEngine::new(redactors, Box::new(fallback_redactor));

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
    fn test_debug_trait_with_empty_redactors() {
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Erase);
        let redactors = HashMap::<DataClass, Box<dyn Redactor + Send + Sync>>::new();

        let engine = RedactionEngine::new(redactors, Box::new(fallback_redactor));

        // Test the Debug trait implementation with no redactors
        let debug_output = format!("{engine:?}");

        // Should be an empty debug list
        assert_eq!(debug_output, "[]");
    }

    #[test]
    fn test_exact_len_returns_correct_value_for_selected_redactor_type() {
        // Create different redactor types with known exact_len behavior
        let erase_redactor = create_test_redactor(SimpleRedactorMode::Erase);
        let replace_redactor = create_test_redactor(SimpleRedactorMode::Replace('*'));
        let passthrough_redactor = create_test_redactor(SimpleRedactorMode::Passthrough);
        let fallback_redactor = create_test_redactor(SimpleRedactorMode::Insert("REDACTED".to_string()));

        let mut redactors = HashMap::<DataClass, Box<dyn Redactor + Send + Sync>>::new();
        _ = redactors.insert(Sensitive::<()>::data_class(), Box::new(erase_redactor));
        _ = redactors.insert(Insensitive::<()>::data_class(), Box::new(replace_redactor));
        _ = redactors.insert(TestTaxonomy::Personal.data_class(), Box::new(passthrough_redactor));

        let engine = RedactionEngine::new(redactors, Box::new(fallback_redactor));

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
        engine.debug_redacted(&classified_long_string, |s| {
            output_buffer.push_str(s);
        });

        let expected_debug_output = format!("<common/sensitive:\"{long_string}\">");
        assert_eq!(output_buffer, expected_debug_output);

        output_buffer.clear();
        engine.display_redacted(&classified_long_string, |s| {
            output_buffer.push_str(s);
        });

        let expected_display_output = format!("<common/sensitive:{long_string}>");
        assert_eq!(output_buffer, expected_display_output);
    }

    #[test]
    fn test_default_creates_engine_with_empty_redactors_and_erase_fallback() {
        let engine = RedactionEngine::default();

        // Should have no specific redactors
        assert_eq!(engine.redactors.len(), 0);

        // Should use the erase fallback for any data class
        let test_data = Sensitive::new("secret data".to_string());
        let result = collect_output(&engine, &test_data);

        // Default fallback should be SimpleRedactor with Erase mode (empty string)
        assert_eq!(result, "");

        // Test with unknown sensitivity as well
        let unknown_data = UnknownSensitivity::new("some data".to_string());
        let result = collect_output(&engine, &unknown_data);
        assert_eq!(result, "");
    }
}
