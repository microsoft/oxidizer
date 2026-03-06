// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Display;
use std::str::FromStr;

use http::uri::PathAndQuery;

use crate::ValidationError;

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

    fn validate_path_format(&self) -> Result<(), ValidationError> {
        let path_str = self.inner.as_str();
        if self.inner.query().is_some() {
            return Err(ValidationError::caused_by("the path must not contain a query string"));
        }
        if !(path_str.starts_with('/') && path_str.ends_with('/')) {
            return Err(ValidationError::caused_by("the path must start and end with a slash"));
        }

        Ok(())
    }

    /// Creates a new `Path` from a `PathAndQuery`, validating its format.
    /// The path must start and end with a slash (`/`) and must not contain a query string.
    fn new(p: PathAndQuery) -> Result<Self, ValidationError> {
        let path = Self { inner: p };
        path.validate_path_format()?;
        Ok(path)
    }

    pub(crate) fn join(&self, other: impl TryInto<PathAndQuery, Error: Into<http::Error>>) -> Result<PathAndQuery, ValidationError> {
        let other: PathAndQuery = other.try_into().map_err(Into::into)?;
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
    type Err = ValidationError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let p: PathAndQuery = s.try_into()?;
        Self::new(p)
    }
}

impl TryFrom<&str> for BasePath {
    type Error = ValidationError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_str(value)
    }
}

impl TryFrom<String> for BasePath {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let p = PathAndQuery::try_from(value)?;
        Self::new(p)
    }
}

impl TryFrom<PathAndQuery> for BasePath {
    type Error = ValidationError;

    fn try_from(paq: PathAndQuery) -> Result<Self, Self::Error> {
        Self::new(paq)
    }
}

impl TryFrom<&PathAndQuery> for BasePath {
    type Error = ValidationError;

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
        let base_path = BasePath::try_from(PathAndQuery::from_static("/base/path/")).unwrap();
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
    fn try_from_path_and_query_ref() {
        let paq = PathAndQuery::from_static("/ref/path/");
        let path = BasePath::try_from(&paq).unwrap();
        assert_eq!(path.as_str(), "/ref/path/");
    }

    #[test]
    fn try_from_path_and_query_ref_invalid() {
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
}
