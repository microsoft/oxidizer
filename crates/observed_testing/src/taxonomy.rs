// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A small stand-in data taxonomy used by the `observed` crate family's
//! examples and integration tests.
//!
//! This taxonomy exists purely to exercise the [`data_privacy`] classification
//! and redaction machinery in examples and tests. It defines a handful of
//! representative data classes and is not intended to model any real-world
//! data-handling policy.
//!
//! # Example
//!
//! ```
//! use data_privacy::classified;
//! use observed_testing::MicrosoftEnterpriseDataTaxonomy;
//!
//! #[classified(MicrosoftEnterpriseDataTaxonomy::Euii)]
//! struct UserPrincipalName(String);
//!
//! let _user = UserPrincipalName("alice@example.com".to_owned());
//! ```

use data_privacy::taxonomy;

/// A stand-in enterprise data taxonomy for examples and tests.
///
/// Each variant maps to a [`data_privacy::DataClass`] via the generated
/// `data_class()` method.
#[taxonomy(microsoft_enterprise)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
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
