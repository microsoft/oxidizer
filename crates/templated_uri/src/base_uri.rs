// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{Debug, Display};
use std::str::FromStr;

use http::uri::{Authority, Parts, PathAndQuery, Scheme};

use crate::origin::{HTTP_DEFAULT_PORT, HTTPS_DEFAULT_PORT};
use crate::{BasePath, Origin, UriError};

/// URI prefix consisting of a scheme, an authority, and an optional path prefix.
///
/// `BaseUri` represents a target location to which paths can be appended. Query and
/// fragment components are intentionally not part of a base URI.
///
/// Use [`BaseUri`] when you need to:
///
/// - Store a base server location that will be combined with different paths.
/// - Manage a list of server locations or configure one in a library API where
///   the consumer only cares about the target.
///
/// # Best Practices
///
/// For optimal performance:
///
/// - Pre-create and cache static paths using `PathAndQuery::from_static`.
/// - Combine cached paths with [`BaseUri::build_http_uri`] to avoid re-parsing.
/// - Consider declaring frequently used paths as `const`s in your application.
///
/// ```rust
/// # use templated_uri::{BaseUri, Scheme}; use http::uri::PathAndQuery;
/// let api_path = PathAndQuery::from_static("/api/v1/resources");
/// let users_path = PathAndQuery::from_static("/api/v1/users");
///
/// let base_uri = BaseUri::from_static("https://api.example.com");
///
/// let uri = base_uri.build_http_uri(api_path)?;
/// assert_eq!(uri.to_string(), "https://api.example.com/api/v1/resources");
///
/// let uri = base_uri.build_http_uri(users_path)?;
/// assert_eq!(uri.to_string(), "https://api.example.com/api/v1/users");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
///
/// # Examples
///
/// Constructing a [`BaseUri`]:
///
/// ```
/// # use templated_uri::{BasePath, BaseUri, Origin};
/// // From an Origin (scheme + authority)
/// let base_uri1: BaseUri = Origin::from_static("https://example.com").into();
/// assert_eq!(base_uri1.to_string(), "https://example.com/");
///
/// // From an Origin and a path prefix
/// let origin = Origin::from_static("http://api.example.com:8080");
/// let base_uri2 = BaseUri::from_parts(origin, BasePath::default());
/// assert_eq!(base_uri2.to_string(), "http://api.example.com:8080/");
///
/// // From a URI string with a path prefix
/// let base_uri3: BaseUri = "https://auth.example.com/path/prefix/".parse()?;
/// assert_eq!(base_uri3.to_string(), "https://auth.example.com/path/prefix/");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
///
/// Building a complete URI from a [`BaseUri`]:
///
/// ```
/// # use templated_uri::BaseUri;
/// let base_uri = BaseUri::from_static("https://api.example.com");
/// let uri = base_uri.build_http_uri("/users/123?active=true")?;
/// assert_eq!(uri.to_string(), "https://api.example.com/users/123?active=true");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BaseUri {
    /// The origin part of the URI, consisting of scheme and authority.
    origin: Origin,
    /// The path prefix of the URI; always starts and ends with a slash.
    ///
    /// This is intentionally a [`BasePath`] (not `Option<BasePath>`): [`http::Uri`]
    /// parses an absent path (`http://example.com`) as `/`, so an explicit empty
    /// state would behave inconsistently with the underlying type.
    path: BasePath,
}

