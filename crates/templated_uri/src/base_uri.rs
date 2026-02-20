// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{Debug, Display};
use std::str::FromStr;

use http::uri::PathAndQuery;

use crate::ValidationError;
use crate::uri::{Authority, Parts, Scheme};

mod base_path;
mod origin;

pub use base_path::BasePath;
pub use origin::Origin;

/// An HTTP or HTTPS `base_uri` representing a target location without path information.
///
/// `base_uri` is a lightweight type that stores only the scheme and authority portions
/// of a URI, making it ideal for representing target destinations in HTTP scenarios.
/// It deliberately omits path, query string, and fragment components, focusing solely on
/// the remote target information.
///
/// Use `base_uri` when you need to:
///
/// - Store a base server location that will be combined with different paths.
/// - Handle scenarios where storing the full URI is unnecessary. For example, a list
///   of servers or `base_uri`.
/// - Expose a way to configure the base server location in your library. In this case,
///   the library manages its own paths and the consumer cares only about the target `base_uri`.
///
/// # Best Practices
///
/// For optimal performance when working with `base_uri`, follow these practices:
///
/// - Pre-create and cache static paths using `PathAndQuery::from_static` for frequently used paths.
/// - Combine static paths with `base_uri` to create complete URIs without allocations.
/// - Use the `build_uri` method with these cached paths instead of passing strings each time.
/// - Consider making common paths constants in your application code.
///
/// ```rust
/// # use templated_uri::{BaseUri, uri::{Scheme, PathAndQuery}};
///
/// // Pre-create PathAndQuery objects (can be static or stored in a cache)
/// let api_path = PathAndQuery::from_static("/api/v1/resources");
/// let users_path = PathAndQuery::from_static("/api/v1/users");
///
/// // Create base_uri once
/// let base_uri = BaseUri::from_uri_static("https://api.example.com");
///
/// // Combine with a path to create a complete URI
/// let uri = base_uri.build_http_uri(api_path)?;
/// assert_eq!(uri.to_string(), "https://api.example.com/api/v1/resources");
///
/// // Reuse the base_uri with different paths without additional allocations
/// let uri = base_uri.build_http_uri(users_path)?;
/// assert_eq!(uri.to_string(), "https://api.example.com/api/v1/users");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
///
/// # Examples
///
/// Creating `base_uri` with various constructors:
///
/// ```
/// # use templated_uri::{BasePath, BaseUri, uri::Scheme, };
/// // From scheme and authority
/// let base_uri1 = BaseUri::new(Scheme::HTTPS, "example.com")?;
/// assert_eq!(base_uri1.to_string(), "https://example.com/");
///
/// // From scheme, host, and port
/// let base_uri2 =
///     BaseUri::from_parts(Scheme::HTTP, "api.example.com", 8080, BasePath::default())?;
/// assert_eq!(base_uri2.to_string(), "http://api.example.com:8080/");
///
/// // From a URI string with path prefix
/// let base_uri3 = BaseUri::from_uri_str("https://auth.example.com/path/prefix/")?;
/// assert_eq!(
///     base_uri3.to_string(),
///     "https://auth.example.com/path/prefix/"
/// );
///
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
///
/// Converting an `base_uri` to a complete URI:
///
/// ```
/// # use templated_uri::{BaseUri, uri::{Scheme, PathAndQuery}};
/// let base_uri = BaseUri::new(Scheme::HTTPS, "api.example.com")?;
///
/// // Combine with a path to create a complete URI
/// let uri = base_uri.build_http_uri("/users/123?active=true")?;
/// assert_eq!(
///     uri.to_string(),
///     "https://api.example.com/users/123?active=true"
/// );
///
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BaseUri {
    /// The origin part of the URI, consisting of scheme and authority.
    origin: Origin,
    /// path prefix for the URI, always starts and ends with a slash
    ///
    /// In a perfect world, this would be `Option<BasePath>` and None would represent no path,
    /// but [`http::Uri`] which we need to interface with
    /// parses absent path (`http://example.com`) into a `/` path - which would lead into confusing
    /// behavior and inconsistency, so we are explicit about it.
    path: BasePath,
}

