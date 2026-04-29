// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::fmt;
use std::fmt::Formatter;
use std::ops::Deref;
use std::sync::Arc;

use data_privacy::{Classified, RedactedDebug, RedactedDisplay, RedactedToString, RedactionEngine, Sensitive};
use http::uri::PathAndQuery as HttpPathAndQuery;

use crate::error::UriError;
use crate::{PathAndQueryTemplate, Uri};

/// Path and query component of a [`Uri`].
///
/// Despite the name, a `PathAndQuery` represents both the path and the optional query
/// string portion of a URI (everything after the authority and before any
/// fragment, e.g. `/api/v1/users?active=true`).
///
/// `PathAndQuery` wraps either a static [`http::uri::PathAndQuery`] or a dynamic value
/// produced by a [`PathAndQueryTemplate`] implementation. Use the `from_*` constructors
/// or `From` impls to build one; the internal representation is intentionally
/// not exposed.
#[derive(Clone)]
pub struct PathAndQuery(PathAndQueryInner);

#[derive(Clone)]
enum PathAndQueryInner {
    Static(Sensitive<HttpPathAndQuery>),
    Templated(Arc<dyn PathAndQueryTemplate>),
}

impl PathAndQuery {
    /// Creates a new `PathAndQuery` from a [`PathAndQueryTemplate`].
    pub fn from_template(template: impl PathAndQueryTemplate) -> Self {
        Self(PathAndQueryInner::Templated(Arc::new(template)))
    }

    /// Creates a new `PathAndQuery` from a static path and query string.
    #[must_use]
    pub fn from_static(path: &'static str) -> Self {
        Self::from(HttpPathAndQuery::from_static(path))
    }

    /// Returns the template string for this path and query.
    #[must_use]
    pub fn template(&self) -> Cow<'static, str> {
        match &self.0 {
            PathAndQueryInner::Static(classified_pq) => Cow::Owned(classified_pq.declassify_ref().to_string()),
            PathAndQueryInner::Templated(templated) => Cow::Borrowed(templated.template()),
        }
    }

    /// Returns an optional label for this path and query.
    /// For templated paths with a label configured, this returns that label.
    /// For non-templated paths, this returns `None`.
    #[must_use]
    pub fn label(&self) -> Option<Cow<'static, str>> {
        match &self.0 {
            PathAndQueryInner::Static(_) => None,
            PathAndQueryInner::Templated(templated) => templated.label().map(Cow::Borrowed),
        }
    }

    /// Returns the path and query as a [`Sensitive`] string, classified under [`Uri::DATA_CLASS`].
    ///
    /// This shadows [`ToString::to_string`] to ensure callers receive a classified value
    /// rather than a plain `String`. Use [`Sensitive::declassify_ref`] (or the
    /// [`RedactedDisplay`] impl) when you need access to the underlying text.
    pub fn to_string(&self) -> Sensitive<String> {
        let s = match &self.0 {
            PathAndQueryInner::Static(classified_pq) => classified_pq.declassify_ref().to_string(),
            PathAndQueryInner::Templated(templated) => templated.render(),
        };
        Sensitive::new(s, Uri::DATA_CLASS)
    }
}

impl RedactedDisplay for PathAndQuery {
    #[cfg_attr(test, mutants::skip)] // Do not mutate display output.
    fn fmt(&self, engine: &RedactionEngine, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.0 {
            PathAndQueryInner::Static(classified_pq) => {
                // We can't use to_string in redaction because it automatically prepends a slash if the path doesn't start with one.
                // as_str doesn't do that, so we declassify to get the inner PathAndQuery and then use as_str.
                let reclassified = Sensitive::new(classified_pq.declassify_ref().as_str(), classified_pq.data_class().clone());
                RedactedDisplay::fmt(&reclassified, engine, f)
            }
            PathAndQueryInner::Templated(templated) => RedactedDisplay::fmt(&**templated, engine, f),
        }
    }
}

impl fmt::Debug for PathAndQuery {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut tuple = f.debug_tuple("PathAndQuery");
        match &self.0 {
            PathAndQueryInner::Static(_) => tuple.finish(),
            PathAndQueryInner::Templated(templated) => tuple.field(templated).finish(),
        }
    }
}

