// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::error::Error as StdError;

use crate::{EnrichmentEntry, OhnoCore};

/// Base trait for adding error enrichment to error types.
///
/// This trait provides the fundamental error enrichment addition method and is dyn-compatible.
/// It serves as the base for the more ergonomic `EnrichableExt` trait.
pub trait Enrichable {
    /// Adds enrichment information to the error.
    ///
    /// This is the core method that other error enrichment methods build upon.
    ///
    /// # Note
    ///
    /// This method is not intended to be used directly. Instead, use:
    /// - The [`enrich_err!()`](crate::enrich_err) macro for convenient error enrichment
    /// - Methods from [`EnrichableExt`] trait ([`enrich()`](EnrichableExt::enrich), [`enrich_with()`](EnrichableExt::enrich_with))
    fn add_enrichment(&mut self, entry: EnrichmentEntry);
}

impl<T, E> Enrichable for Result<T, E>
where
    E: StdError + Enrichable,
{
    fn add_enrichment(&mut self, entry: EnrichmentEntry) {
        if let Err(e) = self {
            e.add_enrichment(entry);
        }
    }
}

impl Enrichable for OhnoCore {
    fn add_enrichment(&mut self, entry: EnrichmentEntry) {
        self.data.enrichment.push(entry);
    }
}

/// Extension trait providing ergonomic error enrichment methods.
pub trait EnrichableExt: Enrichable {
    /// Adds enrichment information to the error.
    ///
    /// It uses [`Location::caller`](std::panic::Location::caller) to capture the file and line number
    /// where this method is invoked.
    #[must_use]
    fn enrich(mut self, msg: impl Into<Cow<'static, str>>) -> Self
    where
        Self: Sized,
    {
        let location = std::panic::Location::caller();
        self.add_enrichment(EnrichmentEntry::new(msg, location.file(), location.line()));
        self
    }

    /// Adds lazily evaluated enrichment information to the error.
    ///
    /// It uses [`Location::caller`](std::panic::Location::caller) to capture the file and line number
    /// where this method is invoked.
    #[must_use]
    fn enrich_with<F, R>(mut self, f: F) -> Self
    where
        F: FnOnce() -> R,
        R: Into<Cow<'static, str>>,
        Self: Sized,
    {
        let location = std::panic::Location::caller();
        self.add_enrichment(EnrichmentEntry::new(f(), location.file(), location.line()));
        self
    }
}

// Blanket implementation: all types that implement Enrichable automatically get EnrichableExt
impl<T: Enrichable> EnrichableExt for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default, ohno::Error)]
    pub struct TestError {
        pub data: OhnoCore,
    }

    #[test]
    fn test_enrich() {
        let mut error = TestError::default();
        error.add_enrichment(EnrichmentEntry::new("Test enrichment", "test.rs", 5));
        assert_eq!(error.data.data.enrichment.len(), 1);
        assert_eq!(error.data.data.enrichment[0].message, "Test enrichment");
        assert_eq!(error.data.data.enrichment[0].location.file, "test.rs");
        assert_eq!(error.data.data.enrichment[0].location.line, 5);

        error.add_enrichment(EnrichmentEntry::new("Test enrichment", "test.rs", 10));
        assert_eq!(error.data.data.enrichment.len(), 2);
        assert_eq!(error.data.data.enrichment[1].message, "Test enrichment");
        let location = &error.data.data.enrichment[1].location;
        assert_eq!(location.file, "test.rs");
        assert_eq!(location.line, 10);
    }

    #[test]
    fn test_enrichable_ext() {
        let error = TestError::default();
        let mut result: Result<(), _> = Err(error);

        result.add_enrichment(EnrichmentEntry::new("Immediate enrichment", "test.rs", 15));

        let err = result.unwrap_err();
        assert_eq!(err.data.data.enrichment.len(), 1);
        assert_eq!(err.data.data.enrichment[0].message, "Immediate enrichment");
        assert_eq!(err.data.data.enrichment[0].location.file, "test.rs");
        assert_eq!(err.data.data.enrichment[0].location.line, 15);

        result = Err(err).enrich("Detailed enrichment");
        let err = result.unwrap_err();

        assert_eq!(err.data.data.enrichment.len(), 2);
        assert_eq!(err.data.data.enrichment[1].message, "Detailed enrichment");
        let location = &err.data.data.enrichment[1].location;
        assert!(location.file.ends_with("enrichable.rs"));
    }
}
