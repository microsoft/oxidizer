// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use crate::{DataClass, IntoDataClass};
use core::fmt::Debug;
use std::collections::HashMap;
use std::fmt::Write;

pub mod simple_redactor;
#[cfg(feature = "xxh3")]
pub mod xxh3_redactor;

/// Represents types that can redact data.
pub trait Redactor {
    /// Redacts the given value and writes the results to the given output sink.
    ///
    /// # Errors
    ///
    /// This function should return [`Err`] if, and only if, the provided [`Formatter`](std::fmt::Formatter) returns [`Err`]. String redaction is considered an infallible operation;
    /// this function only returns a [`std::fmt::Result`] because writing to the underlying stream might fail and it must provide a way to propagate the fact that an error
    /// has occurred back up the stack.
    fn redact(&self, data_class: &DataClass, value: &str, output: &mut dyn Write) -> std::fmt::Result;
}

pub(crate) struct Redactors {
    redactors: HashMap<DataClass, Box<dyn Redactor + Send + Sync>>,
    fallback: Box<dyn Redactor + Send + Sync>,
}

/// Type holding all redactors registered for different data classes.
impl Redactors {
    #[must_use]
    pub fn get(&self, data_class: &DataClass) -> Option<&(dyn Redactor + Send + Sync)> {
        self.redactors.get(data_class).map(AsRef::as_ref)
    }

    #[must_use]
    fn fallback(&self) -> &(dyn Redactor + Send + Sync) {
        self.fallback.as_ref()
    }

    #[must_use]
    pub fn get_or_fallback(&self, data_class: &DataClass) -> &(dyn Redactor + Send + Sync) {
        self.get(data_class).unwrap_or_else(|| self.fallback())
    }

    #[must_use]
    #[allow(dead_code, reason = "This function is used from testing")]
    pub fn len(&self) -> usize {
        self.redactors.len()
    }

    pub fn shrink(&mut self) {
        self.redactors.shrink_to_fit();
    }

    pub fn insert(&mut self, data_class: impl IntoDataClass, redactor: impl Redactor + Send + Sync + 'static) {
        self.redactors.insert(data_class.into_data_class(), Box::new(redactor));
    }

    pub fn set_fallback(&mut self, redactor: impl Redactor + Send + Sync + 'static) {
        self.fallback = Box::new(redactor);
    }
}

impl Default for Redactors {
    fn default() -> Self {
        Self {
            redactors: HashMap::new(),
            fallback: Box::new(SimpleRedactor::with_mode(SimpleRedactorMode::Insert("*".into()))),
        }
    }
}

impl Debug for Redactors {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.redactors.keys()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use data_privacy_macros::taxonomy;

    struct TestRedactor;

    impl Redactor for TestRedactor {
        fn redact(&self, _data_class: &DataClass, value: &str, output: &mut dyn Write) -> std::fmt::Result {
            write!(output, "{value}tomato")
        }
    }

    #[taxonomy(test)]
    enum TestTaxonomy {
        Sensitive,
        Insensitive,
    }

    #[test]
    fn test_redactor_shrink() {
        let mut redactors = Redactors {
            redactors: HashMap::with_capacity(42),
            ..Default::default()
        };

        redactors.insert(
            TestTaxonomy::Sensitive.data_class(),
            SimpleRedactor::with_mode(SimpleRedactorMode::PassthroughAndTag),
        );
        redactors.insert(
            TestTaxonomy::Insensitive.data_class(),
            SimpleRedactor::with_mode(SimpleRedactorMode::Replace('#')),
        );

        // Check initial size
        assert_eq!(redactors.len(), 2);
        assert!(redactors.redactors.capacity() >= 42, "Initial capacity should be at least 42");

        // Shrink the redactors
        redactors.shrink();

        // Check size after shrinking
        assert_eq!(redactors.len(), 2, "Shrink should not change the number of redactors");
        assert!(redactors.redactors.capacity() < 42, "Capacity should shrink below 42");
    }

    #[test]
    fn test_exact_len_default_behavior() {
        let redactor = TestRedactor;
        let mut output_buffer = String::new();
        _ = redactor.redact(&TestTaxonomy::Sensitive.data_class(), "test_value", &mut output_buffer);

        assert_eq!(output_buffer, "test_valuetomato");
    }

    #[test]
    fn test_fallback_isnt_redactor() {
        let fallback_redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Erase);
        let mut redactors = Redactors::default();
        redactors.set_fallback(fallback_redactor);
        assert_eq!(redactors.len(), 0);
    }
}
