// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{DataClass, Redactor, SimpleRedactor, SimpleRedactorMode};
use core::fmt::Debug;
use std::collections::HashMap;

pub struct Redactors {
    redactors: HashMap<DataClass, Box<dyn Redactor + Send + Sync>>,
    fallback: Box<dyn Redactor + Send + Sync>,
}

/// Type holding all redactors registered for different data classes.
impl Redactors {
    #[must_use]
    pub(crate) fn get(&self, data_class: &DataClass) -> Option<&(dyn Redactor + Send + Sync)> {
        self.redactors.get(data_class).map(AsRef::as_ref)
    }

    #[must_use]
    fn fallback(&self) -> &(dyn Redactor + Send + Sync) {
        self.fallback.as_ref()
    }

    #[must_use]
    pub(crate) fn get_or_fallback(&self, data_class: &DataClass) -> &(dyn Redactor + Send + Sync) {
        self.get(data_class).unwrap_or_else(|| self.fallback())
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.redactors.len()
    }

    pub(crate) fn shrink(&mut self) {
        self.redactors.shrink_to_fit();
    }

    pub(crate) fn insert(&mut self, data_class: DataClass, redactor: impl Redactor + Send + Sync + 'static) {
        self.redactors.insert(data_class, Box::new(redactor));
    }

    pub(crate) fn set_fallback(&mut self, redactor: impl Redactor + Send + Sync + 'static) {
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
    use crate::redactors::Redactors;
    use crate::{SimpleRedactor, SimpleRedactorMode};
    use data_privacy_macros::taxonomy;
    use std::collections::HashMap;

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
        assert_eq!(redactors.redactors.len(), 2);
        assert!(redactors.redactors.capacity() >= 42, "Initial capacity should be at least 42");

        // Shrink the redactors
        redactors.shrink();

        // Check size after shrinking
        assert_eq!(redactors.redactors.len(), 2, "Shrink should not change the number of redactors");
        assert!(redactors.redactors.capacity() < 42, "Capacity should shrink below 42");
    }
}