impl BaseUri {
    /// Creates a new [`BaseUri`] with the specified scheme and authority.
    ///
    /// This is a fallible constructor that attempts to convert the inputs to the
    /// required types. You can provide pre-constructed `Scheme` and `Authority` objects
    /// to avoid unnecessary conversions.
    ///
    /// # Arguments
    ///
    /// * `scheme`: The URI scheme (must be either HTTP or HTTPS).
    /// * `authority`: The authority component (hostname and optional port).
    ///
    /// # Errors
    ///
    /// Returns a validation error if:
    ///
    /// - The scheme is not HTTP or HTTPS.
    /// - The scheme or authority conversion fails.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, uri::{Scheme, Authority}};
    /// // Default HTTPS port (443)
    /// let base_uri = BaseUri::new(Scheme::HTTPS, "example.com")?;
    /// assert_eq!(base_uri.to_string(), "https://example.com/");
    ///
    /// // Default HTTPS port (443)
    /// let base_uri = BaseUri::new(Scheme::HTTPS, "www.example.com")?;
    /// assert_eq!(base_uri.to_string(), "https://www.example.com/");
    ///
    /// // Default HTTP port (80)
    /// let base_uri = BaseUri::new(Scheme::HTTP, "example.com")?;
    /// assert_eq!(base_uri.to_string(), "http://example.com/");
    ///
    /// // Custom port
    /// let base_uri = BaseUri::new(Scheme::HTTPS, "example.com:1234")?;
    /// assert_eq!(base_uri.to_string(), "https://example.com:1234/");
    ///
    /// // Invalid or unsupported scheme
    /// let error = BaseUri::new("invalid", "example.com:1234").unwrap_err();
    /// assert!(
    ///     error
    ///         .to_string()
    ///         .starts_with("unsupported scheme: invalid, only HTTP and HTTPS schemes are supported")
    /// );
    ///
    /// // Invalid authority
    /// let error = BaseUri::new(Scheme::HTTPS, "exa/mple.com").unwrap_err();
    /// assert!(error.to_string().starts_with("invalid uri character"));
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(
        scheme: impl TryInto<Scheme, Error: Into<http::Error>>,
        authority: impl TryInto<Authority, Error: Into<http::Error>>,
    ) -> Result<Self, ValidationError> {
        let origin = Origin::new(scheme, authority)?;

        Ok(Self {
            origin,
            path: BasePath::default(),
        })
    }

