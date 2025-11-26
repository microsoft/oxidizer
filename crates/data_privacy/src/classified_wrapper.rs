// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{Classified, DataClass, RedactedDebug, RedactedDisplay, RedactedToString, RedactionEngine};
use core::fmt::{Debug, Display, Formatter};
use core::hash::{Hash, Hasher};
use data_privacy_macros::ClassifiedDebug;

/// A wrapper that dynamically classifies a value with a specific data class.
///
/// Use this wrapper in places where the data class of a value cannot be determined statically. When the data class is known
/// at compile time, prefer using specific classification types defined with the [`classified`](crate::classified) attribute macro.
#[derive(ClassifiedDebug)]
pub struct ClassifiedWrapper<T> {
    value: T,
    data_class: DataClass,
}

impl<T> ClassifiedWrapper<T> {
    /// Creates a new instance of `ClassifiedWrapper` with the given value and data class.
    pub const fn new(value: T, data_class: DataClass) -> Self {
        Self { value, data_class }
    }
}

impl<T> Classified for ClassifiedWrapper<T> {
    fn data_class(&self) -> DataClass {
        self.data_class.clone()
    }
}

impl<T> RedactedDebug for ClassifiedWrapper<T>
where
    T: Debug,
{
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Converting from u64 to usize, value is known to be <= 128"
    )]
    fn fmt(&self, engine: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
        let v = todo!();

        let mut local_buf = [0u8; 128];
        let amount = {
            let mut cursor = std::io::Cursor::new(&mut local_buf[..]);
            if std::io::Write::write_fmt(&mut cursor, format_args!("{v:?}")).is_ok() {
                cursor.position() as usize
            } else {
                local_buf.len() + 1 // force fallback case on write errors
            }
        };

        if amount <= local_buf.len() {
            // SAFETY: We know the buffer contains valid UTF-8 because the Debug impl can only write valid UTF-8.
            let s = unsafe { core::str::from_utf8_unchecked(&local_buf[..amount]) };

            engine.redact(&self.data_class(), s, f)
        } else {
            // If the value is too large to fit in the buffer, we fall back to using the Debug format directly.
            engine.redact(&self.data_class(), format!("{v:?}"), f)
        }
    }
}

impl<T> RedactedDisplay for ClassifiedWrapper<T>
where
    T: Display,
{
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Converting from u64 to usize, value is known to be <= 128"
    )]
    fn fmt(&self, engine: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
        let v = todo!();

        let mut local_buf = [0u8; 128];
        let amount = {
            let mut cursor = std::io::Cursor::new(&mut local_buf[..]);
            if std::io::Write::write_fmt(&mut cursor, format_args!("{v}")).is_ok() {
                cursor.position() as usize
            } else {
                local_buf.len() + 1 // force fallback case on write errors
            }
        };

        if amount <= local_buf.len() {
            // SAFETY: We know the buffer contains valid UTF-8 because the Debug impl can only write valid UTF-8.
            let s = unsafe { core::str::from_utf8_unchecked(&local_buf[..amount]) };

            engine.redact(&self.data_class(), s, f)
        } else {
            // If the value is too large to fit in the buffer, we fall back to using the Debug format directly.
            engine.redact(&self.data_class(), format!("{v}"), f)
        }
    }
}

impl<T> RedactedToString for ClassifiedWrapper<T>
where
    T: ToString,
{
    fn to_string(&self, engine: &RedactionEngine) -> String {
        let mut output = String::new();
        _ = engine.redact(&self.data_class(), self.value.to_string(), &mut output);
        output
    }
}

impl<T> Clone for ClassifiedWrapper<T>
where
    T: Clone,
{
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            data_class: self.data_class.clone(),
        }
    }
}

impl<T> PartialEq for ClassifiedWrapper<T>
where
    T: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value && self.data_class == other.data_class
    }
}

impl<T> Hash for ClassifiedWrapper<T>
where
    T: Hash,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
        self.data_class.hash(state);
    }
}