impl RedactedDebug for PathAndQuery {
    #[cfg_attr(test, mutants::skip)] // Do not mutate debug output.
    fn fmt(&self, engine: &RedactionEngine, f: &mut Formatter<'_>) -> fmt::Result {
        let mut tuple = f.debug_tuple("PathAndQuery");
        match &self.0 {
            PathAndQueryInner::Static(_) => tuple.finish(),
            PathAndQueryInner::Templated(templated) => {
                let rendered = templated.deref().to_redacted_string(engine);
                tuple.field(&rendered).finish()
            }
        }
    }
}

impl TryFrom<Uri> for PathAndQuery {
    type Error = UriError;

    /// Extracts the [`PathAndQuery`] component from a [`Uri`].
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the URI does not contain a path-and-query component.
    fn try_from(uri: Uri) -> Result<Self, Self::Error> {
        uri.path_and_query
            .ok_or_else(|| UriError::invalid_uri("URI does not have a path and query component"))
    }
}

impl From<HttpPathAndQuery> for PathAndQuery {
    fn from(value: HttpPathAndQuery) -> Self {
        Self(PathAndQueryInner::Static(Sensitive::new(value, Uri::DATA_CLASS)))
    }
}

impl TryFrom<&PathAndQuery> for HttpPathAndQuery {
    type Error = UriError;

    /// Materializes the [`PathAndQuery`] into a validated [`http::uri::PathAndQuery`].
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the underlying templated path renders to a value that
    /// is not a valid path-and-query.
    fn try_from(value: &PathAndQuery) -> Result<Self, Self::Error> {
        match &value.0 {
            PathAndQueryInner::Static(classified_pq) => Ok(classified_pq.declassify_ref().clone()),
            PathAndQueryInner::Templated(templated) => templated.to_path_and_query(),
        }
    }
}

impl TryFrom<PathAndQuery> for HttpPathAndQuery {
    type Error = UriError;

    /// Materializes the [`PathAndQuery`] into a validated [`http::uri::PathAndQuery`].
    fn try_from(value: PathAndQuery) -> Result<Self, Self::Error> {
        Self::try_from(&value)
    }
}

impl From<PathAndQuery> for Uri {
    fn from(value: PathAndQuery) -> Self {
        Self::new().with_path_and_query(value)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::BaseUri;

    #[test]
    fn from_path_and_query_roundtrip() {
        let path = HttpPathAndQuery::from_str("/path/to/resource?query=param").unwrap();
        let target_path: PathAndQuery = path.clone().into();
        assert_eq!(target_path.template(), "/path/to/resource?query=param");
        assert_eq!(target_path.to_string().declassify_ref(), "/path/to/resource?query=param");
        assert_eq!(HttpPathAndQuery::try_from(&target_path).unwrap(), path);
        assert_eq!(
            Uri::from(target_path.clone()).to_string(),
            Uri::default().with_path_and_query(target_path).to_string()
        );
    }

    #[test]
    fn try_from_uri_without_path_errors() {
        let uri = Uri::default().with_base(BaseUri::from_static("https://example.com/"));

        let result: Result<PathAndQuery, UriError> = uri.try_into();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not have a path and query component"));
    }

    #[test]
    fn try_from_uri_with_path_succeeds() {
        let path = HttpPathAndQuery::from_static("/test/path?query=value");
        let uri = Uri::default().with_path_and_query(path);

        let target_paq: PathAndQuery = uri.try_into().unwrap();
        assert_eq!(target_paq.to_string().declassify_ref(), "/test/path?query=value");
    }

    #[test]
    fn try_from_owned_uri_path_to_path_and_query() {
        let path = HttpPathAndQuery::from_static("/owned/path?query=value");
        let target_path: PathAndQuery = path.clone().into();

        // Owned conversion.
        let converted: HttpPathAndQuery = HttpPathAndQuery::try_from(target_path.clone()).unwrap();
        assert_eq!(converted, path);

        // Ensure owned and borrowed conversions agree.
        let converted_ref: HttpPathAndQuery = HttpPathAndQuery::try_from(&target_path).unwrap();
        assert_eq!(converted, converted_ref);
    }
}
