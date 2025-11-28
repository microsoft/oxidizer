// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Implements the [`Classified`](crate::Classified) trait on a newtype.
///
/// This macro is applied to a newtype struct declaration. The newtype
/// wraps an inner type that holds sensitive data. The macro generates
/// an implementation of the [`Classified`](crate::Classified), [`Debug`], [`Deref`](core::ops::Deref),
/// and [`DerefMut`](core::ops::DerefMut) traits.
///
/// # Example
///
/// ```
/// use data_privacy::{classified, taxonomy};
///
/// // Declare a taxonomy
/// #[taxonomy(contoso)]
/// enum ContosoTaxonomy {
///     CustomerContent,
///     CustomerIdentifier,
/// }
///
/// // Declare a classified container
/// #[classified(ContosoTaxonomy::CustomerIdentifier)]
/// struct CustomerId(String);
pub use data_privacy_macros::classified;


/// Generates implementation logic and types to expose a data taxonomy.
///
/// This macro is applied to an enum declaration. Each variant of the enum
/// represents a data class within the taxonomy.
///
/// You provide a taxonomy name as first argument, followed by an optional `serde = false` or `serde = true`
/// argument to control whether serde support is included in the generated taxonomy code.
/// The default value for `serde` is `true`, meaning that serde support is included by default.
///
/// This attribute produces an implementation block for the enum which includes one method for
/// each variant of the enum. These methods each return a [`DataClass`](crate::DataClass) instance representing that data class.
/// In addition, classified data container types are generated for each data class.
///
/// ## Example
///
/// ```ignore
/// use data_privacy::taxonomy;
///
/// #[taxonomy(contoso, serde = false)]
/// enum ContosoTaxonomy {
///     CustomerContent,
///     CustomerIdentifier,
///     OrganizationIdentifier,
/// }
/// ```
pub use data_privacy_macros::taxonomy;
pub use data_privacy_macros::RedactedDebug;
pub use data_privacy_macros::RedactedDisplay;
pub use data_privacy_macros::RedactedToString;