impl BaseUri {
    /// Sets the path component of this `BaseUri` and returns the updated value.
    ///
    /// The path must start and end with a slash (`/`).
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, BasePath};
    /// let base_uri = BaseUri::from_static("https://example.com")
    ///     .with_path(BasePath::from_static("/api/v1/"));
    ///
    /// assert_eq!(base_uri.to_string(), "https://example.com/api/v1/");
    /// ```
    #[must_use]
    pub fn with_path(self, path: impl Into<BasePath>) -> Self {
        Self { path: path.into(), ..self }
    }

    /// Sets the path of this `BaseUri` from a value that may fail to convert
    /// into a [`BasePath`] (for example a `&str`), and returns the updated value.
    ///
    /// For inputs that already implement `Into<BasePath>` (such as a [`BasePath`]
    /// itself), prefer the infallible [`BaseUri::with_path`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::BaseUri;
    /// let base_uri = BaseUri::from_static("https://example.com")
    ///     .try_with_path("/api/v1/")?;
    ///
    /// assert_eq!(base_uri.to_string(), "https://example.com/api/v1/");
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the path cannot be converted to a valid [`BasePath`].
    pub fn try_with_path<P>(self, path: P) -> Result<Self, UriError>
    where
        P: TryInto<BasePath>,
        UriError: From<<P as TryInto<BasePath>>::Error>,
    {
        Ok(Self {
            path: path.try_into()?,
            ..self
        })
    }

    /// Creates a [`BaseUri`] from an [`Origin`] and a [`BasePath`].
    ///
    /// This constructor is infallible - both inputs are already validated.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, BasePath, Origin};
    /// let origin = Origin::from_static("https://example.com:1234");
    /// let base_uri = BaseUri::from_parts(origin, BasePath::default());
    /// assert_eq!(base_uri.to_string(), "https://example.com:1234/");
    /// ```
    pub fn from_parts(origin: impl Into<Origin>, path: impl Into<BasePath>) -> Self {
        Self {
            origin: origin.into(),
            path: path.into(),
        }
    }

    /// Consumes the `BaseUri` and returns its [`Origin`] and [`BasePath`] components.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, BasePath, Origin};
    /// let base_uri = BaseUri::from_static("https://example.com:1234/api/");
    /// let (origin, path) = base_uri.into_parts();
    /// assert_eq!(origin, Origin::from_static("https://example.com:1234"));
    /// assert_eq!(path, BasePath::from_static("/api/"));
    /// ```
    #[must_use]
    pub fn into_parts(self) -> (Origin, BasePath) {
        (self.origin, self.path)
    }

    /// Creates a [`BaseUri`] from a scheme, host, port, and path.
    ///
    /// This is a convenience constructor for the common case where the host and
    /// port are available as separate values. The `path` must start and end with a
    /// slash (`/`). For pre-typed components, prefer [`BaseUri::from_parts`].
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the scheme conversion fails, the host is invalid,
    /// or the path is not a valid [`BasePath`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, Scheme, BasePath};
    /// let base_uri =
    ///     BaseUri::try_from_raw_parts(Scheme::HTTPS, "example.com", 1234, BasePath::default())?;
    /// assert_eq!(base_uri.to_string(), "https://example.com:1234/");
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    pub fn try_from_raw_parts(
        scheme: impl TryInto<Scheme, Error: Into<http::Error>>,
        host: impl AsRef<str>,
        port: u16,
        path: impl TryInto<BasePath, Error: Into<UriError>>,
    ) -> Result<Self, UriError> {
        let scheme = scheme.try_into().map_err(|e| UriError::from(e.into()))?;
        let authority: Authority = format!("{}:{}", host.as_ref(), port).parse()?;
        let path = path.try_into().map_err(Into::into)?;
        Ok(Self::from_parts(Origin::from_parts(scheme, authority), path))
    }

    /// Creates a [`BaseUri`] by parsing a static URI string.
    ///
    /// The URI must contain both a scheme and an authority component. Any query
    /// string or fragment is silently discarded; only the scheme, authority, and
    /// path prefix are preserved.
    ///
    /// # Panics
    ///
    /// Panics if the string is not a valid URI with both a scheme and an authority.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, Scheme};
    /// let base_uri = BaseUri::from_static("https://example.com:443");
    /// assert_eq!(base_uri.to_string(), "https://example.com/");
    /// ```
    #[must_use]
    #[expect(clippy::expect_used, reason = "from_static is documented to panic on invalid input")]
    pub fn from_static(uri: &'static str) -> Self {
        Self::try_from(&http::Uri::from_static(uri)).expect("static str is not a valid base URI")
    }

    /// Returns a reference to the scheme component of this [`BaseUri`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, Scheme};
    /// let base_uri = BaseUri::from_static("https://example.com");
    /// assert_eq!(base_uri.scheme().as_str(), "https");
    /// ```
    pub const fn scheme(&self) -> &Scheme {
        self.origin.scheme()
    }

    /// Returns a reference to the authority component of this [`BaseUri`].
    ///
    /// The authority typically consists of a hostname and optional port.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, Scheme};
    /// let base_uri = BaseUri::from_static("https://example.com");
    /// assert_eq!(base_uri.authority().as_str(), "example.com");
    /// ```
    ///
    /// ```
    /// # use templated_uri::{BaseUri, Scheme};
    /// let base_uri = BaseUri::from_static("https://example.com:1234");
    /// assert_eq!(base_uri.authority().as_str(), "example.com:1234");
    /// ```
    pub const fn authority(&self) -> &Authority {
        self.origin.authority()
    }

    /// Returns the host part of this [`BaseUri`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, Scheme};
    /// let base_uri = BaseUri::from_static("https://example.com:443");
    /// assert_eq!(base_uri.host(), "example.com");
    /// ```
    pub fn host(&self) -> &str {
        self.origin.authority().host()
    }

    /// Returns the origin of this [`BaseUri`] in the form `scheme://authority`.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// Returns a new [`BaseUri`] with `origin` replacing this one's origin.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, Origin, Scheme, Authority};
    /// let base_uri = BaseUri::from_static("https://example.com:443");
    /// let new_base_uri = base_uri.with_origin(Origin::from_parts(
    ///     Scheme::HTTPS,
    ///     Authority::from_static("new-example.com:8080"),
    /// ));
    /// assert_eq!(new_base_uri.to_string(), "https://new-example.com:8080/");
    /// ```
    #[must_use]
    pub fn with_origin(self, origin: Origin) -> Self {
        Self { origin, path: self.path }
    }

    /// Returns the port of this [`BaseUri`].
    ///
    /// Returns the explicit port from the authority when present. For HTTP and
    /// HTTPS, the well-known default port is inferred from the scheme when no
    /// port is specified. For other schemes without an explicit port this
    /// method returns `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, Scheme};
    /// // Explicit port
    /// let base_uri = BaseUri::from_static("https://example.com:8443");
    /// assert_eq!(base_uri.port(), Some(8443));
    ///
    /// // Default HTTPS port
    /// let base_uri = BaseUri::from_static("https://example.com");
    /// assert_eq!(base_uri.port(), Some(443));
    ///
    /// // Default HTTP port
    /// let base_uri = BaseUri::from_static("http://example.com");
    /// assert_eq!(base_uri.port(), Some(80));
    /// ```
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        self.origin.port()
    }

    /// Returns a new [`BaseUri`] with the given port.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, Scheme};
    /// let mut base_uri = BaseUri::from_static("https://example.com");
    /// assert_eq!(base_uri.port(), Some(443));
    ///
    /// let base_uri = base_uri.with_port(8443);
    /// assert_eq!(base_uri.port(), Some(8443));
    /// assert_eq!(base_uri.to_string(), "https://example.com:8443/");
    /// ```
    #[must_use]
    pub fn with_port(self, port: u16) -> Self {
        Self {
            origin: self.origin.with_port(port),
            path: self.path,
        }
    }

    /// Returns a reference to the path prefix of this [`BaseUri`].
    ///
    /// The path is guaranteed to start and end with a slash (`/`).
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, Scheme};
    /// let base_uri = BaseUri::from_static("https://example.com/some/path/");
    ///
    /// assert_eq!(base_uri.path().as_str(), "/some/path/");
    /// ```
    ///
    /// ```
    /// # use templated_uri::{BaseUri, Scheme};
    /// let base_uri = BaseUri::from_static("https://example.com");
    ///
    /// assert_eq!(base_uri.path().as_str(), "/");
    /// ```
    pub const fn path(&self) -> &BasePath {
        &self.path
    }

    /// Returns `true` if this [`BaseUri`] uses the HTTPS scheme.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, Scheme};
    /// let secure = BaseUri::from_static("https://example.com");
    /// assert!(secure.is_https());
    ///
    /// let insecure = BaseUri::from_static("http://example.com");
    /// assert!(!insecure.is_https());
    /// ```
    pub fn is_https(&self) -> bool {
        self.origin.is_https()
    }

    /// Builds a complete [`http::Uri`] by appending `path` to this base.
    ///
    /// The resulting URI uses the scheme, authority, and path prefix of this base,
    /// with `path` (a path or path-and-query) appended.
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if `path` cannot be converted into a [`PathAndQuery`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, Scheme}; use http::uri::PathAndQuery;
    /// let base_uri = BaseUri::from_static("https://example.com");
    /// let uri = base_uri.build_http_uri("/api/resource?param=value")?;
    ///
    /// assert_eq!(
    ///     uri.to_string(),
    ///     "https://example.com/api/resource?param=value"
    /// );
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// Using a path prefix as a part of the [`BaseUri`]:
    /// ```
    /// # use templated_uri::{BaseUri, Scheme}; use http::uri::PathAndQuery;
    /// let base_uri = BaseUri::from_static("https://example.com/api/");
    /// let uri = base_uri.build_http_uri("resource?param=value")?;
    ///
    /// assert_eq!(
    ///     uri.to_string(),
    ///     "https://example.com/api/resource?param=value"
    /// );
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// Using a pre-existing `PathAndQuery`:
    ///
    /// ```
    /// # use templated_uri::{BaseUri, Scheme}; use http::uri::PathAndQuery;
    /// let base_uri = BaseUri::from_static("https://example.com");
    ///
    /// // Pre-create and cache path and query to avoid parsing and extra allocations.
    /// let path = PathAndQuery::from_static("/api/resource?param=value");
    ///
    /// let uri = base_uri.build_http_uri(path)?;
    /// assert_eq!(
    ///     uri.to_string(),
    ///     "https://example.com/api/resource?param=value"
    /// );
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    pub fn build_http_uri(&self, path: impl TryInto<PathAndQuery, Error: Into<http::Error>>) -> Result<http::Uri, UriError> {
        let path = path.try_into().map_err(|e| UriError::from(e.into()))?;
        self.build_http_uri_inner(&path)
    }

    fn build_http_uri_inner(&self, path: &PathAndQuery) -> Result<http::Uri, UriError> {
        let full_path = self.path.join_path_and_query(path)?;

        let mut parts = Parts::default();
        parts.scheme = Some(self.scheme().clone());
        parts.authority = Some(self.authority().clone());
        parts.path_and_query = Some(full_path);

        http::Uri::from_parts(parts).map_err(Into::into)
    }
}

