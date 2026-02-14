// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Derives the `UriFragment` trait for newtype wrappers around URI-safe types.
///
/// This derive macro implements `UriFragment` for tuple structs with exactly one field.
/// The implementation calls `as_uri_safe()` on the inner type, making it suitable for
/// use in URI templates with standard `{param}` (percent-encoded) placeholders.
///
/// # Requirements
///
/// - Must be a tuple struct (newtype pattern) with exactly one field
/// - The inner type must implement `ToString`
///
/// # Example
///
/// ```
/// # use obscuri::{UriFragment, UriSafeString};
/// #[derive(UriFragment)]
/// struct UserId(UriSafeString);
/// ```
///
/// This allows `UserId` to be used in URI templates where it will be properly encoded.
pub use obscuri_macros::UriFragment;
/// Derives the `UriUnsafeFragment` trait for newtype wrappers with unrestricted characters.
///
/// This derive macro implements `UriUnsafeFragment` for tuple structs with exactly one field.
/// The implementation calls `as_display()` on the inner type, making it suitable for use in
/// URI templates with unrestricted `{+param}` placeholders that allow reserved characters.
///
/// # Requirements
///
/// - Must be a tuple struct (newtype pattern) with exactly one field
/// - The inner type must implement `ToString`
///
/// # Example
///
/// ```
/// # use std::fmt::Display;
/// use obscuri::UriUnsafeFragment;
///
/// #[derive(UriUnsafeFragment)]
/// struct PathSegment(String);
///
/// impl Display for PathSegment {
///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
///         write!(f, "{}", self.0)
///     }
/// }
/// ```
///
/// This allows `PathSegment` to be used in URI templates with `{+param}` syntax where
/// reserved characters like `/` should be preserved rather than percent-encoded.
pub use obscuri_macros::UriUnsafeFragment;
/// Generates URI templating and data privacy implementations for structs and enums.
///
/// This macro processes RFC 6570 URI templates and generates implementations for:
/// - `TemplatedPathAndQuery`: Methods for URI template expansion and formatting
/// - `Debug`: Custom debug representation showing the template
/// - `RedactedDisplay`: Data privacy-aware display with selective field redaction
/// - `From<T> for TargetPathAndQuery`: Conversion to target path and query
///
/// # Struct Usage
///
/// For structs, specify a URI template. Field names must match template parameter names.
///
/// ```
/// # use obscuri::templated;
/// #[templated(template = "/topic/{topic_id}", unredacted)]
/// struct ListTopics {
///     topic_id: u32,
/// }
/// ```
///
/// ## Template Syntax
///
/// Supports RFC 6570 URI template operators:
/// - `{param}`: URI-safe encoding (percent-encoded)
/// - `{+param}`: Unrestricted (allows reserved characters like `/`)
/// - `{/param1,param2}`: Path segment expansion (`/value1/value2`)
/// - `{?param1,param2}`: Query parameter expansion (`?param1=value1&param2=value2`)
///
/// ## Data Privacy
///
/// By default, all fields use `RedactedDisplay` for privacy protection. Use attributes to control:
/// - `#[templated(template = "...", unredacted)]`: Disable redaction for all fields
/// - `#[unredacted]`: Disable redaction for a specific field
///
/// ```ignore
/// # use obscuri::{templated, UriSafeString};
/// #[templated(template = "/{org_id}/product/{product_id}")]
/// struct ProductPath {
///     org_id: OrgId,           // Will be redacted
///     #[unredacted]
///     product_id: UriSafeString, // Will NOT be redacted
/// }
/// ```
///
/// # Enum Usage
///
/// For enums, each variant must contain exactly one field that implements `TemplatedPathAndQuery`.
/// The macro generates delegating implementations that forward to the inner type.
///
/// ```ignore
/// # use data_privacy::templated;
/// #[templated]
/// enum ApiPath {
///     User(UserPath),
///     Product(ProductPath),
/// }
/// ```
///
/// The enum will delegate all trait methods to whichever variant is active, and also generates
/// `From<VariantType>` implementations for easy construction.
pub use obscuri_macros::templated;