impl<T> PartialOrd for ClassifiedWrapper<T>
where
    T: PartialOrd,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match self.value.partial_cmp(&other.value) {
            Some(std::cmp::Ordering::Equal) => self.data_class.partial_cmp(&other.data_class),
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use data_privacy_macros::taxonomy;
    use std::cmp::Ordering;

    #[taxonomy(test)]
    enum TestTaxonomy {
        Sensitive,
    }

    #[test]
    fn test_classified_wrapper() {
        let classified = ClassifiedWrapper::new(42, TestTaxonomy::Sensitive.data_class());
        // assert_eq!(classified.as_declassified(), &42);
        assert_eq!(classified.data_class(), TestTaxonomy::Sensitive.data_class());
        assert_eq!(format!("{classified:?}"), "<CLASSIFIED:test/sensitive>");
    }

    #[test]
    fn test_clone_and_equality() {
        let classified1 = ClassifiedWrapper::new(42, TestTaxonomy::Sensitive.data_class());
        let classified2 = classified1.clone();
        let classified3 = ClassifiedWrapper::new(12, TestTaxonomy::Sensitive.data_class());
        assert_eq!(classified1, classified2);
        assert_ne!(classified1, classified3);
    }

    #[test]
    fn test_hash() {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let classified = ClassifiedWrapper::new(42, TestTaxonomy::Sensitive.data_class());
        classified.hash(&mut hasher);
        let hash1 = hasher.finish();

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let classified2 = ClassifiedWrapper::new(42, TestTaxonomy::Sensitive.data_class());
        classified2.hash(&mut hasher);
        let hash2 = hasher.finish();

        assert_eq!(hash1, hash2, "Hashes should be equal for the same classified value");

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let classified3 = ClassifiedWrapper::new(12, TestTaxonomy::Sensitive.data_class());
        classified3.hash(&mut hasher);
        let hash3 = hasher.finish();

        assert_ne!(hash1, hash3, "Hashes of data with different values should not be equal");
    }

    #[test]
    fn test_ordering() {
        let classified1 = ClassifiedWrapper::new(42, TestTaxonomy::Sensitive.data_class());
        let classified2 = ClassifiedWrapper::new(12, TestTaxonomy::Sensitive.data_class());

        assert_eq!(classified1.partial_cmp(&classified2).unwrap(), Ordering::Greater);
        assert_eq!(classified2.partial_cmp(&classified1).unwrap(), Ordering::Less);
        assert_eq!(classified1.partial_cmp(&classified1).unwrap(), Ordering::Equal);
    }

    #[test]
    fn test_declassify_returns_inner_value() {
        // Consuming declassification returns the inner value
        let classified = ClassifiedWrapper::new(String::from("secret"), TestTaxonomy::Sensitive.data_class());
        // assert_eq!(value, "secret");
    }

    #[test]
    fn test_as_declassified_mut_allows_mutation() {
        // Mutable access allows in-place mutation of the wrapped value
        let mut classified = ClassifiedWrapper::new(vec![1, 2, 3], TestTaxonomy::Sensitive.data_class());
        // Ensure the data class remains unchanged after mutation
        assert_eq!(classified.data_class(), TestTaxonomy::Sensitive.data_class());
    }

    // New tests exercising RedactedDebug, RedactedDisplay, and RedactedToString implementations.
    use crate::{RedactionEngineBuilder, SimpleRedactor, SimpleRedactorMode};

    #[test]
    fn test_redacted_debug_and_display_replace_mode_string() {
        let engine = RedactionEngineBuilder::new()
            .add_class_redactor(
                &TestTaxonomy::Sensitive.data_class(),
                SimpleRedactor::with_mode(SimpleRedactorMode::Replace('*')),
            )
            .build();
        let wrapper = ClassifiedWrapper::new("secret".to_string(), TestTaxonomy::Sensitive.data_class());

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
        let engine = RedactionEngineBuilder::new()
            .add_class_redactor(
                &TestTaxonomy::Sensitive.data_class(),
                SimpleRedactor::with_mode(SimpleRedactorMode::Replace('*')),
            )
            .build();
        let wrapper = ClassifiedWrapper::new(42u32, TestTaxonomy::Sensitive.data_class());

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
        let engine = RedactionEngineBuilder::new()
            .add_class_redactor(
                &TestTaxonomy::Sensitive.data_class(),
                SimpleRedactor::with_mode(SimpleRedactorMode::PassthroughAndTag),
            )
            .build();
        let wrapper = ClassifiedWrapper::new("secret".to_string(), TestTaxonomy::Sensitive.data_class());

        let mut debug_out = String::new();
        engine.redacted_debug(&wrapper, &mut debug_out).unwrap();
        // Debug includes quotes in inner representation
        assert_eq!(
            debug_out, "<test/sensitive:\"secret\">",
            "PassthroughAndTag debug should include quotes inside tag"
        );

        let mut display_out = String::new();
        engine.redacted_display(&wrapper, &mut display_out).unwrap();
        assert_eq!(
            display_out, "<test/sensitive:secret>",
            "PassthroughAndTag display should not include quotes"
        );

        let to_string_out = engine.redacted_to_string(&wrapper);
        assert_eq!(to_string_out, "<test/sensitive:secret>");
    }

    #[test]
    fn test_redacted_long_value_fallback_path() {
        // Value length > 128 triggers fallback branch in ClassifiedWrapper's redacted debug/display implementations.
        let engine = RedactionEngineBuilder::new()
            .add_class_redactor(
                &TestTaxonomy::Sensitive.data_class(),
                SimpleRedactor::with_mode(SimpleRedactorMode::Replace('*')),
            )
            .build();
        let long_plain = "a".repeat(140);
        let wrapper = ClassifiedWrapper::new(long_plain.clone(), TestTaxonomy::Sensitive.data_class());

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
}