impl TryFrom<http::Uri> for BaseUri {
    type Error = UriError;

    /// Tries to convert a URI into a `BaseUri`.
    ///
    /// Any query string or fragment in the URI is silently discarded - only the scheme,
    /// authority and path are used.
    ///
    /// # Errors
    ///
    /// Returns an error if the URI does not have both scheme and authority components.
    fn try_from(uri: http::Uri) -> Result<Self, Self::Error> {
        Self::try_from(&uri)
    }
}

impl TryFrom<&http::Uri> for BaseUri {
    type Error = UriError;

    /// Tries to convert a URI reference into a `BaseUri`.
    ///
    /// Any query string or fragment in the URI is silently discarded - only the scheme,
    /// authority and path are used.
    ///
    /// # Errors
    ///
    /// Returns an error if the URI does not have both scheme and authority components.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::BaseUri;
    /// let uri = "https://example.com/path/?query=1".parse::<http::Uri>()?;
    /// let base_uri = BaseUri::try_from(&uri)?;
    /// assert_eq!(base_uri.to_string(), "https://example.com/path/");
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    fn try_from(uri: &http::Uri) -> Result<Self, Self::Error> {
        let (Some(scheme), Some(authority)) = (uri.scheme(), uri.authority()) else {
            return Err(UriError::invalid_uri("URI must have both scheme and authority components"));
        };

        // Use only the path component - query and fragment are not part of a base URI.
        let path = match uri.path() {
            "" | "/" => BasePath::default(),
            p => BasePath::try_from(p)?,
        };

        Ok(Self::from_parts(Origin::from_parts(scheme.clone(), authority.clone()), path))
    }
}

