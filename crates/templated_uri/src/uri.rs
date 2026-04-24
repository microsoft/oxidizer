// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Types and traits that constitute a Uri.

use std::fmt;
use std::fmt::{Debug, Formatter};
use std::str::FromStr;

use data_privacy::{DataClass, RedactedDebug, RedactedDisplay, RedactedToString, RedactionEngine, Sensitive};
use http::uri::{Parts, PathAndQuery};

use crate::error::UriError;
use crate::{BasePath, BaseUri, Origin, Path};

/// Represents a URI that can be used as a target for requests.
///
/// This struct encapsulates the [`BaseUri`] (scheme, authority and path prefix) and the path and query components of the URI.
///
/// The `Uri` struct is designed to be flexible and can be constructed with or without a [`BaseUri`].
/// It can also wrap a templated path produced by a [`PathTemplate`](crate::PathTemplate) implementation, allowing for
/// dynamic URI generation.
///
/// ```
/// use templated_uri::PathAndQuery;
/// use templated_uri::{BaseUri, Uri};
/// let base_uri = BaseUri::from_static("http://example.com");
/// let path = PathAndQuery::from_static("/path?query=1");
/// let uri: Uri = Uri::new().with_base(base_uri).with_path(path);
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
/// let base_uri = BaseUri::from_static("http://example.com");
/// let uri: Uri = Uri::new().with_path(my_template).with_base(base_uri);
/// ```
#[derive(Clone)]
pub struct Uri {
    /// The base of the URI, which includes scheme, authority and path prefix
    pub(crate) base_uri: Option<BaseUri>,
    /// The path and query of the URI.
    pub(crate) path: Option<Path>,
}

impl Default for Uri {
    fn default() -> Self {
        Self::new()
    }
}

impl Uri {
    /// The privacy classification used for URI strings whose individual parts
    /// have not been further classified.
    pub const DATA_CLASS: DataClass = DataClass::new(env!("CARGO_PKG_NAME"), "unknown_uri");

    /// Creates a new [`Uri`], empty instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            base_uri: None,
            path: None,
        }
    }

    /// Creates a new [`Uri`] from a static string.
    ///
    /// # Panics
    ///
    /// Panics if the string is not a valid URI. Intended for use with string
    /// literals known at compile time; use [`Uri::from_str`] for fallible parsing.
    ///
    /// ```
    /// use templated_uri::Uri;
    ///
    /// let uri = Uri::from_static("https://example.com/path?query=1");
    /// ```
    #[must_use]
    pub fn from_static(uri: &'static str) -> Self {
        Self::try_from(http::Uri::from_static(uri)).expect("static str is not a valid URI")
    }

    /// Creates a new [`Uri`] from a [`BaseUri`] and a [`Path`].
    ///
    /// ```
    /// use templated_uri::{BaseUri, Uri, Path};
    /// use templated_uri::PathAndQuery;
    ///
    /// let base = BaseUri::from_static("http://example.com");
    /// let path = Path::from(PathAndQuery::from_static("/path?query=1"));
    /// let uri = Uri::from_parts(base, path);
    /// ```
    #[must_use]
    pub fn from_parts(base: impl Into<Option<BaseUri>>, path: impl Into<Option<Path>>) -> Self {
        Self {
            base_uri: base.into(),
            path: path.into(),
        }
    }

    /// Consumes the `Uri` and returns its optional [`BaseUri`] and [`Path`] components.
    ///
    /// ```
    /// use templated_uri::{BaseUri, Path, Uri};
    /// use templated_uri::PathAndQuery;
    ///
    /// let base = BaseUri::from_static("http://example.com");
    /// let path = Path::from(PathAndQuery::from_static("/path?query=1"));
    /// let uri = Uri::from_parts(base.clone(), path.clone());
    ///
    /// let (got_base, got_path) = uri.into_parts();
    /// assert_eq!(got_base, Some(base));
    /// assert!(got_path.is_some());
    /// ```
    #[must_use]
    pub fn into_parts(self) -> (Option<BaseUri>, Option<Path>) {
        (self.base_uri, self.path)
    }

    /// Sets the path component of this `Uri` and returns the updated value.
    #[must_use]
    pub fn with_path(self, path: impl Into<Path>) -> Self {
        Self {
            path: Some(path.into()),
            ..self
        }
    }

    /// Sets the [`BaseUri`] of this `Uri` and returns the updated value.
    #[must_use]
    pub fn with_base(self, base: impl Into<BaseUri>) -> Self {
        Self {
            base_uri: Some(base.into()),
            ..self
        }
    }

    /// Returns the path and query as a [`PathAndQuery`] if present.
    ///
    /// Conversion errors are suppressed and returned as `None`. In practice
    /// this should never happen: a templated path that fails to materialize
    /// into a valid `PathAndQuery` indicates a programming error in the
    /// template implementation rather than a recoverable runtime condition.
    #[must_use]
    pub fn to_path_and_query(&self) -> Option<PathAndQuery> {
        self.path.as_ref().and_then(|p| PathAndQuery::try_from(p).ok())
    }

    /// Returns the [`Path`] for this URI, if any.
    #[must_use]
    pub fn to_path(&self) -> Option<Path> {
        self.path.clone()
    }

    /// Returns the URI as a [`Sensitive`] string, classified under [`Uri::DATA_CLASS`].
    ///
    /// This shadows [`ToString::to_string`] to ensure callers receive a classified value
    /// rather than a plain `String`. Use [`Sensitive::declassify_ref`] (or the
    /// [`RedactedDisplay`] impl) when you need access to the underlying text.
    pub fn to_string(&self) -> Sensitive<String> {
        let mut path = self.base_uri.as_ref().map(ToString::to_string).unwrap_or_default();

        match self.path.as_ref().map(Path::to_string) {
            // If there is a base URI, trim the leading slash from the path and query to avoid double slashes.
            Some(pq) if self.base_uri.is_some() => path.push_str(pq.declassify_ref().trim_start_matches('/')),
            Some(pq) => path.push_str(pq.declassify_ref()),
            None => {}
        }

        Sensitive::new(path, Self::DATA_CLASS)
    }
}

