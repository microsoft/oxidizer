// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A small stand-in data taxonomy for this crate's examples.
//!
//! This taxonomy exists purely to exercise the [`data_privacy`] classification
//! and redaction machinery in examples. It defines a handful of representative
//! data classes and is not intended to model any real-world data-handling
//! policy.
//!
//! It is included by individual examples via `#[path = "support/taxonomy.rs"]`
//! so the examples stay self-contained and need no test-harness dependency.
//! The `observed` crate carries its own copy for the same reason: an example
//! cannot include a file outside its own package directory.

use data_privacy::taxonomy;

/// A stand-in enterprise data taxonomy for examples.
///
/// Each variant maps to a [`data_privacy::DataClass`] via the generated
/// `data_class()` method.
#[taxonomy(microsoft_enterprise)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
// Shared example support type included via `#[path]`, so it is `pub` but lives
// in a private module of each example binary; that combination is fine here.
#[expect(unnameable_types, reason = "example-only support type shared across example binaries via #[path]")]
pub enum MicrosoftEnterpriseDataTaxonomy {
    /// Publicly available, non-personal data.
    PublicNonPersonalData,

    /// End User Identifiable Information: data that directly identifies an end
    /// user.
    Euii,

    /// End User Pseudonymous Identifiers: identifiers that can be linked to an
    /// end user only with additional information.
    Eupi,

    /// System-generated metadata that is not tied to a specific end user.
    SystemMetadata,

    /// Account, configuration, and billing data for a tenant.
    AccountData,
}