    /// Sets the path component of this [`BaseUri`].
    ///
    /// The path must start and end with a slash (`/`).
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, uri::Scheme, BasePath};
    /// let base_uri =
    ///     BaseUri::new(Scheme::HTTPS, "example.com")?.with_path(BasePath::try_from("/api/v1/")?)?;
    ///
    /// assert_eq!(base_uri.to_string(), "https://example.com/api/v1/");
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns a [`ValidationError`] if the path cannot be converted to a valid [`BasePath`].
    pub fn with_path<P>(mut self, path: P) -> Result<Self, ValidationError>
    where
        P: TryInto<BasePath>,
        ValidationError: From<<P as TryInto<BasePath>>::Error>,
    {
        self.path = path.try_into()?;
        Ok(self)
    }

    /// Creates a [`BaseUri`] from a host, port, scheme and path.
    ///
    /// This is a convenience method that constructs the `base_uri` from individual components.
    ///
    /// # Arguments
    ///
    /// - `scheme`: The URI scheme (must be either HTTP or HTTPS).
    /// - `host`: The hostname.
    /// - `port`: The port number.
    /// - `path`: The path component. Must start and end with a slash (`/`)
    ///
    /// # Errors
    ///
    /// Returns a validation error if:
    ///
    /// - The scheme is not HTTP or HTTPS.
    /// - The provided host is invalid.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, uri::Scheme, BasePath};
    /// let base_uri = BaseUri::from_parts(Scheme::HTTPS, "example.com", 1234, BasePath::default())?;
    /// assert_eq!(base_uri.to_string(), "https://example.com:1234/");
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    pub fn from_parts(scheme: Scheme, host: impl AsRef<str>, port: u16, path: BasePath) -> Result<Self, ValidationError> {
        Self::new(scheme, format!("{}:{}", host.as_ref(), port))?.with_path(path)
    }

    /// Creates an `base_uri` from a static URI string.
    ///
    /// This method parses a static string as a URI and creates an `base_uri` from it.
    /// The URI must contain both a scheme (HTTP or HTTPS) and an authority component.
    ///
    /// Any path, query, or fragment components in the URI will be discarded.
    /// Only the scheme and authority parts are used to construct the `base_uri`.
    ///
    /// # Arguments
    ///
    /// - `uri`: A static string representing a valid URI with HTTP or HTTPS scheme and authority.
    ///
    /// # Panics
    ///
    /// Panics if:
    ///
    /// - The provided string is not a valid `base_uri` URI.
    /// - The scheme is not HTTP or HTTPS.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, uri::Scheme};
    /// let base_uri = BaseUri::from_uri_static("https://example.com:443");
    /// assert_eq!(base_uri.to_string(), "https://example.com/");
    /// ```
    #[must_use]
    pub fn from_uri_static(uri: &'static str) -> Self {
        Self::from_http_uri(&http::Uri::from_static(uri)).expect("static str is not a valid base_uri URI")
    }

    /// Creates an `base_uri` from a URI string.
    ///
    /// This method parses a string as a URI and extracts the scheme and authority
    /// components to create an `base_uri`. The URI must contain both components, and
    /// the scheme must be either HTTP or HTTPS.
    ///
    /// Any path, query, or fragment components in the URI will be discarded.
    /// Only the scheme and authority parts are used to construct the `base_uri`.
    ///
    /// # Arguments
    ///
    /// - `uri`: A string representing a valid URI with HTTP or HTTPS scheme and authority.
    ///
    /// # Errors
    ///
    /// Returns a validation error if:
    ///
    /// - The URI string cannot be parsed as a valid URI.
    /// - The URI does not contain both a scheme and an authority component.
    /// - The scheme is not HTTP or HTTPS.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::BaseUri;
    /// let base_uri = BaseUri::from_uri_str("https://example.com:443")?;
    /// assert_eq!(base_uri.to_string(), "https://example.com/");
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// Using a URI string with a path:
    ///
    /// ```
    /// # use templated_uri::BaseUri;
    /// let base_uri = BaseUri::from_uri_str("https://example.com:443/path-prefix/")?;
    /// assert_eq!(base_uri.to_string(), "https://example.com/path-prefix/");
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    pub fn from_uri_str(uri: &str) -> Result<Self, ValidationError> {
        uri.parse::<http::Uri>()
            .map_err(|e| ValidationError::caused_by(e.to_string()))
            .and_then(|uri| Self::from_http_uri(&uri))
    }

    /// Creates an `base_uri` from an existing `Uri`.
    ///
    /// Extracts the scheme and authority components from a `Uri` object to create
    /// an `base_uri`. The URI must contain both components.
    ///
    /// Any path, query, or fragment components in the URI will be discarded.
    /// Only the scheme and authority parts are used to construct the `base_uri`.
    ///
    /// # Arguments
    ///
    /// - `uri`: A reference to a `Uri` object.
    ///
    /// # Errors
    ///
    /// Returns a validation error if the URI is missing either the scheme or authority component.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::BaseUri;
    /// let uri = "https://example.com".parse::<http::Uri>()?;
    /// let base_uri = BaseUri::from_http_uri(&uri)?;
    /// // Note the added trailing slash, this is a behavior of http::Uri parsing which initializes a `/`
    /// // path if none is present in input string, we can't do anything about it.
    /// assert_eq!(base_uri.to_string(), "https://example.com/");
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// Using a URI string with a path:
    ///
    /// ```
    /// # use templated_uri::BaseUri;
    /// let uri = "https://example.com/path/".parse::<http::Uri>()?;
    /// let base_uri = BaseUri::from_http_uri(&uri)?;
    /// assert_eq!(base_uri.to_string(), "https://example.com/path/");
    /// assert_eq!(base_uri.path().as_str(), "/path/");
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    pub fn from_http_uri(uri: &http::Uri) -> Result<Self, ValidationError> {
        let (Some(scheme), Some(authority)) = (uri.scheme(), uri.authority()) else {
            return Err(ValidationError::caused_by("URI must have both scheme and authority components"));
        };

        let path = uri.path_and_query().map_or(Ok(BasePath::default()), BasePath::try_from)?;

        Self::new(scheme.clone(), authority.clone())?.with_path(path)
    }

    /// Returns a reference to the scheme component of this `base_uri`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, uri::Scheme};
    /// let base_uri = BaseUri::from_uri_static("https://example.com");
    /// assert_eq!(base_uri.scheme().as_str(), "https");
    /// ```
    pub const fn scheme(&self) -> &Scheme {
        self.origin.scheme()
    }

    /// Returns a reference to the authority component of this `base_uri`.
    ///
    /// The authority typically consists of a hostname and optional port.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, uri::Scheme};
    /// let base_uri = BaseUri::from_uri_static("https://example.com");
    /// assert_eq!(base_uri.authority().as_str(), "example.com");
    /// ```
    ///
    /// ```
    /// # use templated_uri::{BaseUri, uri::Scheme};
    /// let base_uri = BaseUri::from_uri_static("https://example.com:1234");
    /// assert_eq!(base_uri.authority().as_str(), "example.com:1234");
    /// ```
    pub const fn authority(&self) -> &Authority {
        self.origin.authority()
    }

    /// Returns the host part of this `base_uri`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, uri::Scheme};
    /// let base_uri = BaseUri::from_uri_static("https://example.com:443");
    /// assert_eq!(base_uri.host(), "example.com");
    /// ```
    pub fn host(&self) -> &str {
        self.origin.authority().host()
    }

    /// Returns the origin of this `base_uri` in the form `scheme://authority`.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// Consumes `BaseUri` and returns a new instance with different origin
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, Origin, uri::{Scheme, Authority}};
    /// let base_uri = BaseUri::from_uri_static("https://example.com:443");
    /// let new_base_uri = base_uri.with_origin(Origin::new(Scheme::HTTPS, Authority::from_static("new-example.com:8080")).unwrap());
    /// assert_eq!(new_base_uri.to_string(), "https://new-example.com:8080/");
    #[must_use]
    pub fn with_origin(self, origin: Origin) -> Self {
        Self { origin, path: self.path }
    }

    /// Returns the port of this `base_uri`.
    ///
    /// This method determines the port based on the following rules:
    /// 1. If the authority explicitly specifies a port, that port is returned.
    /// 2. If no port is specified, a default port is returned based on the scheme:
    ///    - `80` for `http` scheme.
    ///    - `443` for `https` scheme.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, uri::Scheme};
    /// // Explicit port
    /// let base_uri = BaseUri::from_uri_static("https://example.com:8443");
    /// assert_eq!(base_uri.port(), 8443);
    ///
    /// // Default HTTPS port
    /// let base_uri = BaseUri::from_uri_static("https://example.com");
    /// assert_eq!(base_uri.port(), 443);
    ///
    /// // Default HTTP port
    /// let base_uri = BaseUri::from_uri_static("http://example.com");
    /// assert_eq!(base_uri.port(), 80);
    /// ```
    pub fn port(&self) -> u16 {
        self.origin.port()
    }

    /// Consume this `BaseUri` instance and return a new one with the specified port.
    ///
    /// # Examples
    /// ```
    /// # use templated_uri::{BaseUri, uri::Scheme};
    /// let mut base_uri = BaseUri::from_uri_static("https://example.com");
    /// assert_eq!(base_uri.port(), 443);
    ///
    /// let base_uri = base_uri.with_port(8443);
    /// assert_eq!(base_uri.port(), 8443);
    /// assert_eq!(base_uri.to_string(), "https://example.com:8443/");
    /// ```
    #[must_use]
    pub fn with_port(self, port: u16) -> Self {
        Self {
            origin: self.origin.with_port(port),
            path: self.path,
        }
    }

    /// Returns a reference to the path component of this `base_uri`.
    ///
    /// The path is guaranteed to start and end with a slash (`/`).
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, uri::Scheme};
    /// let base_uri = BaseUri::from_uri_static("https://example.com/some/path/");
    ///
    /// assert_eq!(base_uri.path().as_str(), "/some/path/");
    /// ```
    ///
    /// ```
    /// # use templated_uri::{BaseUri, uri::Scheme};
    /// let base_uri = BaseUri::from_uri_static("https://example.com");
    ///
    /// assert_eq!(base_uri.path().as_str(), "/");
    /// ```
    pub const fn path(&self) -> &BasePath {
        &self.path
    }

    /// Checks if this `base_uri` uses the HTTPS scheme.
    ///
    /// Returns `true` if the scheme is HTTPS, `false` otherwise.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, uri::Scheme};
    /// let secure = BaseUri::from_uri_static("https://example.com");
    /// assert!(secure.is_https());
    ///
    /// let insecure = BaseUri::from_uri_static("http://example.com");
    /// assert!(!insecure.is_https());
    /// ```
    pub fn is_https(&self) -> bool {
        self.origin.is_https()
    }

    /// Constructs a complete URI by combining this `base_uri` with the given path.
    ///
    /// This method combines the [`BaseUri`] with the provided path to create a complete URI.
    /// The resulting URI will have the scheme, authority and path from this `base_uri`, and the path
    /// from the argument.
    ///
    /// # Arguments
    ///
    /// - `path`: A path or path with query parameters that will be converted to a `PathAndQuery`.
    ///
    /// # Errors
    ///
    /// Returns a validation error if the provided path cannot be converted into a `PathAndQuery`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, uri::{Scheme, PathAndQuery}};
    /// let base_uri = BaseUri::from_uri_static("https://example.com");
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
    /// # use templated_uri::{BaseUri, uri::{Scheme, PathAndQuery}};
    /// let base_uri = BaseUri::from_uri_static("https://example.com/api/");
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
    /// # use templated_uri::{BaseUri, uri::{Scheme, PathAndQuery}};
    /// let base_uri = BaseUri::from_uri_static("https://example.com");
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
    pub fn build_http_uri(&self, path: impl TryInto<PathAndQuery, Error: Into<http::Error>>) -> Result<http::Uri, ValidationError> {
        let full_path = self.path.join(path)?;

        let mut parts = Parts::default();
        parts.scheme = Some(self.scheme().clone());
        parts.authority = Some(self.authority().clone());
        parts.path_and_query = Some(full_path);

        http::Uri::from_parts(parts).map_err(ValidationError::caused_by)
    }
}

