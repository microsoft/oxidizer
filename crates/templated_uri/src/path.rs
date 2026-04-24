// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::fmt;
use std::fmt::Formatter;
use std::ops::Deref;
use std::sync::Arc;

use data_privacy::{Classified, RedactedDebug, RedactedDisplay, RedactedToString, RedactionEngine, Sensitive};
use http::uri::PathAndQuery;

use crate::error::UriError;
use crate::{PathTemplate, Uri};

/// Path and query component of a [`Uri`].
///
/// Despite the name, a `Path` represents both the path and the optional query
/// string portion of a URI (everything after the authority and before any
/// fragment, e.g. `/api/v1/users?active=true`).
///
/// `Path` wraps either a static [`PathAndQuery`] or a dynamic value
/// produced by a [`PathTemplate`] implementation. Use the `from_*` constructors
/// or `From` impls to build one; the internal representation is intentionally
/// not exposed.
#[derive(Clone)]
pub struct Path(PathInner);

#[derive(Clone)]
enum PathInner {
    Static(Sensitive<PathAndQuery>),
    Templated(Arc<dyn PathTemplate>),
}

impl Path {
    /// Creates a new `Path` from a [`PathTemplate`].
    pub fn from_template(template: impl PathTemplate) -> Self {
        Self(PathInner::Templated(Arc::new(template)))
    }

    /// Creates a new `Path` from a static path and query string.
    #[must_use]
    pub fn from_static(path: &'static str) -> Self {
        Self::from(PathAndQuery::from_static(path))
    }

    /// Returns the template string for this path and query.
    #[must_use]
    pub fn template(&self) -> Cow<'static, str> {
        match &self.0 {
            PathInner::Static(classified_pq) => Cow::Owned(classified_pq.declassify_ref().to_string()),
            PathInner::Templated(templated) => Cow::Borrowed(templated.template()),
        }
    }

    /// Returns an optional label for this path and query.
    /// For templated paths with a label configured, this returns that label.
    /// For non-templated paths, this returns `None`.
    #[must_use]
    pub fn label(&self) -> Option<Cow<'static, str>> {
        match &self.0 {
            PathInner::Static(_) => None,
            PathInner::Templated(templated) => templated.label().map(Cow::Borrowed),
        }
    }

    /// Returns the path and query as a [`Sensitive`] string, classified under [`Uri::DATA_CLASS`].
    ///
    /// This shadows [`ToString::to_string`] to ensure callers receive a classified value
    /// rather than a plain `String`. Use [`Sensitive::declassify_ref`] (or the
    /// [`RedactedDisplay`] impl) when you need access to the underlying text.
    pub fn to_string(&self) -> Sensitive<String> {
        let s = match &self.0 {
            PathInner::Static(classified_pq) => classified_pq.declassify_ref().to_string(),
            PathInner::Templated(templated) => templated.render(),
        };
        Sensitive::new(s, Uri::DATA_CLASS)
    }
}

impl RedactedDisplay for Path {
    #[cfg_attr(test, mutants::skip)] // Do not mutate display output.
    fn fmt(&self, engine: &RedactionEngine, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.0 {
            PathInner::Static(classified_pq) => {
                // We can't use to_string in redaction because it automatically prepends a slash if the path doesn't start with one.
                // as_str doesn't do that, so we declassify to get the inner PathAndQuery and then use as_str.
                let reclassified = Sensitive::new(classified_pq.declassify_ref().as_str(), classified_pq.data_class().clone());
                f.write_str(&engine.redacted_to_string(&reclassified))
            }
            PathInner::Templated(templated) => RedactedDisplay::fmt(&**templated, engine, f),
        }
    }
}

impl fmt::Debug for Path {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut tuple = f.debug_tuple("Path");
        match &self.0 {
            PathInner::Static(_) => tuple.finish(),
            PathInner::Templated(templated) => tuple.field(templated).finish(),
        }
    }
}

impl RedactedDebug for Path {
    #[cfg_attr(test, mutants::skip)] // Do not mutate debug output.
    fn fmt(&self, engine: &RedactionEngine, f: &mut Formatter<'_>) -> fmt::Result {
        let mut tuple = f.debug_tuple("Path");
        match &self.0 {
            PathInner::Static(_) => tuple.finish(),
            PathInner::Templated(templated) => {
                let rendered = templated.deref().to_redacted_string(engine);
                tuple.field(&rendered).finish()
            }
        }
    }
}

impl TryFrom<Uri> for Path {
    type Error = UriError;

    /// Extracts the [`Path`] component from a [`Uri`].
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the URI does not contain a path-and-query component.
    fn try_from(uri: Uri) -> Result<Self, Self::Error> {
        uri.path
            .ok_or_else(|| UriError::invalid_uri("URI does not have a path and query component"))
    }
}

impl From<PathAndQuery> for Path {
    fn from(value: PathAndQuery) -> Self {
        Self(PathInner::Static(Sensitive::new(value, Uri::DATA_CLASS)))
    }
}

impl TryFrom<&Path> for PathAndQuery {
    type Error = UriError;

    /// Materializes the [`Path`] into a validated [`PathAndQuery`].
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the underlying templated path renders to a value that
    /// is not a valid path-and-query.
    fn try_from(value: &Path) -> Result<Self, Self::Error> {
        match &value.0 {
            PathInner::Static(classified_pq) => Ok(classified_pq.declassify_ref().clone()),
            PathInner::Templated(templated) => templated.to_path_and_query(),
        }
    }
}

impl TryFrom<Path> for PathAndQuery {
    type Error = UriError;

    /// Materializes the [`Path`] into a validated [`PathAndQuery`].
    fn try_from(value: Path) -> Result<Self, Self::Error> {
        Self::try_from(&value)
    }
}

impl From<Path> for Uri {
    fn from(value: Path) -> Self {
        Self::new().with_path(value)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::BaseUri;

    #[test]
    fn from_path_and_query_roundtrip() {
        let path = PathAndQuery::from_str("/path/to/resource?query=param").unwrap();
        let target_path: Path = path.clone().into();
        assert_eq!(target_path.template(), "/path/to/resource?query=param");
        assert_eq!(target_path.to_string().declassify_ref(), "/path/to/resource?query=param");
        assert_eq!(PathAndQuery::try_from(&target_path).unwrap(), path);
        assert_eq!(
            Uri::from(target_path.clone()).to_string(),
            Uri::default().with_path(target_path).to_string()
        );
    }

    #[test]
    fn try_from_uri_without_path_errors() {
        let uri = Uri::default().with_base(BaseUri::from_static("https://example.com/"));

        let result: Result<Path, UriError> = uri.try_into();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not have a path and query component"));
    }

    #[test]
    fn try_from_uri_with_path_succeeds() {
        let path = PathAndQuery::from_static("/test/path?query=value");
        let uri = Uri::default().with_path(path);

        let target_paq: Path = uri.try_into().unwrap();
        assert_eq!(target_paq.to_string().declassify_ref(), "/test/path?query=value");
    }

    #[test]
    fn try_from_owned_uri_path_to_path_and_query() {
        let path = PathAndQuery::from_static("/owned/path?query=value");
        let target_path: Path = path.clone().into();

        // Owned conversion.
        let converted: PathAndQuery = PathAndQuery::try_from(target_path.clone()).unwrap();
        assert_eq!(converted, path);

        // Ensure owned and borrowed conversions agree.
        let converted_ref: PathAndQuery = PathAndQuery::try_from(&target_path).unwrap();
        assert_eq!(converted, converted_ref);
    }
}