impl TryFrom<&str> for BaseUri {
    type Error = UriError;

    /// Parses a [`BaseUri`] from a string slice.
    ///
    /// # Errors
    ///
    /// See [`BaseUri::from_str`].
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<Origin> for BaseUri {
    /// Converts an [`Origin`] into a [`BaseUri`] with a root path (`/`).
    fn from(origin: Origin) -> Self {
        Self {
            origin,
            path: BasePath::default(),
        }
    }
}

impl FromStr for BaseUri {
    type Err = UriError;

    /// Parses a [`BaseUri`] from a string.
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the string is not a valid URI, or if it does not contain
    /// both a scheme and an authority.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        http::Uri::from_str(s)?.try_into()
    }
}

impl From<BaseUri> for http::Uri {
    /// Converts a [`BaseUri`] into an [`http::Uri`], using `/` as the root path when no prefix is set.
    fn from(value: BaseUri) -> Self {
        let (scheme, authority) = value.origin.into_parts();
        let mut parts = Parts::default();
        parts.scheme = Some(scheme);
        parts.authority = Some(authority);
        parts.path_and_query = Some(value.path.into());

        Self::from_parts(parts).expect("all inputs are already validated, this call never fails")
    }
}

impl Display for BaseUri {
    /// Formats the [`BaseUri`] as `scheme://authority/base_path/`.
    ///
    /// The [`BasePath`] always starts and ends with `/`, so the minimal form is
    /// `scheme://authority/` when no path prefix is set. The default ports for
    /// HTTP (80) and HTTPS (443) are omitted; custom ports are included.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::BaseUri;
    /// let base_uri = BaseUri::from_static("https://example.com:443");
    /// assert_eq!(format!("{}", base_uri), "https://example.com/");
    ///
    /// let custom_port = BaseUri::from_static("https://example.com:8443");
    /// assert_eq!(format!("{}", custom_port), "https://example.com:8443/");
    /// ```
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://", self.scheme())?;