impl TryFrom<http::Uri> for BaseUri {
    type Error = ValidationError;

    /// Tries to convert a URI into an `base_uri`.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// - The URI does not have both scheme and authority components.
    /// - The scheme is not HTTP or HTTPS.
    fn try_from(uri: http::Uri) -> Result<Self, Self::Error> {
        Self::from_http_uri(&uri)
    }
}

impl From<Origin> for BaseUri {
    /// Converts an `Origin` into a `BaseUri` with a root path ("/").
    ///
    /// This conversion adds a minimal path component to ensure the resulting URI is valid.
    fn from(origin: Origin) -> Self {
        Self {
            origin,
            path: BasePath::default(),
        }
    }
}

impl FromStr for BaseUri {
    type Err = ValidationError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_uri_str(s)
    }
}

impl From<BaseUri> for http::Uri {
    /// Converts an `base_uri` into a `Uri` with a root path ("/").
    ///
    /// This conversion adds a minimal path component to ensure the resulting URI is valid.
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
    /// Formats the `base_uri` as a string in the form `scheme://authority`.
    ///
    /// Default ports (80 for HTTP, 443 for HTTPS) are omitted from the display string.
    /// Custom ports are included when present.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::{BaseUri, uri::Scheme, BasePath};
    /// let base_uri = BaseUri::from_parts(Scheme::HTTPS, "example.com", 443, BasePath::default())?;
    /// assert_eq!(format!("{}", base_uri), "https://example.com/");
    ///
    /// let custom_port = BaseUri::from_parts(Scheme::HTTPS, "example.com", 8443, BasePath::default())?;
    /// assert_eq!(format!("{}", custom_port), "https://example.com:8443/");
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://", self.scheme())?;

