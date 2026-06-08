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
        let path_str = other.as_str().trim_start_matches('/');
        if path_str.is_empty() {
            return Ok(self.inner.clone());
        }
        let full_path = format!("{}{path_str}", self.as_str());
        Ok(PathAndQuery::try_from(full_path)?)
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
    /// Formats a [`BasePath`] through the redaction pipeline.
    ///
    /// Consistent with the [`Uri`](crate::Uri) [`RedactedDisplay`] impl, the base path is
    /// considered non-sensitive and is rendered using its [`Display`] representation.
    /// This impl exists so [`BasePath`] can be used in derive-based redaction without
    /// altering the rendered text.
    #[cfg_attr(test, mutants::skip)] // Do not mutate display output.
    fn fmt(&self, _redactor: &dyn Redactor, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

impl RedactedDebug for BasePath {
    /// Formats a [`BasePath`] through the redaction pipeline using its [`Debug`] form.
    ///
    /// A [`BasePath`] is considered non-sensitive, so redacted-debug formatting is
    /// identical to its [`Debug`] output. This impl exists so [`BasePath`] can be
    /// used as a field in derive-based [`RedactedDebug`] structs without altering
    /// the rendered text.
    #[cfg_attr(test, mutants::skip)] // Do not mutate debug output.
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
    fn try_from_path_ref() {
        let paq = PathAndQuery::from_static("/ref/path/");
        let path = BasePath::try_from(&paq).unwrap();
        assert_eq!(path.as_str(), "/ref/path/");
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
    #[should_panic(expected = "invalid base path passed to BasePath::from_static")]
    fn from_static_invalid() {
        let _ = BasePath::from_static("/no-trailing-slash");
    }

    #[test]
    fn redacted_display_preserves_path() {
        use data_privacy::{RedactedToString, RedactionEngine};

        let path = BasePath::from_static("/api/v1/");
        let engine = RedactionEngine::builder().build();

        // Consistent with the `Uri` impl, which renders the base portion via plain
        // `Display`: the base path text is preserved in the redacted form.
        assert_eq!(path.to_redacted_string(&engine), path.to_string());
        assert_eq!(path.to_redacted_string(&engine), "/api/v1/");
    }

    #[test]
    fn redacted_debug_matches_debug() {
        use data_privacy::RedactionEngine;

        let path = BasePath::from_static("/api/v1/");
        let engine = RedactionEngine::builder().build();

        let mut redacted = String::new();
        engine.redacted_debug(&path, &mut redacted).unwrap();
        assert_eq!(redacted, format!("{path:?}"));
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
