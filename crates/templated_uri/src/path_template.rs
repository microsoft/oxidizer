// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use data_privacy::RedactedDisplay;
use http::uri::PathAndQuery;

use crate::{Path, Uri, UriError};

/// Allows for the creation of URIs based on templates.
///
/// A `PathTemplate` describes both the path and the optional query string
/// portion of a URI (everything after the authority and before any fragment).
/// Variables may appear in either part (e.g. `/users/{id}?filter={kind}`).
///
/// This trait is not meant to be implemented directly; use the `#[templated]` attribute macro instead.
///
/// Templates are based on [RFC 6570](https://datatracker.ietf.org/doc/html/rfc6570) Level 3,
/// with additional constraints for valid HTTP URI construction:
///
/// - Variable names must be valid Rust identifiers (ASCII letters, digits, underscores)
/// - Templates must start with a leading `/`
/// - Fragment expansion (`{#var}`) is not supported (fragments are ignored by HTTP clients)
///
/// All template values must implement [`Escape`](crate::Escape), except for
/// unfiltered expansions (`{+foo}`). This ensures variables cannot contain reserved characters
/// as defined by the RFC.
///
/// # Example
///
/// ```
/// use templated_uri::{PathTemplate, EscapedString, templated};
///
/// #[templated(template = "/{org_id}/user/{user_id}/", unredacted)]
/// #[derive(Clone)]
/// struct UserPath {
///     org_id: EscapedString,
///     user_id: EscapedString,
/// }
///
/// let user_path = UserPath {
///     org_id: EscapedString::from_static("acme"),
///     user_id: EscapedString::from_static("john_doe"),
/// };
///
/// assert_eq!(user_path.to_uri_string(), "/acme/user/john_doe/");
/// ```
///
/// # Classified fields
///
/// The `classified` attribute enables data classification via `data_privacy` types.
///
/// ```
/// #![allow(non_upper_case_globals)]
/// # const Pii: DataClass = DataClass::new("templated_uri", "pii");
/// use data_privacy::{
///     Classified, DataClass, RedactedToString, RedactionEngine, RedactionEngineBuilder, Sensitive,
/// };
/// use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
/// use templated_uri::{PathTemplate, EscapedString, templated};
///
/// #[templated(template = "/{org_id}/user/{user_id}/")]
/// #[derive(Clone)]
/// struct UserPath {
///     #[unredacted]
///     org_id: EscapedString,
///     user_id: Sensitive<EscapedString>,
/// }
///
/// let user_path = UserPath {
///     org_id: EscapedString::from_static("acme"),
///     user_id: Sensitive::new(EscapedString::from_static("john_doe"), Pii),
/// };
/// assert_eq!(user_path.to_uri_string(), "/acme/user/john_doe/");
///
/// let asterisk_redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Replace('*'));
/// let redaction_engine = RedactionEngine::builder()
///     .set_fallback_redactor(asterisk_redactor)
///     .build();
///
/// assert_eq!(
///     user_path.to_redacted_string(&redaction_engine),
///     "/acme/user/********/"
/// )
/// ```
pub trait PathTemplate: RedactedDisplay + Debug + Sync + Send
where
    Self: 'static,
{
    /// Returns the URI path string with template values filled in.
    fn to_uri_string(&self) -> String;

    /// Converts to a validated [`PathAndQuery`].
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the rendered URI is not a valid path-and-query.
    fn to_http_path(&self) -> Result<PathAndQuery, UriError>;

    /// Returns the original RFC 6570 template string.
    fn rfc_6570_template(&self) -> &'static str;

    /// Returns the template in Rust format string syntax.
    fn template(&self) -> &'static str;

    /// Returns the optional label for this template.
    ///
    /// Set via `#[templated(template = "...", label = "my_label")]`.
    /// Useful for telemetry when the full template is too verbose.
    fn label(&self) -> Option<&'static str>;

    /// Converts this template into a [`Uri`].
    fn into_uri(self) -> Uri
    where
        Self: Sized,
    {
        Uri::from(Path::from_template(self))
    }
}

impl<T: PathTemplate> From<T> for Uri {
    fn from(value: T) -> Self {
        value.into_uri()
    }
}