impl RedactedDisplay for Uri {
    #[cfg_attr(test, mutants::skip)] // Do not mutate display output.
    fn fmt(&self, engine: &RedactionEngine, f: &mut Formatter) -> fmt::Result {
        if let Some(base_uri) = self.base_uri.as_ref() {
            write!(f, "{base_uri}")?;
        }

        match self.path.as_ref().map(|path| path.to_redacted_string(engine)) {
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
        if let Some(base_uri) = self.base_uri.as_ref() {
            write!(f, "{base_uri}")?;
        }

        match self.path.as_ref().map(|path| path.to_redacted_string(engine)) {
            // If there is a base URI, trim the leading slash from the path and query to avoid double slashes.
            Some(pq) if self.base_uri.is_some() => f.write_str(pq.trim_start_matches('/'))?,
            Some(pq) => f.write_str(&pq)?,
            None => {}
        }
        Ok(())
    }
}

impl TryFrom<http::Uri> for Uri {
    type Error = UriError;

    /// Converts an [`http::Uri`] into a [`Uri`].
    ///
    /// # Errors
    ///
    /// Currently infallible in practice, but returns [`UriError`] for forward-compatibility
    /// if internal validation fails.
    fn try_from(uri: http::Uri) -> Result<Self, Self::Error> {
        let parts = uri.into_parts();
        let path = parts.path_and_query.map(Path::from);

        let (Some(authority), Some(scheme)) = (parts.authority, parts.scheme) else {
            return Ok(Self { base_uri: None, path });
        };

        let base_uri = BaseUri::from_parts(Origin::from_parts(scheme, authority), BasePath::default());
        Ok(Self {
            base_uri: Some(base_uri),
            path,
        })
    }
}

impl Debug for Uri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut dbg = f.debug_struct("Uri");
        if let Some(base_uri) = &self.base_uri {
            dbg.field("base_uri", base_uri);
        }
        dbg.field("path", &self.path).finish()
    }
}

impl FromStr for Uri {
    type Err = UriError;

    /// Parses a [`Uri`] from a string.
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the string is not a valid URI.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uri: http::Uri = http::Uri::from_str(s)?;
        uri.try_into()
    }
}

impl TryFrom<&str> for Uri {
    type Error = UriError;

    /// Parses a [`Uri`] from a string slice.
    ///
    /// # Errors
    ///
    /// See [`Uri::from_str`].
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_str(value)
    }
}

impl TryFrom<String> for Uri {
    type Error = UriError;