        match (self.scheme().as_str(), self.authority().port_u16()) {
            (s, Some(HTTP_DEFAULT_PORT)) if s == Scheme::HTTP.as_str() => write!(f, "{}", self.host())?,
            (s, Some(HTTPS_DEFAULT_PORT)) if s == Scheme::HTTPS.as_str() => write!(f, "{}", self.host())?,
            _ => write!(f, "{}", self.authority())?,
        }
        write!(f, "{}", self.path)
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for BaseUri {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for BaseUri {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod from_parts {
        use super::*;

        #[test]
        fn valid_base_uri() {
            let origin = Origin::from_parts(Scheme::HTTPS, Authority::from_static("example.com"));
            let base_uri = BaseUri::from_parts(origin, BasePath::default());
            assert_eq!(base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(base_uri.authority().as_str(), "example.com");
        }

        #[test]
        fn with_custom_port() {
            let origin = Origin::from_parts(Scheme::HTTP, Authority::from_static("example.com:8080"));
            let base_uri = BaseUri::from_parts(origin, BasePath::default());
            assert_eq!(base_uri.scheme(), &Scheme::HTTP);
            assert_eq!(base_uri.authority().as_str(), "example.com:8080");
        }

        #[test]
        fn non_http_scheme_is_accepted() {
            let origin = Origin::from_parts(Scheme::try_from("ftp").unwrap(), Authority::from_static("example.com:21"));
            let base_uri = BaseUri::from_parts(origin, BasePath::default());
            assert_eq!(base_uri.scheme().as_str(), "ftp");
            assert_eq!(base_uri.port(), Some(21));
            assert_eq!(base_uri.to_string(), "ftp://example.com:21/");
        }

        #[test]
        fn with_path() {
            let origin = Origin::from_parts(Scheme::HTTPS, Authority::from_static("example.com:443"));
            let base_uri = BaseUri::from_parts(origin, BasePath::from_static("/example/"));
            assert_eq!(base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(base_uri.authority().as_str(), "example.com:443");
            assert_eq!(base_uri.to_string(), "https://example.com/example/");
        }
    }

    #[test]
    fn try_from_raw_parts_builds_uri_with_try_into_args() {
        // Exercises both `TryInto<Scheme>` (via `&str`) and `TryInto<BasePath>` (via `&str`).
        let base_uri = BaseUri::try_from_raw_parts("https", "example.com", 1234, "/api/v1/").unwrap();
        assert_eq!(base_uri.to_string(), "https://example.com:1234/api/v1/");

        // Non-HTTP schemes are now accepted.
        let base_uri = BaseUri::try_from_raw_parts("ftp", "example.com", 21, BasePath::default()).unwrap();
        assert_eq!(base_uri.scheme().as_str(), "ftp");
        assert_eq!(base_uri.port(), Some(21));

        // An invalid host still produces an error.
        BaseUri::try_from_raw_parts("https", "not a host", 1234, BasePath::default()).unwrap_err();
    }

    mod from_uri_static {
        use super::*;

        #[test]
        fn valid_uri() {
            let base_uri = BaseUri::from_static("https://example.com");
            assert_eq!(base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(base_uri.authority().as_str(), "example.com");
        }

        #[test]
        fn with_path() {
            let base_uri = BaseUri::from_static("https://example.com/path/to/resource/");
            assert_eq!(base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(base_uri.authority().as_str(), "example.com");
            assert_eq!(base_uri.to_string(), "https://example.com/path/to/resource/");
        }

        #[should_panic(expected = "static str is not a valid base URI")]
        #[test]
        fn invalid_uri() {
            let _base_uri = BaseUri::from_static("not-a-valid-uri");
        }
    }

    mod from_uri_str {
        use ohno::ErrorExt;

        use super::*;

        #[test]
        fn valid_uri() {
            let base_uri = BaseUri::from_str("https://example.com/").unwrap();
            assert_eq!(base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(base_uri.authority().as_str(), "example.com");
        }

        #[test]
        fn with_path() {
            let base_uri = BaseUri::from_str("https://example.com/path/").unwrap();
            assert_eq!(base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(base_uri.authority().as_str(), "example.com");
        }

        #[test]
        fn strips_query_from_path() {
            let base_uri = BaseUri::from_str("https://example.com/path/?query=1&other=2").unwrap();
            assert_eq!(base_uri.to_string(), "https://example.com/path/");
        }

        #[test]
        fn invalid_uri() {
            let err = BaseUri::from_str("not-a-valid-uri").unwrap_err();
            assert_eq!(err.message(), "URI must have both scheme and authority components");
        }
    }

    mod from_uri {
        use super::*;

        #[test]
        fn valid_uri() {
            let uri = http::Uri::from_static("https://example.com");
            let base_uri = BaseUri::try_from(&uri).unwrap();
            assert_eq!(base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(base_uri.authority().as_str(), "example.com");
        }

        #[test]
        fn with_path() {
            let uri = http::Uri::from_static("https://example.com/path/");
            let base_uri = BaseUri::try_from(&uri).unwrap();
            assert_eq!(base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(base_uri.authority().as_str(), "example.com");
            assert_eq!(base_uri.path().as_str(), "/path/");
        }

        #[test]
        fn strips_query_from_path() {
            let uri = http::Uri::from_static("https://example.com/path/?query=1");
            let base_uri = BaseUri::try_from(&uri).unwrap();
            assert_eq!(base_uri.to_string(), "https://example.com/path/");
        }

        #[test]
        fn missing_components() {
            let uri = http::Uri::from_static("/just-a-path");
            let err = BaseUri::try_from(&uri).unwrap_err();
            assert!(err.to_string().contains("URI must have both scheme and authority"));
        }
    }

    mod with_path {
        use super::*;

        #[test]
        fn replaces_path_infallibly() {
            let base_uri = BaseUri::from_static("https://example.com/old/").with_path(BasePath::from_static("/api/v1/"));
            assert_eq!(base_uri.to_string(), "https://example.com/api/v1/");
        }

        #[test]
        fn try_with_path_from_str() {
            let base_uri = BaseUri::from_static("https://example.com/").try_with_path("/api/v1/").unwrap();
            assert_eq!(base_uri.to_string(), "https://example.com/api/v1/");
        }

        #[test]
        fn try_with_path_invalid_returns_error() {
            BaseUri::from_static("https://example.com/")
                .try_with_path("no-leading-slash/")
                .unwrap_err();
        }
    }

    #[test]
    fn into_parts_round_trips_with_from_parts() {
        let base_uri = BaseUri::from_static("https://example.com:1234/api/");
        let (origin, path) = base_uri.clone().into_parts();
        assert_eq!(origin, Origin::from_static("https://example.com:1234"));
        assert_eq!(path, BasePath::from_static("/api/"));
        assert_eq!(BaseUri::from_parts(origin, path), base_uri);
    }

    #[test]
    fn try_from_str_delegates_to_from_str() {
        let base_uri = BaseUri::try_from("https://example.com/api/").unwrap();
        assert_eq!(base_uri.to_string(), "https://example.com/api/");

        BaseUri::try_from("not-a-valid-uri").unwrap_err();
    }

    mod accessors {
        use super::*;

        #[test]
        fn scheme() {
            let base_uri = BaseUri::from_static("https://example.com");
            assert_eq!(base_uri.scheme().as_str(), "https");
        }

        #[test]
        fn authority() {
            let base_uri = BaseUri::from_static("https://example.com:8443");
            assert_eq!(base_uri.authority().as_str(), "example.com:8443");
        }

        #[test]
        fn host() {
            let base_uri = BaseUri::from_static("https://example.com:8443");
            assert_eq!(base_uri.host(), "example.com");
        }

        #[test]
        fn port_explicit() {
            let base_uri = BaseUri::from_static("https://example.com:8443");
            assert_eq!(base_uri.port(), Some(8443));
        }

        #[test]
        fn port_default_https() {
            let base_uri = BaseUri::from_static("https://example.com");
            assert_eq!(base_uri.port(), Some(443));
        }

        #[test]
        fn port_default_http() {
            let base_uri = BaseUri::from_static("http://example.com");
            assert_eq!(base_uri.port(), Some(80));
        }
    }

    mod is_https {
        use super::*;

        #[test]
        fn secure() {
            let base_uri = BaseUri::from_static("https://example.com");
            assert!(base_uri.is_https());
        }

        #[test]
        fn insecure() {
            let base_uri = BaseUri::from_static("http://example.com");
            assert!(!base_uri.is_https());
        }
    }

    mod build_uri {
        use super::*;

        #[test]
        fn with_path_string() {
            let base_uri = BaseUri::from_static("https://example.com");
            let uri = base_uri.build_http_uri("/api/resource").unwrap();
            assert_eq!(uri.to_string(), "https://example.com/api/resource");
        }

        #[test]
        fn with_empty_uri() {
            let base_uri = BaseUri::from_static("https://example.com");
            let uri = base_uri.build_http_uri("/").unwrap();
            assert_eq!(uri.to_string(), "https://example.com/");
        }

        #[test]
        fn with_path_query_string() {
            let base_uri = BaseUri::from_static("https://example.com");
            let uri = base_uri.build_http_uri("/api/resource?param=value").unwrap();
            assert_eq!(uri.to_string(), "https://example.com/api/resource?param=value");
        }

        #[test]
        fn with_path_object() {
            let base_uri = BaseUri::from_static("https://example.com");
            let path = PathAndQuery::from_static("/api/resource?param=value");
            let uri = base_uri.build_http_uri(path).unwrap();
            assert_eq!(uri.to_string(), "https://example.com/api/resource?param=value");
        }

        #[test]
        fn invalid_path() {
            let base_uri = BaseUri::from_static("https://example.com");
            let invalid_path = "some path/?invalid\\character";
            let err = base_uri.build_http_uri(invalid_path).unwrap_err();
            assert!(err.to_string().contains("invalid uri character"));
        }
    }

    mod conversions {
        use super::*;

        #[test]
        fn uri_to_base_uri() {
            let uri = http::Uri::from_static("https://example.com/path/");
            let base_uri: BaseUri = uri.try_into().unwrap();
            assert_eq!(base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(base_uri.authority().as_str(), "example.com");
        }

        #[test]
        fn base_uri_to_uri() {
            let base_uri = BaseUri::from_static("https://example.com");
            let uri: http::Uri = base_uri.into();
            assert_eq!(uri.to_string(), "https://example.com/");
        }

        #[test]
        fn origin_to_base_uri() {
            let origin = Origin::from_static("https://example.com:8443");
            let base_uri: BaseUri = origin.into();

            assert_eq!(base_uri.to_string(), "https://example.com:8443/");
        }

        #[test]
        fn from_str_valid() {
            let base_uri: BaseUri = "https://example.com:8443/api/".parse().unwrap();

            assert_eq!(base_uri.to_string(), "https://example.com:8443/api/");
        }

        #[test]
        fn from_str_invalid() {
            let err = "not-a-valid-uri".parse::<BaseUri>().unwrap_err();
            assert!(err.to_string().contains("URI must have both scheme and authority"));
        }
    }

    mod display {
        use super::*;

        #[test]
        fn http_default_port() {
            let base_uri = BaseUri::from_static("http://example.com:80");
            assert_eq!(base_uri.to_string(), "http://example.com/");
        }

        #[test]
        fn https_default_port() {
            let base_uri = BaseUri::from_static("https://example.com:443");
            assert_eq!(base_uri.to_string(), "https://example.com/");
        }

        #[test]
        fn custom_port() {
            let base_uri = BaseUri::from_static("https://example.com:8443");
            assert_eq!(base_uri.to_string(), "https://example.com:8443/");
        }
    }

    mod with_origin {
        use super::*;

        #[test]
        fn replaces_origin() {
            let base_uri = BaseUri::from_static("https://example.com/api/");
            let new_origin = Origin::from_static("https://new-example.com:8080");

            let new_base_uri = base_uri.with_origin(new_origin.clone());

            assert_eq!(new_base_uri.origin(), &new_origin);
            assert_eq!(new_base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(new_base_uri.authority().as_str(), "new-example.com:8080");
            assert_eq!(new_base_uri.port(), Some(8080));
            assert_eq!(new_base_uri.path().as_str(), "/api/");
            assert_eq!(new_base_uri.to_string(), "https://new-example.com:8080/api/");
        }
    }

    mod with_port {
        use super::*;

        #[test]
        fn changes_port() {
            let base_uri = BaseUri::from_static("https://example.com/api/");

            let new_base_uri = base_uri.with_port(8443);

            assert_eq!(new_base_uri.origin().port(), Some(8443));
            assert_eq!(new_base_uri.port(), Some(8443));
            assert_eq!(new_base_uri.to_string(), "https://example.com:8443/api/");
        }
    }

    #[cfg(feature = "serde")]
    mod serde_tests {
        use super::*;

        #[test]
        fn base_uri_roundtrip() {
            let original = BaseUri::from_static("https://example.com:8443/api/");
            let json = serde_json::to_string(&original).unwrap();
            assert_eq!(json, r#""https://example.com:8443/api/""#);
            let deserialized: BaseUri = serde_json::from_str(&json).unwrap();
            assert_eq!(original, deserialized);
        }

        #[test]
        fn base_uri_deserialize_rejects_invalid() {
            serde_json::from_str::<BaseUri>(r#""not-a-uri""#).unwrap_err();
        }
    }
}
