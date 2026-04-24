// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Derives the `Escape` trait for newtype wrappers around URI-escaped types.
///
/// This derive macro implements `Escape` for tuple structs with exactly one field.
/// The implementation calls `escape()` on the inner type, making it suitable for
/// use in URI templates with standard `{param}` (percent-encoded) placeholders.
///
/// # Requirements
///
/// - Must be a tuple struct (newtype pattern) with exactly one field
/// - The inner type must implement [`Escape`]
///
/// # Examples
///
/// ```
/// # use templated_uri::{Escape, EscapedString};
/// #[derive(Escape)]
/// struct UserId(EscapedString);
/// ```
///
/// This allows `UserId` to be used in URI templates where it will be properly encoded.
pub use templated_uri_macros::Escape;
/// Derives the `UnescapedDisplay` trait for newtype wrappers with unrestricted characters.
///
/// This derive macro implements `UnescapedDisplay` for tuple structs with exactly one field.
/// The implementation delegates to the inner field's [`Display`](std::fmt::Display) impl, making it suitable for use in
/// URI templates with unrestricted `{+param}` placeholders that allow reserved characters.
///
/// # Requirements
///
/// - Must be a tuple struct (newtype pattern) with exactly one field
/// - The inner type must implement [`Display`](std::fmt::Display)
///
/// # Examples
///
/// ```
/// use templated_uri::UnescapedDisplay;
///
/// #[derive(UnescapedDisplay)]
/// struct PathSegment(String);
/// ```
///
/// This allows `PathSegment` to be used in URI templates with `{+param}` syntax where
/// reserved characters like `/` should be preserved rather than percent-encoded.
pub use templated_uri_macros::UnescapedDisplay;
/// Generates URI templating and data privacy implementations for structs and enums.
///
/// This macro processes RFC 6570 URI templates and generates implementations for:
/// - `PathTemplate`: Methods for URI template expansion and formatting
/// - `Debug`: Custom debug representation showing the template
/// - `RedactedDisplay`: Data privacy-aware display with selective field redaction
/// - `From<T> for Path`: Conversion to a URI path
///
/// # Struct Usage
///
/// For structs, specify a URI template. Field names must match template parameter names.
///
/// ```
/// # use templated_uri::templated;
/// #[templated(template = "/topic/{topic_id}", unredacted)]
/// struct ListTopics {
///     topic_id: u32,
/// }
/// ```
///
/// ## Template Syntax
///
/// Supports RFC 6570 URI template operators:
/// - `{param}`: URI-escaped (percent-encoded)
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
/// # use templated_uri::{templated, EscapedString};
/// #[templated(template = "/{org_id}/product/{product_id}")]
/// struct ProductPath {
///     org_id: OrgId,           // Will be redacted
///     #[unredacted]
///     product_id: EscapedString, // Will NOT be redacted
/// }
/// ```
///
/// # Enum Usage
///
/// For enums, each variant must contain exactly one field that implements `PathTemplate`.
/// The macro generates delegating implementations that forward to the inner type.
///
/// ```ignore
/// # use templated_uri::templated;
/// #[templated]
/// enum ApiPath {
///     User(UserPath),
///     Product(ProductPath),
/// }
/// ```
///
/// The enum will delegate all trait methods to whichever variant is active, and also generates
/// `From<VariantType>` implementations for easy construction.
pub use templated_uri_macros::templated;
