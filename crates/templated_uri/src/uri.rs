// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Types and traits that constitute a Uri.

use std::borrow::Cow;
use std::fmt;
use std::fmt::{Debug, Formatter};
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use data_privacy::{Classified, DataClass, RedactedDebug, RedactedDisplay, RedactedToString, RedactionEngine, Sensitive};
pub use http::uri::{Authority, Parts, PathAndQuery, Scheme};

use crate::error::ValidationError;
use crate::{BaseUri, TemplatedPathAndQuery};

/// The privacy classification of an unknown URI.
pub const DATA_CLASS_UNKNOWN_URI: DataClass = DataClass::new(env!("CARGO_PKG_NAME"), "unknown_uri");

/// Represents a URI that can be used as a target for requests.
///
/// This struct encapsulates the [`BaseUri`] (scheme, authority and path prefix) and the path and query components of the URI.
///
/// The `Uri` struct is designed to be flexible and can be constructed with or without a [`BaseUri`].
/// It can also handle templated paths and queries, allowing for dynamic URI generation based on templates.
///
/// ```
/// use templated_uri::uri::PathAndQuery;
/// use templated_uri::{BaseUri, Uri};
/// let base_uri = BaseUri::from_uri_static("http://example.com");
/// let path_and_query = PathAndQuery::from_static("/path?query=1");
/// let uri: Uri = Uri::new().base_uri(base_uri).path_and_query(path_and_query);
/// ```
///
/// ```
/// use templated_uri::{BaseUri, Uri, templated};
///
/// #[templated(template = "/example.com/{param}", unredacted)]
/// #[derive(Clone)]
/// struct MyTemplate {
///     param: usize,
/// }
///
/// let my_template = MyTemplate { param: 42 };
/// let base_uri = BaseUri::from_uri_static("http://example.com");
/// let uri: Uri = Uri::new().path_and_query(my_template).base_uri(base_uri);
/// ```
#[derive(Clone)]
pub struct Uri {
    /// The base of the URI, which includes scheme, authority and path prefix
    base_uri: Option<BaseUri>,
    /// The path and query of the URI.
    path_and_query: Option<TargetPathAndQuery>,
}

impl Default for Uri {
    fn default() -> Self {
        Self::new()
    }
}

impl Uri {
    /// Creates a new [`Uri`], empty instance.
    #[must_use]
    pub fn new() -> Self {
        Self::with_base_and_path(None, None)
    }

    /// Creates a new [`Uri`] instance with the specified classified [`BaseUri`] and path and query.
    /// ```
    /// use templated_uri::uri::PathAndQuery;
    /// use templated_uri::{BaseUri, Uri};
    /// let base_uri = BaseUri::from_uri_static("http://example.com");
    /// let path_and_query = PathAndQuery::from_static("/path?query=1");
    /// let uri: Uri = Uri::with_base_and_path(Some(base_uri.into()), Some(path_and_query.into()));
    /// ```
    #[must_use]
    pub fn with_base_and_path(base_uri: Option<BaseUri>, path_and_query: Option<TargetPathAndQuery>) -> Self {
        Self { base_uri, path_and_query }
    }

    /// adds a path and query to the `Uri` and outputs a new `Uri` instance.
    #[must_use]
    pub fn path_and_query<T>(mut self, path_and_query: T) -> Self
    where
        T: Into<TargetPathAndQuery>,
    {
        self.path_and_query = Some(path_and_query.into());
        self
    }

    /// Adds [`BaseUri`] to the `Uri` and outputs a new `Uri` instance.
    #[must_use]
    pub fn base_uri<E>(mut self, base_uri: E) -> Self
    where
        E: Into<BaseUri>,
    {
        self.base_uri = Some(base_uri.into());
        self
    }

    /// Returns path and query as a `PathAndQuery` if it exists.
    ///
    /// # Errors
    ///
    /// Returns a [`ValidationError`] if the path and query cannot be validated.
    pub fn to_path_and_query(&self) -> Result<Option<PathAndQuery>, ValidationError> {
        self.path_and_query.as_ref().map(TargetPathAndQuery::to_path_and_query).transpose()
    }

    /// Returns the target path and query if it exists.
    pub fn target_path_and_query(&self) -> Option<&TargetPathAndQuery> {
        self.path_and_query.as_ref()
    }

