// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::sync::Arc;

use data_privacy::RedactedDisplay;
use http::uri::PathAndQuery;

use crate::uri::TargetPathAndQuery;
use crate::{Uri, ValidationError};

/// Allows for the creation of URIs based on templates.
///
/// This trait is not meant to be implemented directly; use the `#[derive(TemplatedPathAndQuery)]` macro instead.
///
/// Templates follow [RFC 6570](https://datatracker.ietf.org/doc/html/rfc6570) Level 3.
/// All template values must implement [`UriSafe`](crate::UriSafe), except for fragments (`{#foo}`)
/// and unfiltered expansions (`{+foo}`). This ensures variables cannot contain reserved characters
/// as defined by the RFC.
///
/// # Example
///
/// ```
/// use obscuri::{TemplatedPathAndQuery, UriSafeString, templated};
/// use uuid::Uuid;
///
/// #[templated(template = "/{org_id}/user/{user_id}/", unredacted)]
/// #[derive(Clone)]
/// struct UserPath {
///     org_id: Uuid,           // Uuid implements `UriSafe` by default
///     user_id: UriSafeString, // String wrapper that ensures URI safety
/// }
///
/// let org_id = Uuid::new_v4();
/// let user_path = UserPath {
///     org_id,
///     user_id: UriSafeString::from_static("john_doe"),
/// };
///
/// assert_eq!(
///     user_path.to_uri_string(),
///     format!("/{org_id}/user/john_doe/")
/// );
/// ```
///
/// # Classified fields
///
/// The `classified` attribute enables data classification via `data_privacy` types.
///
/// ```
/// #![allow(non_upper_case_globals)]
/// # use std::str::FromStr;
/// # const Pii: DataClass = DataClass::new("obscuri", "pii");
/// use data_privacy::{
///     Classified, DataClass, RedactedToString, RedactionEngine, RedactionEngineBuilder, Sensitive,
/// };
/// use obscuri::{TemplatedPathAndQuery, UriSafeString, templated};
/// use uuid::Uuid;
///
/// #[templated(template = "/{org_id}/user/{user_id}/")]
/// #[derive(Clone)]
/// struct UserPath {
///     #[unredacted]
///     org_id: Uuid,
///     user_id: Sensitive<UriSafeString>,
/// }
///
/// let user_path = UserPath {
///     org_id: Uuid::from_str("e2a8cdb5-300f-4f83-aa10-f08756578f9b")
///         .unwrap()
///         .into(),
///     user_id: Sensitive::new(UriSafeString::from_static("john_doe").into(), Pii),
/// };
/// assert_eq!(
///     user_path.to_uri_string(),
///     "/e2a8cdb5-300f-4f83-aa10-f08756578f9b/user/john_doe/"
/// );
///
/// let redaction_engine = RedactionEngine::builder().build();
///
/// assert_eq!(
///     user_path.to_redacted_string(&redaction_engine),
///     "/e2a8cdb5-300f-4f83-aa10-f08756578f9b/user/*/"
/// )
/// ```
pub trait TemplatedPathAndQuery: RedactedDisplay + Debug + Sync + Send
where
    Self: 'static,
{
    /// Returns the URI path string with template values filled in.
    fn to_uri_string(&self) -> String;

    /// Converts to a validated [`PathAndQuery`].
    ///
    /// # Errors
    ///
    /// Returns a [`ValidationError`] if the path and query string is invalid.
    fn to_path_and_query(&self) -> Result<PathAndQuery, ValidationError>;

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
        Uri::with_base_and_path(None, Some(TargetPathAndQuery::TemplatedPathAndQuery(Arc::new(self))))
    }
}

impl<T: TemplatedPathAndQuery> From<T> for Uri {
    fn from(value: T) -> Self {
        value.into_uri()
    }
}
