// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Derives the [`RedactedDebug`](crate::RedactedDebug) trait for a struct.
///
/// This macro generates an implementation that formats the struct similarly to the standard
/// library's [`Debug`](std::fmt::Debug) trait, but produces redacted output based on the provided [`RedactionEngine`](crate::RedactionEngine).
/// All fields implementing [`Classified`](crate::Classified) will be automatically redacted according to the engine's policy.
///
/// Fields can be marked with `#[unredacted]` to exclude them from redaction.
///
/// # Example
///
/// ```
/// use data_privacy::{RedactedDebug, classified, taxonomy};
///
/// #[taxonomy(example)]
/// enum ExampleTaxonomy {
///     Sensitive,
/// }
///
/// #[classified(ExampleTaxonomy::Sensitive)]
/// struct UserId(String);
///
/// #[derive(RedactedDebug)]
/// struct User {
///     id: UserId,
///     #[unredacted]
///     age: u32,
/// }
/// ```
pub use data_privacy_macros::RedactedDebug;
/// Derives the [`RedactedDisplay`](crate::RedactedDisplay) trait for a struct.
///
/// This macro generates an implementation that formats the struct similarly to the standard
/// library's [`Display`](std::fmt::Display) trait, but produces redacted output based on the provided [`RedactionEngine`](crate::RedactionEngine).
/// All fields implementing [`Classified`](crate::Classified) will be automatically redacted according to the engine's policy.
///
/// Fields can be marked with `#[unredacted]` to exclude them from redaction.
///
/// # Example
///
/// ```
/// use data_privacy::{RedactedDisplay, classified, taxonomy};
///
/// #[taxonomy(example)]
/// enum ExampleTaxonomy {
///     Sensitive,
/// }
///
/// #[classified(ExampleTaxonomy::Sensitive)]
/// struct UserId(String);
///
/// #[derive(RedactedDisplay)]
/// struct User {
///     id: UserId,
///     #[unredacted]
///     age: u32,
/// }
/// ```
pub use data_privacy_macros::RedactedDisplay;
/// Implements the [`Classified`](crate::Classified) trait on a newtype or single-field struct.
///
/// This macro is applied to a newtype struct or single-field struct declaration. The struct
/// wraps an inner type that holds sensitive data. The macro generates
/// an implementation of the [`Classified`](crate::Classified), [`Debug`], [`Deref`](core::ops::Deref),
/// [`DerefMut`](core::ops::DerefMut), [`RedactedDebug`](crate::RedactedDebug),
/// and [`RedactedDisplay`](crate::RedactedDisplay) traits.
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