    /// Converts the URI to a string representation.
    pub fn to_string(&self) -> Sensitive<String> {
        let mut path = self.base_uri.as_ref().map(ToString::to_string).unwrap_or_default();

        match self.path_and_query.as_ref().map(TargetPathAndQuery::to_uri_string) {
            // If there is a base URI, trim the leading slash from the path and query to avoid double slashes.
            Some(pq) if self.base_uri.is_some() => path.push_str(pq.trim_start_matches('/')),
            Some(pq) => path.push_str(&pq),
            None => {}
        }

        Sensitive::new(path, DATA_CLASS_UNKNOWN_URI)
    }

    /// Convert the URI to an [`http::Uri`].
    ///
    /// # Errors
    ///
    /// Returns a [`ValidationError`] if the URI is invalid.
    pub fn to_http_uri(&self) -> Result<http::Uri, ValidationError> {
        self.clone().try_into()
    }

    /// Convert the URI into an [`http::Uri`] consuming self.
    ///
    /// # Errors
    ///
    /// Returns a [`ValidationError`] if the URI is invalid.
    pub fn into_http_uri(self) -> Result<http::Uri, ValidationError> {
        self.try_into()
    }
}

impl RedactedDisplay for Uri {
    #[cfg_attr(test, mutants::skip)] // Do not mutate display output.
    fn fmt(&self, engine: &RedactionEngine, f: &mut Formatter) -> fmt::Result {
        self.base_uri
            .as_ref()
            .map_or(Ok(()), |base_uri| f.write_str(base_uri.to_string().as_str()))?;

        match self
            .path_and_query
            .as_ref()
            .map(|path_and_query| path_and_query.to_uri_string_redacted(engine))
        {
            // If there is a base URI, trim the leading slash from the path and query to avoid double slashes.
            Some(pq) if self.base_uri.is_some() => f.write_str(pq.trim_start_matches('/'))?,
            Some(pq) => f.write_str(&pq)?,
            None => {}
        }
        Ok(())
    }
}

impl RedactedDebug for Uri {
    #[cfg_attr(test, mutants::skip)] // Do not mutate debug output.
    fn fmt(&self, engine: &RedactionEngine, f: &mut Formatter) -> fmt::Result {
        self.base_uri
            .as_ref()
            .map_or(Ok(()), |base_uri| f.write_str(base_uri.to_string().as_str()))?;

        match self
            .path_and_query
            .as_ref()
            .map(|path_and_query| path_and_query.to_uri_string_redacted(engine))
        {
            // If there is a base URI, trim the leading slash from the path and query to avoid double slashes.
            Some(pq) if self.base_uri.is_some() => f.write_str(pq.trim_start_matches('/'))?,
            Some(pq) => f.write_str(&pq)?,
            None => {}
        }
        Ok(())
    }
}

impl TryFrom<http::Uri> for Uri {
    type Error = ValidationError;
    fn try_from(uri: http::Uri) -> Result<Self, Self::Error> {
        let parts = uri.into_parts();
        let path_and_query = parts
            .path_and_query
            .map(|pq| TargetPathAndQuery::PathAndQuery(Sensitive::<PathAndQuery>::new(pq, DATA_CLASS_UNKNOWN_URI)));

        let (Some(authority), Some(scheme)) = (parts.authority, parts.scheme) else {
            return Ok(Self::with_base_and_path(None, path_and_query));
        };

        let base_uri = BaseUri::new(scheme, authority)?;
        Ok(Self::with_base_and_path(Some(base_uri), path_and_query))
    }
}

impl Debug for Uri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut dbg = f.debug_struct("Uri");
        if let Some(base_uri) = &self.base_uri {
            dbg.field("base_uri", base_uri);
        }
        dbg.field("path_and_query", &self.path_and_query).finish()
    }
}

impl FromStr for Uri {
    type Err = ValidationError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uri: http::Uri = http::Uri::from_str(s)?;
        uri.try_into()
    }
}

impl TryFrom<&str> for Uri {
    type Error = ValidationError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_str(value)
    }
}

impl TryFrom<String> for Uri {
    type Error = ValidationError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        let uri = http::Uri::try_from(s)?;
        uri.try_into()
    }
}

impl TryFrom<Uri> for http::Uri {
    type Error = ValidationError;
    fn try_from(value: Uri) -> Result<Self, Self::Error> {
        let Uri { base_uri, path_and_query } = value;

        let path_and_query = path_and_query.map(|pq| pq.to_path_and_query()).transpose()?;

        match (base_uri, path_and_query) {
            (Some(base_uri), None) => Ok(base_uri.into()),
            (Some(base_uri), Some(path_and_query)) => base_uri.build_http_uri(path_and_query),
            (None, pq) => {
                let mut parts = Parts::default();
                parts.path_and_query = pq;
                Self::from_parts(parts).map_err(Into::into)
            }
        }
    }
}