        match (self.scheme().as_str(), self.port()) {
            ("http", 80) | ("https", 443) => write!(f, "{}", self.host())?,
            _ => write!(f, "{}", self.authority())?,
        }
        write!(f, "{}", self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod new {
        use super::*;

        #[test]
        fn valid_base_uri() {
            let base_uri = BaseUri::new(Scheme::HTTPS, "example.com").unwrap();
            assert_eq!(base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(base_uri.authority().as_str(), "example.com");
        }

        #[test]
        fn with_custom_port() {
            let base_uri = BaseUri::new(Scheme::HTTP, "example.com:8080").unwrap();
            assert_eq!(base_uri.scheme(), &Scheme::HTTP);
            assert_eq!(base_uri.authority().as_str(), "example.com:8080");
        }

        #[test]
        fn with_string_scheme() {
            let base_uri = BaseUri::new("https", "example.com").unwrap();
            assert_eq!(base_uri.scheme(), &Scheme::HTTPS);
        }

        #[test]
        fn invalid_scheme() {
            let err = BaseUri::new("ftp", "example.com").unwrap_err();
            assert!(err.to_string().contains("unsupported scheme: ftp"));
        }

        #[test]
        fn invalid_authority() {
            let err = BaseUri::new(Scheme::HTTPS, "exam/ple.com:123").unwrap_err();
            assert!(err.to_string().contains("invalid uri"));
        }
    }

    mod from_parts {
        use super::*;

        #[test]
        fn valid_parts() {
            let base_uri = BaseUri::from_parts(Scheme::HTTPS, "example.com", 443, BasePath::from_str("/example/").unwrap()).unwrap();
            assert_eq!(base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(base_uri.authority().as_str(), "example.com:443");
            assert_eq!(base_uri.to_string(), "https://example.com/example/");
        }

        #[test]
        fn invalid_host() {
            let err = BaseUri::from_parts(Scheme::HTTPS, "exa/mple.com", 443, BasePath::from_str("/example/").unwrap()).unwrap_err();
            assert!(err.to_string().contains("invalid uri"));
        }
    }

    mod from_uri_static {
        use super::*;

        #[test]
        fn valid_uri() {
            let base_uri = BaseUri::from_uri_static("https://example.com");
            assert_eq!(base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(base_uri.authority().as_str(), "example.com");
        }

        #[test]
        fn with_path() {
            let base_uri = BaseUri::from_uri_static("https://example.com/path/to/resource/");
            assert_eq!(base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(base_uri.authority().as_str(), "example.com");
            assert_eq!(base_uri.to_string(), "https://example.com/path/to/resource/");
        }

        #[should_panic(expected = "static str is not a valid base_uri URI")]
        #[test]
        fn invalid_uri() {
            let _base_uri = BaseUri::from_uri_static("not-a-valid-uri");
        }
    }

    mod from_uri_str {
        use ohno::ErrorExt;

        use super::*;

        #[test]
        fn valid_uri() {
            let base_uri = BaseUri::from_uri_str("https://example.com/").unwrap();
            assert_eq!(base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(base_uri.authority().as_str(), "example.com");
        }

        #[test]
        fn with_path() {
            let base_uri = BaseUri::from_uri_str("https://example.com/path/").unwrap();
            assert_eq!(base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(base_uri.authority().as_str(), "example.com");
        }

        #[test]
        fn with_invalid_path() {
            let err = BaseUri::from_uri_str("https://example.com/path").unwrap_err();
            assert_eq!(err.message(), "the path must start and end with a slash");
        }

        #[test]
        fn invalid_uri() {
            let err = BaseUri::from_uri_str("not-a-valid-uri").unwrap_err();
            assert_eq!(err.message(), "URI must have both scheme and authority components");
        }
    }

    mod from_uri {
        use super::*;

        #[test]
        fn valid_uri() {
            let uri = http::Uri::from_static("https://example.com");
            let base_uri = BaseUri::from_http_uri(&uri).unwrap();
            assert_eq!(base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(base_uri.authority().as_str(), "example.com");
        }

        #[test]
        fn with_path() {
            let uri = http::Uri::from_static("https://example.com/path/");
            let base_uri = BaseUri::from_http_uri(&uri).unwrap();
            assert_eq!(base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(base_uri.authority().as_str(), "example.com");
            assert_eq!(base_uri.path().as_str(), "/path/");
        }

        #[test]
        fn missing_components() {
            let uri = http::Uri::from_static("/just-a-path");
            let err = BaseUri::from_http_uri(&uri).unwrap_err();
            assert!(err.to_string().contains("URI must have both scheme and authority"));
        }
    }

    mod accessors {
        use super::*;

        #[test]
        fn scheme() {
            let base_uri = BaseUri::from_uri_static("https://example.com");
            assert_eq!(base_uri.scheme().as_str(), "https");
        }

        #[test]
        fn authority() {
            let base_uri = BaseUri::from_uri_static("https://example.com:8443");
            assert_eq!(base_uri.authority().as_str(), "example.com:8443");
        }

        #[test]
        fn host() {
            let base_uri = BaseUri::from_uri_static("https://example.com:8443");
            assert_eq!(base_uri.host(), "example.com");
        }

        #[test]
        fn port_explicit() {
            let base_uri = BaseUri::from_uri_static("https://example.com:8443");
            assert_eq!(base_uri.port(), 8443);
        }

        #[test]
        fn port_default_https() {
            let base_uri = BaseUri::from_uri_static("https://example.com");
            assert_eq!(base_uri.port(), 443);
        }

        #[test]
        fn port_default_http() {
            let base_uri = BaseUri::from_uri_static("http://example.com");
            assert_eq!(base_uri.port(), 80);
        }
    }

    mod is_https {
        use super::*;

        #[test]
        fn secure() {
            let base_uri = BaseUri::from_uri_static("https://example.com");
            assert!(base_uri.is_https());
        }

        #[test]
        fn insecure() {
            let base_uri = BaseUri::from_uri_static("http://example.com");
            assert!(!base_uri.is_https());
        }
    }

    mod build_uri {
        use super::*;

        #[test]
        fn with_path_string() {
            let base_uri = BaseUri::from_uri_static("https://example.com");
            let uri = base_uri.build_http_uri("/api/resource").unwrap();
            assert_eq!(uri.to_string(), "https://example.com/api/resource");
        }

        #[test]
        fn with_empty_uri() {
            let base_uri = BaseUri::from_uri_static("https://example.com");
            let uri = base_uri.build_http_uri("/").unwrap();
            assert_eq!(uri.to_string(), "https://example.com/");
        }

        #[test]
        fn with_path_and_query_string() {
            let base_uri = BaseUri::from_uri_static("https://example.com");
            let uri = base_uri.build_http_uri("/api/resource?param=value").unwrap();
            assert_eq!(uri.to_string(), "https://example.com/api/resource?param=value");
        }

        #[test]
        fn with_path_and_query_object() {
            let base_uri = BaseUri::from_uri_static("https://example.com");
            let path_and_query = PathAndQuery::from_static("/api/resource?param=value");
            let uri = base_uri.build_http_uri(path_and_query).unwrap();
            assert_eq!(uri.to_string(), "https://example.com/api/resource?param=value");
        }

        #[test]
        fn invalid_path() {
            let base_uri = BaseUri::from_uri_static("https://example.com");
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
            let base_uri = BaseUri::from_uri_static("https://example.com");
            let uri: http::Uri = base_uri.into();
            assert_eq!(uri.to_string(), "https://example.com/");
        }

        #[test]
        fn origin_to_base_uri() {
            let origin = Origin::new(Scheme::HTTPS, "example.com:8443").unwrap();
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
            let base_uri = BaseUri::from_uri_static("http://example.com:80");
            assert_eq!(base_uri.to_string(), "http://example.com/");
        }

        #[test]
        fn https_default_port() {
            let base_uri = BaseUri::from_uri_static("https://example.com:443");
            assert_eq!(base_uri.to_string(), "https://example.com/");
        }

        #[test]
        fn custom_port() {
            let base_uri = BaseUri::from_uri_static("https://example.com:8443");
            assert_eq!(base_uri.to_string(), "https://example.com:8443/");
        }
    }

    mod with_origin {
        use super::*;

        #[test]
        fn replaces_origin() {
            let base_uri = BaseUri::from_uri_static("https://example.com/api/");
            let new_origin = Origin::new(Scheme::HTTPS, "new-example.com:8080").unwrap();

            let new_base_uri = base_uri.with_origin(new_origin.clone());

            assert_eq!(new_base_uri.origin(), &new_origin);
            assert_eq!(new_base_uri.scheme(), &Scheme::HTTPS);
            assert_eq!(new_base_uri.authority().as_str(), "new-example.com:8080");
            assert_eq!(new_base_uri.port(), 8080);
            assert_eq!(new_base_uri.path().as_str(), "/api/");
            assert_eq!(new_base_uri.to_string(), "https://new-example.com:8080/api/");
        }
    }

    mod with_port {
        use super::*;

        #[test]
        fn changes_port() {
            let base_uri = BaseUri::from_uri_static("https://example.com/api/");

            let new_base_uri = base_uri.with_port(8443);

            assert_eq!(new_base_uri.origin().port(), 8443);
            assert_eq!(new_base_uri.port(), 8443);
            assert_eq!(new_base_uri.to_string(), "https://example.com:8443/api/");
        }
    }
}
