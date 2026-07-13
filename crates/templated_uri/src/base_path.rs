// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Debug, Display, Formatter};
use std::str::FromStr;

use data_privacy::{RedactedDebug, RedactedDisplay, Redactor};
use http::uri::PathAndQuery;

use crate::UriError;

/// The base of a Uri, like `/foo`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BasePath {
    inner: PathAndQuery,
}

impl BasePath {
    /// Returns the path as a string slice.
    pub fn as_str(&self) -> &str {
        self.inner.as_str()
    }

    /// Creates a new `BasePath` by parsing a static string.
    ///
    /// # Panics
    ///
    /// Panics if the string is not a valid base path (must start and end with `/` and
    /// must not contain a query string). Intended for use with compile-time-known
    /// constants; use [`BasePath::from_str`](std::str::FromStr::from_str) or the
    /// `TryFrom<&str>` impl for fallible parsing.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::BasePath;
    /// let path = BasePath::from_static("/api/v1/");
    /// assert_eq!(path.as_str(), "/api/v1/");
    /// ```
    #[must_use]
    #[expect(clippy::expect_used, reason = "from_static is documented to panic on invalid input")]
    pub fn from_static(s: &'static str) -> Self {
        Self::new(PathAndQuery::from_static(s)).expect("invalid base path passed to BasePath::from_static")
    }

    fn validate_path_format(&self) -> Result<(), UriError> {
        let path_str = self.inner.as_str();
        if self.inner.query().is_some() {
            return Err(UriError::invalid_uri("the path must not contain a query string"));
        }
        if !(path_str.starts_with('/') && path_str.ends_with('/')) {
            return Err(UriError::invalid_uri("the path must start and end with a slash"));
        }

        Ok(())
    }

    /// Creates a new `Path` from a `PathAndQuery`, validating its format.
    /// The path must start and end with a slash (`/`) and must not contain a query string.
    fn new(p: PathAndQuery) -> Result<Self, UriError> {
        let path = Self { inner: p };
        path.validate_path_format()?;
        Ok(path)
    }

    pub(crate) fn join_path_and_query(&self, other: &PathAndQuery) -> Result<PathAndQuery, UriError> {
        let base = self.as_str();
        let other_str = other.as_str();
        let path_str = other_str.trim_start_matches('/');
        if path_str.is_empty() {
            return Ok(self.inner.clone());
        }
        // Fast path: when the base path is just the root `/` (the common case, e.g. a base
        // URL like `https://api.example.com` with no path prefix) and the rendered path has
        // exactly one leading slash, the join is byte-identical to `other` - which is already
        // a validated `http::PathAndQuery`. Return it directly, skipping both the string
        // allocation and the redundant re-validation scan (`clone` is a cheap `Bytes`
        // reference-count bump). A difference of exactly one byte between `other_str` and the
        // slash-trimmed `path_str` means exactly one leading slash was trimmed, so
        // `"/" + path_str == other_str` (`path_str` is always a suffix of `other_str`, so the
        // subtraction never underflows).
        if base == "/" && other_str.len().saturating_sub(path_str.len()) == 1 {
            return Ok(other.clone());
        }
        // General path: capacity-hinted `push_str` join rather than `format!`: sizes the
        // buffer once and skips the formatting machinery, and the resulting `String` is
        // donated to `PathAndQuery::try_from` without a re-copy. This is the request hot-path
        // join used when reusing an already-rendered path (see `Uri::to_http_uri`).
        let mut full_path = String::with_capacity(base.len() + path_str.len());
        full_path.push_str(base);
        full_path.push_str(path_str);
        Ok(PathAndQuery::try_from(full_path)?)
    }

