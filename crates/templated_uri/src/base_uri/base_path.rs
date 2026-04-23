// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Display;
use std::str::FromStr;

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

    pub(crate) fn join(&self, other: impl TryInto<PathAndQuery, Error: Into<http::Error>>) -> Result<PathAndQuery, UriError> {
        let other = other.try_into().map_err(|e| UriError::from(e.into()))?;
        self.join_path_and_query(other)
    }

    pub(crate) fn join_path_and_query(&self, other: PathAndQuery) -> Result<PathAndQuery, UriError> {
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

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let p: PathAndQuery = s.try_into()?;
        Self::new(p)
    }
}

impl TryFrom<&str> for BasePath {
    type Error = UriError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_str(value)
    }
}

impl TryFrom<String> for BasePath {
    type Error = UriError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let p = PathAndQuery::try_from(value)?;
        Self::new(p)
    }
}

impl TryFrom<PathAndQuery> for BasePath {
    type Error = UriError;

    fn try_from(paq: PathAndQuery) -> Result<Self, Self::Error> {
        Self::new(paq)
    }
}

impl TryFrom<&PathAndQuery> for BasePath {
    type Error = UriError;

    fn try_from(paq: &PathAndQuery) -> Result<Self, Self::Error> {
        paq.clone().try_into()
    }
}

impl Display for BasePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner.as_str())
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
mod test {
    use ohno::ErrorExt;

    use super::*;

    #[test]
    fn valid_path() {
        let path: BasePath = "/valid/path/".parse().unwrap();
        assert_eq!(path.as_str(), "/valid/path/");
    }

    #[test]
    fn invalid_path_no_slashes() {
        let err = BasePath::try_from("invalid/path").unwrap_err();
        assert_eq!(err.message(), "the path must start and end with a slash");
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
        let joined = base_path.join("/additional/resource?param=value").unwrap();
        assert_eq!(
            joined.as_str(),
            "/base/path/additional/resource?param=value",
            "Path join with slash prefixed uri string should result in correct concatenation"
        );

        let joined = base_path.join("additional/resource?param=value").unwrap();
        assert_eq!(
            joined.as_str(),
            "/base/path/additional/resource?param=value",
            "Path join with uri string missing the slash prefix should also result in correct concatenation"
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