/// Path and Query for `Uri`.
#[derive(Clone)]
pub enum TargetPathAndQuery {
    /// A static path and query.
    PathAndQuery(Sensitive<PathAndQuery>),
    /// A templated path and query.
    TemplatedPathAndQuery(Arc<dyn TemplatedPathAndQuery>),
}

impl TargetPathAndQuery {
    /// Creates a new `TargetPathAndQuery` from a classified path and query.
    pub fn from_path_and_query(path_and_query: PathAndQuery) -> Self {
        Self::PathAndQuery(Sensitive::new(path_and_query, DATA_CLASS_UNKNOWN_URI))
    }

    /// Creates a new `TargetPathAndQuery` from a templated path and query.
    pub fn from_templated(templated_path_and_query: impl TemplatedPathAndQuery) -> Self {
        Self::TemplatedPathAndQuery(Arc::new(templated_path_and_query))
    }

    /// Creates a new `TargetPathAndQuery` from a static path and query string.
    #[must_use]
    pub fn from_static(path_and_query: &'static str) -> Self {
        let path_and_query = PathAndQuery::from_static(path_and_query);
        let classified_pq = Sensitive::<PathAndQuery>::new(path_and_query, DATA_CLASS_UNKNOWN_URI);
        Self::PathAndQuery(classified_pq)
    }

    /// Returns the template string for this path and query.
    #[must_use]
    pub fn template(&self) -> Cow<'static, str> {
        match self {
            Self::PathAndQuery(classified_pq) => Cow::Owned(classified_pq.clone().declassify_ref().to_string()),
            Self::TemplatedPathAndQuery(templated) => Cow::Borrowed(templated.template()),
        }
    }

    /// Returns an optional label for this path and query.
    /// For templated paths with a label configured, this returns that label.
    /// For non-templated paths, this returns `None`.
    #[must_use]
    pub fn label(&self) -> Option<Cow<'static, str>> {
        match self {
            Self::PathAndQuery(_) => None,
            Self::TemplatedPathAndQuery(templated) => templated.label().map(Cow::Borrowed),
        }
    }

    /// Converts to a validated [`PathAndQuery`].
    ///
    /// # Errors
    ///
    /// Returns a [`ValidationError`] if the path and query is invalid.
    pub fn to_path_and_query(&self) -> Result<PathAndQuery, ValidationError> {
        match self {
            Self::PathAndQuery(classified_pq) => Ok(classified_pq.declassify_ref().clone()),
            Self::TemplatedPathAndQuery(templated) => templated.to_path_and_query(),
        }
    }

    /// Converts to a URI string.
    pub fn to_uri_string(&self) -> String {
        match self {
            Self::PathAndQuery(classified_pq) => classified_pq.declassify_ref().to_string(),
            Self::TemplatedPathAndQuery(templated) => templated.to_uri_string(),
        }
    }

    /// Converts to a redacted URI string using the provided redaction engine.
    pub fn to_uri_string_redacted(&self, redaction_engine: &RedactionEngine) -> String {
        match self {
            Self::PathAndQuery(classified_pq) => {
                // We can't use to_string in redaction because it automatically prepends a slash if the path doesn't start with one.
                // as_str doesn't do that, so we declassify to get the inner PathAndQuery and then use as_str.
                // TODO?! .reclassify()?
                let declassified = classified_pq.declassify_ref().as_str().to_string();
                let reclassified = Sensitive::new(declassified, classified_pq.data_class().clone());
                redaction_engine.redacted_to_string(&reclassified)
            }
            Self::TemplatedPathAndQuery(templated) => templated.deref().to_redacted_string(redaction_engine),
        }
    }

    /// Converts this target path and query into a [`Uri`].
    pub fn into_uri(self) -> Uri {
        Uri::with_base_and_path(None, Some(self))
    }
}

impl Debug for TargetPathAndQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PathAndQuery(_) => f.debug_tuple("PathAndQuery").finish(),
            Self::TemplatedPathAndQuery(templated) => f.debug_tuple("TemplatedPathAndQuery").field(templated).finish(),
        }
    }
}

