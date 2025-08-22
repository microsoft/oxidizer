// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt::Debug;
use std::collections::HashMap;

use crate::redaction_engine::RedactionEngine;
use crate::{DataClass, Redactor, SimpleRedactor, SimpleRedactorMode};

/// A builder for creating a [`RedactionEngine`].
pub struct RedactionEngineBuilder {
    redactors: HashMap<DataClass, Box<dyn Redactor + Send + Sync>>,
    fallback: Box<dyn Redactor + Send + Sync>,
}

impl RedactionEngineBuilder {
    /// Creates a new instance of `RedactionEngineBuilder`.
    ///
    /// This is initialized with no registered redactors and a fallback redactor that erases the input.
    #[must_use]
    pub fn new() -> Self {
        Self {
            redactors: HashMap::new(),
            fallback: Box::new(SimpleRedactor::with_mode(SimpleRedactorMode::Erase)),
        }
    }

    /// Adds a redactor for a specific data class.
    ///
    /// Whenever the redaction engine encounters data of this class, it will use the provided redactor.
    #[must_use]
    pub fn add_class_redactor(mut self, data_class: &DataClass, redactor: impl Redactor + Send + Sync + 'static) -> Self {
        _ = self.redactors.insert(data_class.clone(), Box::new(redactor));
        self
    }

    /// Adds a redactor that's a fallback for when there is no redactor registered for a particular
    /// data class.
    ///
    /// The default fallback is to use an `ErasingRedactor`, which simply erases the original string.
    #[must_use]
    pub fn set_fallback_redactor(mut self, redactor: impl Redactor + Send + Sync + 'static) -> Self {
        self.fallback = Box::new(redactor);
        self
    }

    /// Builds the `RedactionEngine`.
    #[must_use]
    pub fn build(self) -> RedactionEngine {
        RedactionEngine::new(self.redactors, self.fallback)
    }
}

impl Default for RedactionEngineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Debug for RedactionEngineBuilder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.redactors.keys()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_redaction(engine: &RedactionEngine, data_class: &DataClass, input: &str, expected: &str) {
        let mut output = String::new();
        engine.redact(data_class, input, |s| output.push_str(s));
        assert_eq!(output, expected);
    }

    #[test]
    fn new_creates_builder_with_default_values() {
        let builder = RedactionEngineBuilder::new();
        let engine = builder.build();
        test_redaction(&engine, &DataClass::new("test_taxonomy", "test_class"), "sensitive data", "");

        let builder = RedactionEngineBuilder::default();
        let engine = builder.build();
        test_redaction(&engine, &DataClass::new("test_taxonomy", "test_class"), "sensitive data", "");
    }

    #[test]
    fn add_multiple_class_redactors() {
        let redactor1 = SimpleRedactor::with_mode(SimpleRedactorMode::Insert("XX".to_string()));
        let redactor2 = SimpleRedactor::with_mode(SimpleRedactorMode::Insert("YY".to_string()));

        let data_class1 = DataClass::new("taxonomy", "class1");
        let data_class2 = DataClass::new("taxonomy", "class2");
        let data_class3 = DataClass::new("taxonomy", "class3");

        let builder = RedactionEngineBuilder::new()
            .add_class_redactor(&data_class1, redactor1)
            .add_class_redactor(&data_class2, redactor2);

        let engine = builder.build();
        test_redaction(&engine, &data_class1, "sensitive data", "XX");
        test_redaction(&engine, &data_class2, "sensitive data", "YY");
        test_redaction(&engine, &data_class3, "sensitive data", "");
    }

    #[test]
    fn set_fallback_redactor_overwrites_default() {
        let redactor1 = SimpleRedactor::with_mode(SimpleRedactorMode::Insert("XX".to_string()));
        let redactor2 = SimpleRedactor::with_mode(SimpleRedactorMode::Insert("YY".to_string()));
        let redactor3 = SimpleRedactor::with_mode(SimpleRedactorMode::Insert("ZZ".to_string()));

        let data_class1 = DataClass::new("taxonomy", "class1");
        let data_class2 = DataClass::new("taxonomy", "class2");
        let data_class3 = DataClass::new("taxonomy", "class3");

        let builder = RedactionEngineBuilder::new()
            .add_class_redactor(&data_class1, redactor1)
            .add_class_redactor(&data_class2, redactor2)
            .set_fallback_redactor(redactor3);

        let engine = builder.build();
        test_redaction(&engine, &data_class1, "sensitive data", "XX");
        test_redaction(&engine, &data_class2, "sensitive data", "YY");
        test_redaction(&engine, &data_class3, "sensitive data", "ZZ");
    }

    #[test]
    fn debug_trait_implementation() {
        let redactor1 = SimpleRedactor::with_mode(SimpleRedactorMode::Insert("XX".to_string()));
        let redactor2 = SimpleRedactor::with_mode(SimpleRedactorMode::Insert("YY".to_string()));

        let data_class1 = DataClass::new("taxonomy", "class1");
        let data_class2 = DataClass::new("taxonomy", "class2");

        let builder = RedactionEngineBuilder::new()
            .add_class_redactor(&data_class1, redactor1)
            .add_class_redactor(&data_class2, redactor2);

        let debug_output = format!("{builder:?}");

        // The debug output should contain both data classes
        assert!(debug_output.contains("class1"));
        assert!(debug_output.contains("class2"));
        assert!(debug_output.contains("taxonomy"));

        // Test empty builder debug output
        let empty_builder = RedactionEngineBuilder::new();
        let empty_debug_output = format!("{empty_builder:?}");
        assert_eq!(empty_debug_output, "[]");
    }
}