    /// Joins this base path with a [`crate::PathAndQuery`], rendering and validating in a
    /// single pass.
    ///
    /// This is the request hot-path equivalent of [`join_path_and_query`](Self::join_path_and_query):
    /// rather than materializing the other path into a validated [`PathAndQuery`] first (an
    /// allocation plus a full URI parse) and then re-joining and re-parsing, it renders the
    /// other path's text directly into a buffer already seeded with this base path and
    /// validates the joined result exactly once.
    ///
    /// The join semantics match [`join_path_and_query`](Self::join_path_and_query): the base
    /// path always ends with `/`, and any leading slashes of the rendered path are trimmed so
    /// exactly one separator sits at the boundary (a rendered path of only slashes collapses
    /// to the bare base path).
    ///
    /// # Errors
    ///
    /// The rendered path must be absolute (start with `/`), which every template the crate
    /// produces satisfies (the `#[templated]` macro requires a leading `/`, and static paths
    /// are validated to have one). This is rejected up front so that materializing a `Uri`
    /// *with* a base accepts exactly the same paths as materializing one *without* it (where
    /// [`to_path_and_query`](crate::PathAndQueryTemplate::to_path_and_query) parses the
    /// rendered path standalone and `http` rejects a path-and-query without a leading slash) -
    /// a base must never turn an otherwise-invalid path into a valid URI. A [`UriError`] is
    /// also returned if the joined result is not a valid path-and-query.
    pub(crate) fn join_rendered(&self, other: &crate::PathAndQuery) -> Result<PathAndQuery, UriError> {
        let base = self.as_str();
        let mut buf = String::with_capacity(base.len() + other.render_capacity_hint());
        buf.push_str(base);

        // Everything appended from here is the rendered "other" path; `mark` is where it starts.
        let mark = buf.len();
        other.render_into(&mut buf);

        // The rendered path must be absolute, matching the standalone validation done when
        // there is no base, so a base never rescues an invalid path. An empty render (which
        // does not start with `/`) is rejected here too, consistent with `http`.
        if !buf[mark..].starts_with('/') {
            return Err(UriError::invalid_uri("the rendered path and query must start with a slash"));
        }

        // Reproduce `base + rendered.trim_start_matches('/')`: the base already ends with a
        // slash, so drop the rendered path's leading slashes at the boundary.
        match buf[mark..].bytes().position(|b| b != b'/') {
            Some(first_non_slash) => {
                drop(buf.drain(mark..mark + first_non_slash));
            }
            // The rendered path was all slashes: the join is just the base path.
            None => return Ok(self.inner.clone()),
        }

        Ok(PathAndQuery::try_from(buf)?)
    }
}

impl Default for BasePath {
    fn default() -> Self {
        Self {
            inner: PathAndQuery::from_static("/"),
        }
    }
}

impl FromStr for BasePath {
    type Err = UriError;

    /// Parses a [`BasePath`] from a string.
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the string is not a valid path-and-query, contains a query string,
    /// or does not start and end with a slash (`/`).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let p: PathAndQuery = s.try_into()?;
        Self::new(p)
    }
}

impl TryFrom<&str> for BasePath {
    type Error = UriError;

    /// Parses a [`BasePath`] from a string slice.
    ///
    /// # Errors
    ///
    /// See [`BasePath::from_str`].
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_str(value)
    }
}

impl TryFrom<String> for BasePath {
    type Error = UriError;

    /// Parses a [`BasePath`] from an owned `String`, reusing the allocation when possible.
    ///
    /// # Errors
    ///
    /// See [`BasePath::from_str`].
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let p = PathAndQuery::try_from(value)?;
        Self::new(p)
    }
}

impl TryFrom<PathAndQuery> for BasePath {
    type Error = UriError;

    /// Validates the given [`PathAndQuery`] as a [`BasePath`].
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the value contains a query string or does not start and end with a slash (`/`).
    fn try_from(paq: PathAndQuery) -> Result<Self, Self::Error> {
        Self::new(paq)
    }
}

impl TryFrom<&PathAndQuery> for BasePath {
    type Error = UriError;

    /// Validates a borrowed [`PathAndQuery`] as a [`BasePath`].
    fn try_from(paq: &PathAndQuery) -> Result<Self, Self::Error> {
        paq.clone().try_into()
    }
}

impl Display for BasePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner.as_str())
    }
}

impl RedactedDisplay for BasePath {
    fn fmt(&self, _redactor: &dyn Redactor, f: &mut Formatter<'_>) -> fmt::Result {
        // A base path is static configuration rather than request data, so it is
        // rendered unredacted, consistently with how `Uri` formats its base components.
        Display::fmt(self, f)
    }
}

impl RedactedDebug for BasePath {
    fn fmt(&self, _redactor: &dyn Redactor, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(self, f)
    }
}

