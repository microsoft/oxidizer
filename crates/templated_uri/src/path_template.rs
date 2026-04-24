// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use data_privacy::RedactedDisplay;
use http::uri::PathAndQuery;

use crate::UriError;

/// Allows for the creation of URIs based on templates.
///
/// A `PathTemplate` describes both the path and the optional query string
/// portion of a URI (everything after the authority and before any fragment).
/// Variables may appear in either part (e.g. `/users/{id}?filter={kind}`).
///
/// Use the `#[templated]` attribute macro to derive an implementation.
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
/// # Examples
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
/// assert_eq!(user_path.render(), "/acme/user/john_doe/");
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
/// assert_eq!(user_path.render(), "/acme/user/john_doe/");
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
    /// Renders the template with its current field values into a path-and-query string.
    ///
    /// For a validated [`PathAndQuery`] use [`PathTemplate::to_path_and_query`] instead.
    fn render(&self) -> String;

    /// Converts to a validated [`PathAndQuery`].
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the rendered URI is not a valid path-and-query.
    fn to_path_and_query(&self) -> Result<PathAndQuery, UriError>;

    /// Returns the original RFC 6570 template string.
    ///
    /// This is the string the user wrote in `#[templated(template = "...")]`,
    /// containing variables in the form `{var}`, `{+var}`, `{/var}`, `{?var}`, etc.
    fn template(&self) -> &'static str;

    /// Returns the template in Rust format-string syntax.
    ///
    /// The original RFC 6570 expansions are flattened into bare `{var}` placeholders
    /// suitable for use with [`std::format!`] and friends. Used internally during expansion.
    fn format_template(&self) -> &'static str;

    /// Returns the optional label for this template.
    ///
    /// Set via `#[templated(template = "...", label = "my_label")]`.
    /// Useful for telemetry when the full template is too verbose.
    fn label(&self) -> Option<&'static str>;
}

impl<T: PathTemplate> From<T> for crate::Uri {
    fn from(value: T) -> Self {
        Self::from(crate::Path::from_template(value))
    }
}
