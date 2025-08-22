// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A simple data taxonomy with universal data classes.
//!
//! Data classes in this taxonomy are generic in nature and are useful in a few situations:
//!
//! * [`Insensitive`] is used when data is specifically not classified.
//! * [`UnknownSensitivity`] is used when data is sensitive, but the specific
//!   classification is unknown.
//! * [`Sensitive`] is primarily intended for libraries to indicate particular
//!   data contains some form of sensitive information. General-purpose libraries
//!   are usually agnostic to the application's specific data taxonomy, so if they
//!   need to classify data, they can use [Sensitive] as a general indication to
//!   the application that the data is sensitive.

use data_privacy_macros::taxonomy;

/// A simple data taxonomy with universal data classes.
#[cfg_attr(feature = "serde", taxonomy(common, serde = true))]
#[cfg_attr(not(feature = "serde"), taxonomy(common, serde = false))]
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub enum CommonTaxonomy {
    /// The `sensitive` data class indicates data must be treated carefully.
    ///
    /// This data class is typically used in libraries which are agnostic to a
    /// specific data taxonomy.
    Sensitive,

    /// The `insensitive` data class indicates data is specifically not classified.
    Insensitive,

    /// The `unknown_sensitivity` data class indicates data has an unknown classification.
    UnknownSensitivity,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DataClass;

    #[test]
    fn test_common_taxonomy() {
        assert_eq!(CommonTaxonomy::Sensitive.data_class(), DataClass::new("common", "sensitive"));
        assert_eq!(CommonTaxonomy::Insensitive.data_class(), DataClass::new("common", "insensitive"));
        assert_eq!(
            CommonTaxonomy::UnknownSensitivity.data_class(),
            DataClass::new("common", "unknown_sensitivity")
        );
    }

    #[test]
    fn test_debug_trait() {
        assert_eq!(format!("{:?}", Sensitive::new(2)), "<common/sensitive:REDACTED>");
        assert_eq!(format!("{:?}", Insensitive::new("Hello")), "<common/insensitive:REDACTED>");
        assert_eq!(
            format!("{:?}", UnknownSensitivity::new(31.4)),
            "<common/unknown_sensitivity:REDACTED>"
        );
    }

    #[test]
    fn test_partial_eq() {
        assert_eq!(CommonTaxonomy::Sensitive, CommonTaxonomy::Sensitive.data_class());
        assert_eq!(CommonTaxonomy::Insensitive, CommonTaxonomy::Insensitive.data_class());
        assert_eq!(CommonTaxonomy::UnknownSensitivity, CommonTaxonomy::UnknownSensitivity.data_class());
        assert_ne!(CommonTaxonomy::Sensitive, CommonTaxonomy::Insensitive.data_class());

        // Ensure that the equality is symmetric
        assert_eq!(CommonTaxonomy::Sensitive.data_class(), CommonTaxonomy::Sensitive,);
        assert_ne!(CommonTaxonomy::Sensitive.data_class(), CommonTaxonomy::Insensitive);
    }

    #[test]
    fn test_mapping() {
        let sensitive_int = Sensitive::new(42);
        let sensitive_str = sensitive_int.map(|i| format!("The answer is {i}"));
        assert_eq!(sensitive_str.declassify(), "The answer is 42");
    }
}
