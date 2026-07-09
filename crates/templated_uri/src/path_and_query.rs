// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::fmt;
use std::fmt::Formatter;
use std::ops::Deref;
use std::sync::Arc;

use data_privacy::{Classified, RedactedDebug, RedactedDisplay, RedactedToString, Redactor, Sensitive};
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

    /// Appends this path-and-query's rendered text to `buf`.
    ///
    /// For a static value this is a single `push_str`; for a templated value it renders
    /// the fields directly into `buf`, avoiding the intermediate `String` that
    /// [`to_string`](Self::to_string) would allocate. Used by base-URI joining on the
    /// request hot path.
    pub(crate) fn render_into(&self, buf: &mut String) {
        match &self.0 {
            PathAndQueryInner::Static(classified_pq) => buf.push_str(classified_pq.declassify_ref().as_str()),
            PathAndQueryInner::Templated(templated) => templated.render_into(buf),
        }
    }

    /// Returns a heuristic byte-capacity estimate for [`render_into`](Self::render_into),
    /// so a caller can size its buffer to avoid reallocating mid-render.
    pub(crate) fn render_capacity_hint(&self) -> usize {
        match &self.0 {
            PathAndQueryInner::Static(classified_pq) => classified_pq.declassify_ref().as_str().len(),
            PathAndQueryInner::Templated(templated) => templated.render_capacity_hint(),
        }
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
    fn fmt(&self, redactor: &dyn Redactor, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.0 {
            PathAndQueryInner::Static(classified_pq) => {
                // We can't use to_string in redaction because it automatically prepends a slash if the path doesn't start with one.
                // as_str doesn't do that, so we declassify to get the inner PathAndQuery and then use as_str.
                let reclassified = Sensitive::new(classified_pq.declassify_ref().as_str(), classified_pq.data_class().clone());
                RedactedDisplay::fmt(&reclassified, redactor, f)
            }
            PathAndQueryInner::Templated(templated) => RedactedDisplay::fmt(&**templated, redactor, f),
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
    fn fmt(&self, redactor: &dyn Redactor, f: &mut Formatter<'_>) -> fmt::Result {
        let mut tuple = f.debug_tuple("PathAndQuery");
        match &self.0 {
            PathAndQueryInner::Static(_) => tuple.finish(),
            PathAndQueryInner::Templated(templated) => {
                let rendered = templated.deref().to_redacted_string(redactor);
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

impl TryFrom<&str> for PathAndQuery {
    type Error = UriError;

    /// Parses a string into a [`PathAndQuery`].
    ///
    /// The input must start with `/` (per RFC 3986 `path-abempty`); inputs without a
    /// leading slash are rejected to avoid inconsistent rendering downstream.
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the string does not start with `/` or is not a valid
    /// path-and-query.
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Ok(Self::from(HttpPathAndQuery::try_from(value)?))
    }
}

impl TryFrom<String> for PathAndQuery {
    type Error = UriError;

    /// Parses an owned string into a [`PathAndQuery`], reusing its buffer.
    ///
    /// Prefer this over the `&str` overload when the string is already owned: the
    /// underlying [`http::uri::PathAndQuery`] takes the buffer without copying.
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the string does not start with `/` or is not a valid
    /// path-and-query.
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Ok(Self::from(HttpPathAndQuery::try_from(value)?))
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

/// Deserializes a [`PathAndQuery`] from a string, validating it via [`PathAndQuery::try_from`].
///
/// Deserialization always yields the static variant; a [`PathAndQueryTemplate`]
/// is never reconstructed from serialized data. Brace characters are accepted as
/// literal path content, so a string like `/users/{id}` deserializes into a
/// static path whose text is `/users/{id}` verbatim - it is *not* interpreted as
/// a template placeholder. Only a [`Deserialize`](serde::Deserialize) impl is
/// provided: [`PathAndQuery`] is privacy-classified and intentionally has no
/// plain [`Display`](std::fmt::Display)/[`Serialize`](serde::Serialize).
#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for PathAndQuery {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::try_from(s).map_err(serde::de::Error::custom)
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

    #[test]
    fn try_from_str_succeeds() {
        let target_path = PathAndQuery::try_from("/api/v1/users?active=true").unwrap();
        assert_eq!(target_path.to_string().declassify_ref(), "/api/v1/users?active=true");
    }

    #[test]
    fn try_from_str_invalid_errors() {
        use ohno::Labeled;
        let err = PathAndQuery::try_from("/invalid path\0").unwrap_err();
        assert_eq!(err.label(), "uri_invalid");
    }

    #[test]
    fn try_from_str_without_leading_slash_errors() {
        use ohno::Labeled;
        let err = PathAndQuery::try_from("api/v1/users").unwrap_err();
        assert_eq!(err.label(), "uri_invalid");
    }

    #[test]
    fn try_from_string_succeeds() {
        let target_path = PathAndQuery::try_from(String::from("/api/v1/users?active=true")).unwrap();
        assert_eq!(target_path.to_string().declassify_ref(), "/api/v1/users?active=true");
    }

    #[test]
    fn try_from_string_invalid_errors() {
        use ohno::Labeled;
        let err = PathAndQuery::try_from(String::from("api/v1/users")).unwrap_err();
        assert_eq!(err.label(), "uri_invalid");
    }

    /// Minimal hand-written [`PathAndQueryTemplate`] used to exercise the `Templated` arm of
    /// [`PathAndQuery::render_into`] / [`PathAndQuery::render_capacity_hint`] with values that
    /// differ from both `0` and `1` (so whole-body mutants are caught) and from the `Static`
    /// arm's string length (so an arm swap would change the observed result).
    #[derive(Debug)]
    struct FixedTemplate;

    impl RedactedDisplay for FixedTemplate {
        fn fmt(&self, _redactor: &dyn Redactor, f: &mut Formatter<'_>) -> fmt::Result {
            f.write_str("/fixed/template")
        }
    }

    impl PathAndQueryTemplate for FixedTemplate {
        fn render(&self) -> String {
            String::from("/fixed/template")
        }

        fn render_into(&self, buf: &mut String) {
            buf.push_str("/fixed/template");
        }

        fn render_capacity_hint(&self) -> usize {
            42
        }

        fn to_path_and_query(&self) -> Result<HttpPathAndQuery, UriError> {
            Ok(HttpPathAndQuery::try_from(self.render())?)
        }

        fn template(&self) -> &'static str {
            "/fixed/template"
        }

        fn format_template(&self) -> &'static str {
            "/fixed/template"
        }

        fn label(&self) -> Option<&'static str> {
            None
        }
    }

    #[test]
    fn render_into_static_appends_text() {
        // The `Static` arm appends the underlying path text verbatim to the caller's buffer,
        // preserving any existing prefix. Kills a mutant that replaces `render_into` with `()`.
        let pq = PathAndQuery::from_static("/api/v1/users?active=true");
        let mut buf = String::from("/base/");
        pq.render_into(&mut buf);
        assert_eq!(buf, "/base//api/v1/users?active=true");
    }

    #[test]
    fn render_into_templated_delegates() {
        // The `Templated` arm delegates to the template's `render_into`. Using a distinct
        // string guards against an arm swap and against dropping the append.
        let pq = PathAndQuery::from_template(FixedTemplate);
        let mut buf = String::from("/base/");
        pq.render_into(&mut buf);
        assert_eq!(buf, "/base//fixed/template");
    }

    #[test]
    fn render_capacity_hint_static_is_str_len() {
        // The `Static` arm reports the exact byte length of the path text. Asserting a value
        // that is neither `0` nor `1` kills the whole-body replacement mutants.
        let pq = PathAndQuery::from_static("/api/v1/");
        assert_eq!(pq.render_capacity_hint(), "/api/v1/".len());
        assert_eq!(pq.render_capacity_hint(), 8);
    }

    #[test]
    fn render_capacity_hint_templated_delegates() {
        // The `Templated` arm forwards to the template's hint (here a fixed `42`), distinct
        // from the `Static` arm's length computation so an arm swap is observable.
        let pq = PathAndQuery::from_template(FixedTemplate);
        assert_eq!(pq.render_capacity_hint(), 42);
    }
}

#[cfg(all(test, feature = "serde"))]
mod serde_tests {
    use super::PathAndQuery;

    #[test]
    fn deserialize_static_path_and_query() {
        let paq: PathAndQuery = serde_json::from_str(r#""/api/v1/users?active=true""#).unwrap();
        assert_eq!(paq.to_string().declassify_ref(), "/api/v1/users?active=true");
    }

    #[test]
    fn deserialize_rejects_missing_leading_slash() {
        serde_json::from_str::<PathAndQuery>(r#""api/v1/users""#).unwrap_err();
    }

    #[test]
    fn deserialize_error_does_not_leak_input() {
        // `UriError` must never echo the raw input, preserving the privacy posture.
        let err = serde_json::from_str::<PathAndQuery>(r#""SECRETPATH_no_slash""#).unwrap_err();
        assert!(
            !err.to_string().contains("SECRETPATH"),
            "deserialize error must not leak the raw input"
        );
    }

    #[test]
    fn path_and_query_does_not_implement_serialize() {
        // Deserialize-only is intentional for this privacy-classified type.
        static_assertions::assert_not_impl_any!(PathAndQuery: serde::Serialize);
    }

    #[test]
    fn deserialize_braces_are_literal_static_content() {
        // A `PathAndQueryTemplate` is never reconstructed from serialized data.
        // Braces are valid path characters, so `/users/{id}` deserializes into a
        // static path containing the literal text `{id}`, not a template placeholder.
        let paq: PathAndQuery = serde_json::from_str(r#""/users/{id}""#).unwrap();
        assert_eq!(paq.to_string().declassify_ref(), "/users/{id}");
        assert!(paq.label().is_none(), "deserialized value must be a static path, not a template");
    }
}
