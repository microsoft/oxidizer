// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt::Debug;

use rustc_hash::FxHashMap;

use crate::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use crate::{DataClass, IntoDataClass, Redactor};

/// Represents the redaction policy for a specific data class.
pub(crate) enum RedactionPolicy {
    /// Apply the specified redactor to the data.
    Redact(Box<dyn Redactor + Send + Sync>),
    /// Pass through unmodified, bypassing redaction.
    Suppressed,
}

pub(crate) struct RedactionEngineInner {
    redactors: FxHashMap<DataClass, RedactionPolicy>,
    fallback: Box<dyn Redactor + Send + Sync>,
}

/// Type holding all redactors registered for different data classes.
impl RedactionEngineInner {
    /// Returns the redactor to use for the given data class, or `None` if redaction is suppressed.
    ///
    /// Performs a single hash lookup. Returns `None` when the class has been explicitly suppressed,
    /// `Some(class_redactor)` when a class-specific redactor is registered, or `Some(fallback)` otherwise.
    #[must_use]
    pub fn resolve(&self, data_class: &DataClass) -> Option<&(dyn Redactor + Send + Sync)> {
        match self.redactors.get(data_class) {
            Some(RedactionPolicy::Redact(redactor)) => Some(redactor.as_ref()),
            Some(RedactionPolicy::Suppressed) => None,
            None => Some(self.fallback.as_ref()),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.redactors.len()
    }

    pub fn shrink(&mut self) {
        self.redactors.shrink_to_fit();
    }

    pub fn insert(&mut self, data_class: impl IntoDataClass, redactor: impl Redactor + Send + Sync + 'static) {
        self.redactors
            .insert(data_class.into_data_class(), RedactionPolicy::Redact(Box::new(redactor)));
    }

    pub fn suppress(&mut self, data_class: impl IntoDataClass) {
        self.redactors.insert(data_class.into_data_class(), RedactionPolicy::Suppressed);
    }

    #[must_use]
    pub fn would_redact(&self, data_class: &DataClass) -> bool {
        !matches!(self.redactors.get(data_class), Some(RedactionPolicy::Suppressed))
    }

    pub fn set_fallback(&mut self, redactor: impl Redactor + Send + Sync + 'static) {
        self.fallback = Box::new(redactor);
    }
}

impl Default for RedactionEngineInner {
    fn default() -> Self {
        Self {
            redactors: FxHashMap::default(),
            fallback: Box::new(SimpleRedactor::with_mode(SimpleRedactorMode::Erase)),
        }
    }
}

impl Debug for RedactionEngineInner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.redactors.keys()).finish()
    }
}

#[cfg(test)]
mod tests {
    use data_privacy_macros::taxonomy;
    use rustc_hash::FxBuildHasher;

    use super::*;

    #[taxonomy(test)]
    enum TestTaxonomy {
        Sensitive,
        Insensitive,
    }

    #[test]
    fn test_redactor_shrink() {
        let mut inner = RedactionEngineInner {
            redactors: FxHashMap::with_capacity_and_hasher(42, FxBuildHasher),
            ..Default::default()
        };

        inner.insert(
            TestTaxonomy::Sensitive.data_class(),
            SimpleRedactor::with_mode(SimpleRedactorMode::PassthroughAndTag),
        );
        inner.insert(
            TestTaxonomy::Insensitive.data_class(),
            SimpleRedactor::with_mode(SimpleRedactorMode::Replace('#')),
        );

        // Check initial size
        assert_eq!(inner.len(), 2);
        assert!(inner.redactors.capacity() >= 42, "Initial capacity should be at least 42");

        // Shrink the redactors
        inner.shrink();

        // Check size after shrinking
        assert_eq!(inner.len(), 2, "Shrink should not change the number of redactors");
        assert!(inner.redactors.capacity() < 42, "Capacity should shrink below 42");
    }

    #[test]
    fn test_fallback_isnt_redactor() {
        let fallback_redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Erase);
        let mut inner = RedactionEngineInner::default();
        inner.set_fallback(fallback_redactor);
        assert_eq!(inner.len(), 0);
    }

    #[test]
    fn test_suppress_marks_class_as_suppressed() {
        let mut inner = RedactionEngineInner::default();
        inner.suppress(TestTaxonomy::Sensitive);
        assert_eq!(inner.len(), 1);
        assert!(inner.resolve(&TestTaxonomy::Sensitive.data_class()).is_none());
    }

    #[test]
    fn test_would_redact_returns_true_for_unknown_class() {
        let inner = RedactionEngineInner::default();
        assert!(inner.would_redact(&TestTaxonomy::Sensitive.data_class()));
    }

    #[test]
    fn test_would_redact_returns_true_for_registered_redactor() {
        let mut inner = RedactionEngineInner::default();
        inner.insert(TestTaxonomy::Sensitive, SimpleRedactor::new());
        assert!(inner.would_redact(&TestTaxonomy::Sensitive.data_class()));
    }

    #[test]
    fn test_would_redact_returns_false_for_suppressed_class() {
        let mut inner = RedactionEngineInner::default();
        inner.suppress(TestTaxonomy::Sensitive);
        assert!(!inner.would_redact(&TestTaxonomy::Sensitive.data_class()));
    }
}