impl TryFrom<Uri> for TargetPathAndQuery {
    type Error = ValidationError;
    fn try_from(uri: Uri) -> Result<Self, Self::Error> {
        uri.to_path_and_query()?
            .map(Self::from_path_and_query)
            .ok_or_else(|| ValidationError::caused_by("URI does not have a path and query component"))
    }
}

impl From<PathAndQuery> for TargetPathAndQuery {
    fn from(value: PathAndQuery) -> Self {
        Self::PathAndQuery(Sensitive::new(value, DATA_CLASS_UNKNOWN_URI))
    }
}

impl TryFrom<Uri> for PathAndQuery {
    type Error = ValidationError;
    fn try_from(uri: Uri) -> Result<Self, Self::Error> {
        uri.to_path_and_query()?
            .ok_or_else(|| ValidationError::caused_by("URI does not have a path and query component"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uri_try_from_str() {
        let uri_str = "https://example.com/path?query=1";
        let uri = Uri::try_from(uri_str).unwrap();
        assert_eq!(uri.to_string().declassify_ref(), uri_str);
    }

    #[test]
    fn test_uri_try_from_string() {
        let uri_str = String::from("https://example.com/path?query=1");
        let uri: Uri = Uri::try_from(uri_str.clone()).unwrap();
        assert_eq!(uri.to_string().declassify_into(), uri_str);
    }

    #[test]
    fn test_uri_from_http_uri() {
        let uri_str = "https://example.com/path?query=1";
        let http_uri = http::Uri::from_static(uri_str);
        let uri: Uri = http_uri.clone().try_into().expect("Failed to convert http::Uri to Uri");
        assert_eq!(uri.to_string().declassify_ref(), uri_str);

        let target_hyper_uri: http::Uri = uri.try_into().expect("Failed to convert Uri to http::Uri");
        assert_eq!(target_hyper_uri, http_uri);
    }

    #[test]
    fn test_uri_into_http_uri() {
        let base_uri = BaseUri::from_uri_static("https://example.com/");
        let path_and_query_with_slash = PathAndQuery::from_static("/path?query=1");
        let path_and_query_without_slash = PathAndQuery::from_static("/path?query=1");

        let uri: Uri = Uri::default().base_uri(base_uri).path_and_query(path_and_query_with_slash.clone());
        let http_uri: http::Uri = uri.try_into().expect("Failed to convert Uri to http::Uri");
        assert_eq!(http_uri.to_string(), "https://example.com/path?query=1");

        let base_uri = BaseUri::from_uri_static("https://example.com/foo/");
        let uri: Uri = Uri::default().base_uri(base_uri.clone()).path_and_query(path_and_query_with_slash);
        let http_uri: http::Uri = uri.try_into().expect("Failed to convert Uri to http::Uri");
        assert_eq!(
            http_uri.to_string(),
            "https://example.com/foo/path?query=1",
            "prefix works correctly with trailing slash"
        );

        let uri: Uri = Uri::default().base_uri(base_uri).path_and_query(path_and_query_without_slash);
        let http_uri: http::Uri = uri.try_into().expect("Failed to convert Uri to http::Uri");
        assert_eq!(
            http_uri.to_string(),
            "https://example.com/foo/path?query=1",
            "prefix works correctly without trailing slash"
        );
    }

    #[test]
    fn test_authority_only_uri_from_str() {
        let uri_str = "https://example.com/";
        let uri: Uri = uri_str.parse().unwrap();
        assert_eq!(uri.to_path_and_query().unwrap(), Some(PathAndQuery::from_static("/")));
        assert_eq!(&uri.to_string().declassify_ref(), &uri_str);
    }

    #[test]
    fn test_path_only_uri() {
        let uri_str = "/path/to/resource";
        let uri: Uri = uri_str.parse().unwrap();
        assert!(uri.base_uri.is_none());
        assert_eq!(uri.to_string().declassify_ref(), uri_str);
    }

    #[test]
    fn test_path_and_query_template() {
        let path_and_query = PathAndQuery::from_str("/path/to/resource?query=param").unwrap();
        let target_path_and_query: TargetPathAndQuery = path_and_query.clone().into();
        assert_eq!(target_path_and_query.template(), "/path/to/resource?query=param");
        assert_eq!(target_path_and_query.to_uri_string(), "/path/to/resource?query=param");
        assert_eq!(target_path_and_query.to_path_and_query().unwrap(), path_and_query);
        assert_eq!(
            target_path_and_query.clone().into_uri().to_string(),
            Uri::with_base_and_path(None, Some(target_path_and_query)).to_string()
        );
    }

    #[test]
    fn uri_compare() {
        let uri1 = Uri::from_str("https://example.com/path?query=1").unwrap();
        let uri2 = Uri::from_str("https://example.com/path?query=1").unwrap();
        let uri3 = Uri::from_str("https://example.com/otherpath?query=2").unwrap();
        let uri4 = Uri::from_str("https://www.example.com/otherpath?query=2").unwrap();

        assert_eq!(uri1.to_string(), uri2.to_string());
        assert_ne!(uri1.to_string(), uri3.to_string());
        assert_ne!(uri4.to_string(), uri3.to_string());
    }

    #[test]
    fn test_display_uri() {
        let uri = Uri::from_str("https://example.com/path?query=1").unwrap();
        assert_eq!(uri.to_string().declassify_ref(), "https://example.com/path?query=1");
    }

    #[test]
    fn test_debug_uri() {
        let uri = Uri::from_str("https://example.com/path?query=1").unwrap();
        assert_eq!(
            format!("{uri:?}"),
            r#"Uri { base_uri: BaseUri { origin: Origin { scheme: "https", authority: example.com }, path: BasePath { inner: / } }, path_and_query: Some(PathAndQuery) }"#
        );
    }

    #[test]
    fn redact_path_and_query_uri() {
        let insensitive_paq = |paq: &'static str| TargetPathAndQuery::from_path_and_query(PathAndQuery::from_static(paq));

        let redaction_engine = RedactionEngine::builder().build();
        let paq_with_trailing_slash = insensitive_paq("/sensitive/path?query=secret");
        let paq_without_trailing_slash = insensitive_paq("sensitive/path?query=secret");
        let base_uri = BaseUri::from_uri_static("https://example.com/api/v1/");

        let redacted_uri = Uri::default()
            .base_uri(base_uri.clone())
            .path_and_query(paq_without_trailing_slash.clone())
            .to_redacted_string(&redaction_engine);
        assert_eq!(
            redacted_uri, "https://example.com/api/v1/*",
            "redaction should replace the entire path and query with a single asterisk"
        );

        let redacted_uri = Uri::default()
            .base_uri(base_uri)
            .path_and_query(paq_with_trailing_slash.clone())
            .to_redacted_string(&redaction_engine);
        assert_eq!(
            redacted_uri, "https://example.com/api/v1/*",
            "redaction should replace the entire path and query with a single asterisk and avoid double slashes"
        );

        let redacted_uri = Uri::default()
            .path_and_query(paq_without_trailing_slash)
            .to_redacted_string(&redaction_engine);
        assert_eq!(redacted_uri, "*");

        let redacted_uri = Uri::default()
            .path_and_query(paq_with_trailing_slash)
            .to_redacted_string(&redaction_engine);
        assert_eq!(redacted_uri, "*");
    }

    #[test]
    fn test_redacted_debug_uri() {
        let insensitive_paq = |paq: &'static str| TargetPathAndQuery::from_path_and_query(PathAndQuery::from_static(paq));

        let redaction_engine = RedactionEngine::builder().build();

        // Test with base URI and path and query
        let base_uri = BaseUri::from_uri_static("https://example.com/api/v1/");
        let paq = insensitive_paq("/sensitive/path?query=secret");
        let uri = Uri::default().base_uri(base_uri.clone()).path_and_query(paq);

        let mut redacted_debug = String::new();
        redaction_engine.redacted_debug(&uri, &mut redacted_debug).unwrap();
        assert_eq!(
            redacted_debug, "https://example.com/api/v1/*",
            "RedactedDebug should redact the path and query with asterisk"
        );

        // Test with path and query only (no base URI)
        let paq_only = insensitive_paq("/sensitive/path");
        let uri_no_base = Uri::default().path_and_query(paq_only);

        let mut redacted_debug = String::new();
        redaction_engine.redacted_debug(&uri_no_base, &mut redacted_debug).unwrap();
        assert_eq!(redacted_debug, "*", "RedactedDebug should redact path-only URI to asterisk");

        // Test with base URI only (no path and query)
        let uri_base_only = Uri::default().base_uri(base_uri);

        let mut redacted_debug = String::new();
        redaction_engine.redacted_debug(&uri_base_only, &mut redacted_debug).unwrap();
        assert_eq!(
            redacted_debug, "https://example.com/api/v1/",
            "RedactedDebug should show base URI when no path and query is present"
        );

        // Test empty URI
        let empty_uri = Uri::default();
        let mut redacted_debug = String::new();
        redaction_engine.redacted_debug(&empty_uri, &mut redacted_debug).unwrap();
        assert_eq!(redacted_debug, "", "RedactedDebug should return empty string for empty URI");

        // Test with path that doesn't have leading slash
        let paq_no_slash = insensitive_paq("sensitive/path");
        let uri_no_slash = Uri::default()
            .base_uri(BaseUri::from_uri_static("https://example.com/api/"))
            .path_and_query(paq_no_slash);

        let mut redacted_debug = String::new();
        redaction_engine.redacted_debug(&uri_no_slash, &mut redacted_debug).unwrap();
        assert_eq!(
            redacted_debug, "https://example.com/api/*",
            "RedactedDebug should handle paths without leading slash and avoid double slashes"
        );
    }

    #[test]
    fn to_http_uri() {
        let uri = Uri::from_str("https://example.com/path?query=1").unwrap();
        let http_uri = uri.to_http_uri().unwrap();
        assert_eq!(http_uri.to_string(), "https://example.com/path?query=1");
        drop(uri); // just check that uri is not consumed by to_http_uri
    }

    #[test]
    fn into_http_uri() {
        let uri = Uri::from_str("https://example.com/path?query=1").unwrap();
        let http_uri = uri.into_http_uri().unwrap();
        assert_eq!(http_uri.to_string(), "https://example.com/path?query=1");
    }

    #[test]
    fn test_try_from_uri_to_http_uri_base_only() {
        // Test match arm: (Some(base_uri), None)
        let base_uri = BaseUri::from_uri_static("https://example.com/api/");
        let uri = Uri::default().base_uri(base_uri);

        let http_uri: http::Uri = uri.try_into().unwrap();
        assert_eq!(http_uri.to_string(), "https://example.com/api/");
    }

    #[test]
    fn test_try_from_uri_to_http_uri_path_only() {
        // Test match arm: (None, Some(pq))
        let path_and_query = PathAndQuery::from_static("/path?query=value");
        let uri = Uri::default().path_and_query(path_and_query);

        let http_uri: http::Uri = uri.try_into().unwrap();
        assert_eq!(http_uri.to_string(), "/path?query=value");
    }

    #[test]
    fn test_try_from_uri_to_target_path_and_query_error() {
        // Test error case when URI has no path and query
        let uri = Uri::default().base_uri(BaseUri::from_uri_static("https://example.com/"));

        let result: Result<TargetPathAndQuery, ValidationError> = uri.try_into();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not have a path and query component"));
    }

    #[test]
    fn test_try_from_uri_to_target_path_and_query_success() {
        // Test successful conversion when URI has path and query
        let path_and_query = PathAndQuery::from_static("/test/path?query=value");
        let uri = Uri::default().path_and_query(path_and_query);

        let target_paq: TargetPathAndQuery = uri.try_into().unwrap();
        assert_eq!(target_paq.to_uri_string(), "/test/path?query=value");
    }

    #[test]
    fn test_try_from_uri_to_path_and_query_success() {
        // Test successful conversion when URI has path and query
        let path_and_query = PathAndQuery::from_static("/success/path");
        let uri = Uri::default().path_and_query(path_and_query);

        let paq: PathAndQuery = uri.try_into().unwrap();
        assert_eq!(paq.to_string(), "/success/path");
    }

    #[test]
    fn test_try_from_uri_to_path_and_query_error() {
        // Test error case when URI has no path and query
        let uri = Uri::default().base_uri(BaseUri::from_uri_static("https://example.com/"));

        let result: Result<PathAndQuery, ValidationError> = uri.try_into();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not have a path and query component"));
    }

    #[test]
    fn test_uri_with_base_uri_only_to_string() {
        // Test None branch (line 126) in to_string() method
        let base_uri = BaseUri::from_uri_static("https://example.com/api/");
        let uri = Uri::default().base_uri(base_uri);

        let uri_string = uri.to_string();
        assert_eq!(uri_string.declassify_ref(), "https://example.com/api/");
    }

    #[test]
    fn test_uri_with_base_uri_only_redacted_display() {
        // Test None branch (line 166) in RedactedDisplay::fmt() method
        let base_uri = BaseUri::from_uri_static("https://example.com/api/v1/");
        let uri = Uri::default().base_uri(base_uri);

        let redaction_engine = RedactionEngine::builder().build();
        let redacted = uri.to_redacted_string(&redaction_engine);

        assert_eq!(redacted, "https://example.com/api/v1/");
    }
}