    /// Parses a [`Uri`] from an owned `String`.
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the string is not a valid URI.
    fn try_from(s: String) -> Result<Self, Self::Error> {
        let uri = http::Uri::try_from(s)?;
        uri.try_into()
    }
}

impl TryFrom<Uri> for http::Uri {
    type Error = UriError;

    /// Converts a [`Uri`] into an [`http::Uri`].
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the templated path fails to materialize into a valid
    /// path-and-query, or if the resulting parts cannot be assembled into an [`http::Uri`].
    fn try_from(value: Uri) -> Result<Self, Self::Error> {
        let Uri { base_uri, path } = value;

        let path = path.map(|pq| PathAndQuery::try_from(&pq)).transpose()?;

        match (base_uri, path) {
            (Some(base_uri), None) => Ok(base_uri.into()),
            (Some(base_uri), Some(path)) => base_uri.build_http_uri(path),
            (None, pq) => {
                let mut parts = Parts::default();
                parts.path_and_query = pq;
                Self::from_parts(parts).map_err(Into::into)
            }
        }
    }
}

impl From<BaseUri> for Uri {
    fn from(value: BaseUri) -> Self {
        Self {
            base_uri: Some(value),
            path: None,
        }
    }
}

impl TryFrom<Uri> for PathAndQuery {
    type Error = UriError;

    /// Extracts the [`PathAndQuery`] from a [`Uri`].
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the URI has no path component, or if the templated path
    /// fails to materialize into a valid path-and-query.
    fn try_from(uri: Uri) -> Result<Self, Self::Error> {
        let Uri { path, .. } = uri;
        let path = path.ok_or_else(|| UriError::invalid_uri("URI does not have a path and query component"))?;

        Self::try_from(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uri_from_base_uri() {
        let base = BaseUri::from_static("https://example.com/api/");
        let uri: Uri = base.into();
        assert_eq!(uri.to_string().declassify_ref(), "https://example.com/api/");
        assert!(uri.to_path().is_none());
    }

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
        let base_uri = BaseUri::from_static("https://example.com/");
        let path_with_slash = PathAndQuery::from_static("/path?query=1");
        let path_without_slash = PathAndQuery::from_static("path?query=1");

        let uri: Uri = Uri::default().with_base(base_uri).with_path(path_with_slash.clone());
        let http_uri: http::Uri = uri.try_into().expect("Failed to convert Uri to http::Uri");
        assert_eq!(http_uri.to_string(), "https://example.com/path?query=1");

        let base_uri = BaseUri::from_static("https://example.com/foo/");
        let uri: Uri = Uri::default().with_base(base_uri.clone()).with_path(path_with_slash);
        let http_uri: http::Uri = uri.try_into().expect("Failed to convert Uri to http::Uri");
        assert_eq!(
            http_uri.to_string(),
            "https://example.com/foo/path?query=1",
            "prefix works correctly with trailing slash"
        );

        let uri: Uri = Uri::default().with_base(base_uri).with_path(path_without_slash);
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
        assert_eq!(uri.to_path_and_query(), Some(PathAndQuery::from_static("/")));
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
            r#"Uri { base_uri: BaseUri { origin: Origin { scheme: "https", authority: example.com }, path: BasePath { inner: / } }, path: Some(Path) }"#
        );
    }

    #[test]
    fn redact_path_uri() {
        let insensitive_paq = |paq: &'static str| Path::from_static(paq);

        let redaction_engine = RedactionEngine::builder().build();
        let paq_with_trailing_slash = insensitive_paq("/sensitive/path?query=secret");
        let paq_without_trailing_slash = insensitive_paq("sensitive/path?query=secret");
        let base_uri = BaseUri::from_static("https://example.com/api/v1/");

        let redacted_uri = Uri::default()
            .with_base(base_uri.clone())
            .with_path(paq_without_trailing_slash.clone())
            .to_redacted_string(&redaction_engine);
        assert_eq!(
            redacted_uri, "https://example.com/api/v1/",
            "redaction should erase the entire path and query"
        );

        let redacted_uri = Uri::default()
            .with_base(base_uri)
            .with_path(paq_with_trailing_slash.clone())
            .to_redacted_string(&redaction_engine);
        assert_eq!(
            redacted_uri, "https://example.com/api/v1/",
            "redaction should erase the entire path and query and avoid double slashes"
        );

        let redacted_uri = Uri::default()
            .with_path(paq_without_trailing_slash)
            .to_redacted_string(&redaction_engine);
        assert_eq!(redacted_uri, "");

        let redacted_uri = Uri::default()
            .with_path(paq_with_trailing_slash)
            .to_redacted_string(&redaction_engine);
        assert_eq!(redacted_uri, "");
    }

