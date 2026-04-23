// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::fmt;
use std::fmt::{Debug, Formatter};
use std::ops::Deref;
use std::sync::Arc;

use data_privacy::{Classified, RedactedToString, RedactionEngine, Sensitive};
use http::uri::PathAndQuery;

use crate::error::UriError;
use crate::{Uri, UriTemplate};

/// Path and query component of a [`Uri`].
///
/// `UriPath` wraps either a static [`PathAndQuery`] or a dynamic value
/// produced by a [`UriTemplate`] implementation. Use the `from_*` constructors
/// or `From` impls to build one; the internal representation is intentionally
/// not exposed.
#[derive(Clone)]
pub struct UriPath(UriPathInner);

#[derive(Clone)]
enum UriPathInner {
    Static(Sensitive<PathAndQuery>),
    Templated(Arc<dyn UriTemplate>),
}

impl UriPath {
    /// Creates a new `UriPath` from a [`UriTemplate`].
    pub fn from_template(template: impl UriTemplate) -> Self {
        Self(UriPathInner::Templated(Arc::new(template)))
    }

    /// Creates a new `UriPath` from a static path and query string.
    #[must_use]
    pub fn from_static(path: &'static str) -> Self {
        Self::from(PathAndQuery::from_static(path))
    }

    /// Returns the template string for this path and query.
    #[must_use]
    pub fn template(&self) -> Cow<'static, str> {
        match &self.0 {
            UriPathInner::Static(classified_pq) => Cow::Owned(classified_pq.clone().declassify_ref().to_string()),
            UriPathInner::Templated(templated) => Cow::Borrowed(templated.template()),
        }
    }

    /// Returns an optional label for this path and query.
    /// For templated paths with a label configured, this returns that label.
    /// For non-templated paths, this returns `None`.
    #[must_use]
    pub fn label(&self) -> Option<Cow<'static, str>> {
        match &self.0 {
            UriPathInner::Static(_) => None,
            UriPathInner::Templated(templated) => templated.label().map(Cow::Borrowed),
        }
    }

    /// Converts to a URI string.
    pub fn to_uri_string(&self) -> String {
        match &self.0 {
            UriPathInner::Static(classified_pq) => classified_pq.declassify_ref().to_string(),
            UriPathInner::Templated(templated) => templated.to_uri_string(),
        }
    }

    /// Converts to a redacted URI string using the provided redaction engine.
    pub fn to_uri_string_redacted(&self, redaction_engine: &RedactionEngine) -> String {
        match &self.0 {
            UriPathInner::Static(classified_pq) => {
                // We can't use to_string in redaction because it automatically prepends a slash if the path doesn't start with one.
                // as_str doesn't do that, so we declassify to get the inner PathAndQuery and then use as_str.
                let reclassified = Sensitive::new(classified_pq.declassify_ref().as_str(), classified_pq.data_class().clone());
                redaction_engine.redacted_to_string(&reclassified)
            }
            UriPathInner::Templated(templated) => templated.deref().to_redacted_string(redaction_engine),
        }
    }
}

impl Debug for UriPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut tuple = f.debug_tuple("UriPath");
        match &self.0 {
            UriPathInner::Static(_) => tuple.finish(),
            UriPathInner::Templated(templated) => tuple.field(templated).finish(),
        }
    }
}

impl TryFrom<Uri> for UriPath {
    type Error = UriError;
    fn try_from(uri: Uri) -> Result<Self, Self::Error> {
        uri.to_http_path()
            .map(Self::from)
            .ok_or_else(|| UriError::invalid_uri("URI does not have a path and query component"))
    }
}

impl From<PathAndQuery> for UriPath {
    fn from(value: PathAndQuery) -> Self {
        Self(UriPathInner::Static(Sensitive::new(value, Uri::DATA_CLASS)))
    }
}

impl TryFrom<&UriPath> for PathAndQuery {
    type Error = UriError;
    fn try_from(value: &UriPath) -> Result<Self, Self::Error> {
        match &value.0 {
            UriPathInner::Static(classified_pq) => Ok(classified_pq.declassify_ref().clone()),
            UriPathInner::Templated(templated) => templated.to_http_path(),
        }
    }
}

impl TryFrom<UriPath> for PathAndQuery {
    type Error = UriError;
    fn try_from(value: UriPath) -> Result<Self, Self::Error> {
        Self::try_from(&value)
    }
}

impl From<UriPath> for Uri {
    fn from(value: UriPath) -> Self {
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
        let target_path: UriPath = path.clone().into();
        assert_eq!(target_path.template(), "/path/to/resource?query=param");
        assert_eq!(target_path.to_uri_string(), "/path/to/resource?query=param");
        assert_eq!(PathAndQuery::try_from(&target_path).unwrap(), path);
        assert_eq!(
            Uri::from(target_path.clone()).to_string(),
            Uri::default().with_path(target_path).to_string()
        );
    }

    #[test]
    fn try_from_uri_without_path_errors() {
        let uri = Uri::default().with_base(BaseUri::from_static("https://example.com/"));

        let result: Result<UriPath, UriError> = uri.try_into();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not have a path and query component"));
    }

    #[test]
    fn try_from_uri_with_path_succeeds() {
        let path = PathAndQuery::from_static("/test/path?query=value");
        let uri = Uri::default().with_path(path);

        let target_paq: UriPath = uri.try_into().unwrap();
        assert_eq!(target_paq.to_uri_string(), "/test/path?query=value");
    }
}