impl From<BasePath> for PathAndQuery {
    fn from(base_path: BasePath) -> Self {
        base_path.inner
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for BasePath {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for BasePath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use ohno::ErrorExt;

    use super::*;

    #[test]
    fn valid_path() {
        let path: BasePath = "/valid/path/".parse().unwrap();
        assert_eq!(path.as_str(), "/valid/path/");
    }

    #[test]
    fn invalid_path_no_slashes() {
        use ohno::Labeled;
        let err = BasePath::try_from("invalid/path").unwrap_err();
        assert_eq!(err.label(), "uri_invalid");
    }

    #[test]
    fn invalid_path_no_trailing_slash() {
        let err = BasePath::try_from("/invalid/path").unwrap_err();
        assert_eq!(err.message(), "the path must start and end with a slash");
    }

    #[test]
    fn invalid_path_with_query() {
        let err = BasePath::try_from("/invalid/path?query=1").unwrap_err();
        assert_eq!(err.message(), "the path must not contain a query string");
    }

    #[test]
    fn path_join() {
        let base_path = BasePath::from_static("/base/path/");
        let joined = base_path
            .join_path_and_query(&"/additional/resource?param=value".parse().unwrap())
            .unwrap();
        assert_eq!(
            joined.as_str(),
            "/base/path/additional/resource?param=value",
            "Path join with slash prefixed uri string should result in correct concatenation"
        );
    }

    #[test]
    fn join_rendered_matches_join_path_and_query_for_static_paths() {
        // `join_rendered` (single render+join+validate pass, used on the request hot path)
        // must produce byte-identical results to the original `join_path_and_query`
        // (materialize-then-join) across a broad range of bases and paths, including the
        // multiple-leading-slash and empty cases the join must normalize identically.
        let bases = ["/", "/api/", "/api/v1/", "/deep/nested/base/"];
        let paths = [
            "/",
            "/users",
            "/users/42",
            "/users/42?active=true",
            "/a/b/c?x=1&y=2",
            "//double",      // 2 leading slashes must collapse to one at the boundary
            "///triple/seg", // 3 leading slashes must collapse to one
            "/trailing/",
            "/only?q=1",
        ];

        for base_str in bases {
            let base = BasePath::from_static(base_str);
            for path_str in paths {
                let http_pq = PathAndQuery::from_static(path_str);
                let templated_pq = crate::PathAndQuery::from(http_pq.clone());

                let old = base.join_path_and_query(&http_pq).expect("old join should succeed");
                let new = base.join_rendered(&templated_pq).expect("new join should succeed");

                assert_eq!(
                    new.as_str(),
                    old.as_str(),
                    "join mismatch for base {base_str:?} + path {path_str:?}"
                );
            }
        }
    }

    #[test]
    fn join_root_base_returns_path_unchanged() {
        // Fast path: joining an already-validated path onto a root (`/`) base path yields the
        // path unchanged (the optimization then returns it directly, skipping the re-allocation
        // and re-validation scan; that allocation/scan property is verified by the Callgrind
        // benches rather than asserted here, to avoid coupling to `http`'s internal storage).
        let base = BasePath::from_static("/");
        let path = PathAndQuery::from_static("/users/42?active=true");

        let joined = base.join_path_and_query(&path).expect("join should succeed");

        assert_eq!(joined.as_str(), "/users/42?active=true");
    }

    #[test]
    fn join_root_base_double_slash_takes_general_path() {
        // A path with multiple leading slashes must NOT take the reuse fast path: the join
        // collapses them to one, so the result differs from the input and is re-validated.
        let base = BasePath::from_static("/");
        let path = PathAndQuery::from_static("//double//slash");
        let joined = base.join_path_and_query(&path).expect("join should succeed");
        assert_eq!(joined.as_str(), "/double//slash", "leading slashes must collapse to one");
    }

    #[test]
    fn try_from_path_ref() {
        let paq = PathAndQuery::from_static("/ref/path/");
        let path = BasePath::try_from(&paq).unwrap();
        assert_eq!(path.as_str(), "/ref/path/");
    }

    /// A hand-written [`crate::PathAndQueryTemplate`] whose `render` output does *not* start
    /// with `/`, used to check that a base does not rescue an otherwise-invalid path.
    #[derive(Debug)]
    struct NonAbsoluteTemplate;

    impl RedactedDisplay for NonAbsoluteTemplate {
        fn fmt(&self, _redactor: &dyn Redactor, f: &mut Formatter<'_>) -> fmt::Result {
            f.write_str("users/42")
        }
    }

    impl crate::PathAndQueryTemplate for NonAbsoluteTemplate {
        fn render(&self) -> String {
            // Deliberately non-absolute (no leading slash), violating the template contract.
            String::from("users/42")
        }

        fn to_path_and_query(&self) -> Result<PathAndQuery, UriError> {
            Ok(PathAndQuery::try_from(self.render())?)
        }

        fn template(&self) -> &'static str {
            "users/42"
        }

        fn format_template(&self) -> &'static str {
            "users/42"
        }

        fn label(&self) -> Option<&'static str> {
            None
        }
    }

    #[test]
    fn join_rendered_rejects_non_absolute_path() {
        // A rendered path that is not absolute must be rejected, so that materializing a `Uri`
        // with a base is consistent with `to_path_and_query()` (which also rejects it): a base
        // must never turn an invalid path into a valid one.
        let base = BasePath::from_static("/api/");
        let bad = crate::PathAndQuery::from_template(NonAbsoluteTemplate);

        // Standalone validation rejects it...
        crate::PathAndQueryTemplate::to_path_and_query(&NonAbsoluteTemplate).unwrap_err();

        // ...and so does the base-join hot path, with a descriptive message.
        let err = base.join_rendered(&bad).unwrap_err();
        assert!(err.to_string().contains("must start with a slash"), "unexpected error: {err}");
    }

    #[test]
    fn join_rendered_accepts_absolute_rendered_path() {
        // The positive counterpart: an absolute rendered path joins successfully. Guards
        // against a mutant that inverts the leading-slash check and rejects valid paths.
        let base = BasePath::from_static("/api/");
        let good = crate::PathAndQuery::from(PathAndQuery::from_static("/users/42"));
        let joined = base.join_rendered(&good).expect("absolute path should join");
        assert_eq!(joined.as_str(), "/api/users/42");
    }

    #[test]
    fn non_absolute_template_trait_methods() {
        // Exercise the remaining `PathAndQueryTemplate` / `RedactedDisplay` methods of the
        // `NonAbsoluteTemplate` test helper so it is fully covered.
        use data_privacy::{RedactedToString, RedactionEngine};

        use crate::PathAndQueryTemplate;

        let template = NonAbsoluteTemplate;
        assert_eq!(template.render(), "users/42");
        assert_eq!(template.template(), "users/42");
        assert_eq!(template.format_template(), "users/42");
        assert_eq!(template.label(), None);

        let engine = RedactionEngine::builder().build();
        assert_eq!(template.to_redacted_string(&engine), "users/42");
    }

    #[test]
    fn try_from_path_ref_invalid() {
        let paq: PathAndQuery = "/no-trailing-slash".parse().unwrap();
        let err = BasePath::try_from(&paq).unwrap_err();
        assert!(err.to_string().contains("the path must start and end with a slash"));
    }

    #[test]
    fn from_string() {
        let s = String::from("/string/path/");
        let p = s.as_bytes().as_ptr();
        let path = BasePath::try_from(s).unwrap();
        assert_eq!(path.as_str(), "/string/path/");
        let p2 = path.as_str().as_ptr();
        assert_eq!(p, p2, "The string data should not be copied");
    }

    #[test]
    fn from_static_valid() {
        let path = BasePath::from_static("/api/v1/");
        assert_eq!(path.as_str(), "/api/v1/");
    }

    #[test]
    fn redacted_display_matches_display() {
        use data_privacy::{RedactedToString, RedactionEngine};

        let engine = RedactionEngine::builder().build();
        let path = BasePath::from_static("/api/v1/");

        // A base path carries no request data, so redaction is a no-op and renders
        // identically to `Display`, consistently with the `Uri` redaction impl.
        assert_eq!(path.to_redacted_string(&engine), path.to_string());
        assert_eq!(path.to_redacted_string(&engine), "/api/v1/");
    }

    #[test]
    fn redacted_debug_matches_debug() {
        use data_privacy::RedactionEngine;

        let engine = RedactionEngine::builder().build();
        let path = BasePath::from_static("/api/v1/");

        let mut redacted = String::new();
        engine.redacted_debug(&path, &mut redacted).unwrap();
        assert_eq!(redacted, format!("{path:?}"));
    }

    #[test]
    #[should_panic(expected = "invalid base path passed to BasePath::from_static")]
    fn from_static_invalid() {
        let _ = BasePath::from_static("/no-trailing-slash");
    }

    #[cfg(feature = "serde")]
    mod serde_tests {
        use super::*;

        #[test]
        fn base_path_roundtrip() {
            let original: BasePath = "/api/v1/".parse().unwrap();
            let json = serde_json::to_string(&original).unwrap();
            assert_eq!(json, r#""/api/v1/""#);
            let deserialized: BasePath = serde_json::from_str(&json).unwrap();
            assert_eq!(original, deserialized);
        }

        #[test]
        fn base_path_deserialize_rejects_invalid() {
            serde_json::from_str::<BasePath>(r#""no-slashes""#).unwrap_err();
        }
    }
}