    #[test]
    fn test_redacted_debug_uri() {
        let insensitive_paq = |paq: &'static str| Path::from_static(paq);

        let redaction_engine = RedactionEngine::builder().build();

        // Test with base URI and path and query
        let base_uri = BaseUri::from_static("https://example.com/api/v1/");
        let paq = insensitive_paq("/sensitive/path?query=secret");
        let uri = Uri::default().with_base(base_uri.clone()).with_path(paq);

        let mut redacted_debug = String::new();
        redaction_engine.redacted_debug(&uri, &mut redacted_debug).unwrap();
        assert_eq!(
            redacted_debug, "https://example.com/api/v1/",
            "RedactedDebug should erase the path and query"
        );

        // Test with path and query only (no base URI)
        let paq_only = insensitive_paq("/sensitive/path");
        let uri_no_base = Uri::default().with_path(paq_only);

        let mut redacted_debug = String::new();
        redaction_engine.redacted_debug(&uri_no_base, &mut redacted_debug).unwrap();
        assert_eq!(redacted_debug, "", "RedactedDebug should erase path-only URI");

        // Test with base URI only (no path and query)
        let uri_base_only = Uri::default().with_base(base_uri);

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
            .with_base(BaseUri::from_static("https://example.com/api/"))
            .with_path(paq_no_slash);

        let mut redacted_debug = String::new();
        redaction_engine.redacted_debug(&uri_no_slash, &mut redacted_debug).unwrap();
        assert_eq!(
            redacted_debug, "https://example.com/api/",
            "RedactedDebug should handle paths without leading slash and avoid double slashes"
        );
    }

    #[test]
    fn try_into_http_uri() {
        let uri = Uri::from_str("https://example.com/path?query=1").unwrap();
        let http_uri = http::Uri::try_from(uri).unwrap();
        assert_eq!(http_uri.to_string(), "https://example.com/path?query=1");
    }

    #[test]
    fn test_try_from_uri_to_http_uri_base_only() {
        // Test match arm: (Some(base_uri), None)
        let base_uri = BaseUri::from_static("https://example.com/api/");
        let uri = Uri::default().with_base(base_uri);

        let http_uri: http::Uri = uri.try_into().unwrap();
        assert_eq!(http_uri.to_string(), "https://example.com/api/");
    }

    #[test]
    fn test_try_from_uri_to_http_uri_path_only() {
        // Test match arm: (None, Some(pq))
        let path = PathAndQuery::from_static("/path?query=value");
        let uri = Uri::default().with_path(path);

        let http_uri: http::Uri = uri.try_into().unwrap();
        assert_eq!(http_uri.to_string(), "/path?query=value");
    }

    #[test]
    fn test_try_from_uri_to_path_success_paq() {
        // Test successful conversion when URI has path and query
        let path = PathAndQuery::from_static("/success/path");
        let uri = Uri::default().with_path(path);

        let paq: PathAndQuery = uri.try_into().unwrap();
        assert_eq!(paq.to_string(), "/success/path");
    }

    #[test]
    fn test_try_from_uri_to_path_error_paq() {
        // Test error case when URI has no path and query
        let uri = Uri::default().with_base(BaseUri::from_static("https://example.com/"));

        let result: Result<PathAndQuery, UriError> = uri.try_into();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not have a path and query component"));
    }

    #[test]
    fn test_uri_with_base_uri_only_to_string() {
        // Test None branch (line 126) in to_string() method
        let base_uri = BaseUri::from_static("https://example.com/api/");
        let uri = Uri::default().with_base(base_uri);

        let uri_string = uri.to_string();
        assert_eq!(uri_string.declassify_ref(), "https://example.com/api/");
    }

    #[test]
    fn test_uri_with_base_uri_only_redacted_display() {
        // Test None branch (line 166) in RedactedDisplay::fmt() method
        let base_uri = BaseUri::from_static("https://example.com/api/v1/");
        let uri = Uri::default().with_base(base_uri);

        let redaction_engine = RedactionEngine::builder().build();
        let redacted = uri.to_redacted_string(&redaction_engine);

        assert_eq!(redacted, "https://example.com/api/v1/");
    }
}
