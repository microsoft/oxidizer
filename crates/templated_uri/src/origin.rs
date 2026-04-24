// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Display;

use http::uri::{Authority, Scheme};

use crate::UriError;

/// Default TCP port for the `http` scheme as defined by [RFC 9110, section 4.2.1](https://www.rfc-editor.org/rfc/rfc9110.html#section-4.2.1).
pub(crate) const HTTP_DEFAULT_PORT: u16 = 80;
/// Default TCP port for the `https` scheme as defined by [RFC 9110, section 4.2.2](https://www.rfc-editor.org/rfc/rfc9110.html#section-4.2.2).
pub(crate) const HTTPS_DEFAULT_PORT: u16 = 443;

/// Represents the origin of a URI, consisting of the scheme and authority components.
///
/// This struct is useful for scenarios where you need to work with the base parts of a URI
/// without the path, query, or fragment components.
///
/// `Origin` accepts any valid URI scheme. For HTTP and HTTPS the well-known default
/// ports are inferred from the scheme when the authority does not specify one
/// explicitly; for other schemes the port is reported as `None` unless explicitly
/// provided in the authority.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Origin {
    /// The URI scheme (e.g., HTTP or HTTPS).
    scheme: Scheme,
    /// The authority component (hostname and optional port).
    authority: Authority,
}

impl Origin {
    /// Creates a new `Origin` by parsing a static string.
    ///
    /// # Panics
    ///
    /// Panics if the string is not a valid origin (missing scheme or invalid authority).
    /// Intended for use with compile-time-known constants;
    /// use [`Origin::from_str`](std::str::FromStr::from_str) for fallible parsing.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::Origin;
    /// let origin = Origin::from_static("https://example.com:8443");
    /// assert_eq!(origin.scheme().as_str(), "https");
    /// assert_eq!(origin.authority().as_str(), "example.com:8443");
    /// ```
    #[must_use]
    #[expect(clippy::expect_used, reason = "from_static is documented to panic on invalid input")]
    pub fn from_static(s: &'static str) -> Self {
        s.parse().expect("invalid origin passed to Origin::from_static")
    }

    /// Creates a new `Origin` from the given scheme and authority.
    ///
    /// Both components are already validated by their respective types, so this
    /// constructor is infallible.
    ///
    /// # Arguments
    ///
    /// * `scheme`: The URI scheme.
    /// * `authority`: The authority component (hostname and optional port).
    #[must_use]
    pub fn from_parts(scheme: Scheme, authority: Authority) -> Self {
        Self { scheme, authority }
    }

    /// Creates a new `Origin` from values that can be converted into a scheme
    /// and authority (e.g. string slices).
    ///
    /// For pre-typed [`Scheme`] and [`Authority`] values, prefer the infallible
    /// [`Origin::from_parts`].
    ///
    /// # Arguments
    ///
    /// * `scheme`: A value convertible into a [`Scheme`].
    /// * `authority`: A value convertible into an [`Authority`].
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if either conversion fails.
    pub fn try_from_parts(
        scheme: impl TryInto<Scheme, Error: Into<http::Error>>,
        authority: impl TryInto<Authority, Error: Into<http::Error>>,
    ) -> Result<Self, UriError> {
        let scheme = scheme.try_into().map_err(|e| UriError::from(e.into()))?;
        let authority = authority.try_into().map_err(|e| UriError::from(e.into()))?;
        Ok(Self::from_parts(scheme, authority))
    }

    /// Returns a reference to the scheme.
    #[must_use]
    pub const fn scheme(&self) -> &Scheme {
        &self.scheme
    }

    /// Returns a reference to the authority.
    #[must_use]
    pub const fn authority(&self) -> &Authority {
        &self.authority
    }

    /// Consumes the origin and returns the scheme and authority.
    #[must_use]
    pub fn into_parts(self) -> (Scheme, Authority) {
        (self.scheme, self.authority)
    }

    /// Returns the port of this origin.
    ///
    /// Returns the explicit port from the authority when present. Otherwise, for
    /// the well-known HTTP and HTTPS schemes the default port is inferred from
    /// the scheme (`80` for `http`, `443` for `https`). For all other schemes
    /// without an explicit port this method returns `None`.
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        if let Some(port) = self.authority.port_u16() {
            return Some(port);
        }

        if self.scheme == Scheme::HTTP {
            return Some(HTTP_DEFAULT_PORT);
        }

        if self.scheme == Scheme::HTTPS {
            return Some(HTTPS_DEFAULT_PORT);
        }

        None
    }

    /// Set port for this `Origin` instance.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::Origin;
    /// let origin = Origin::from_static("https://example.com");
    /// let origin_with_port = origin.with_port(8443);
    /// assert_eq!(origin_with_port.port(), Some(8443));
    /// assert_eq!(format!("{}", origin_with_port), "https://example.com:8443");
    /// ```
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "host comes from a valid Authority and u16 always formats as a valid port, so the resulting authority is always parseable"
    )]
    #[expect(clippy::missing_panics_doc, reason = "the documented expect is unreachable")]
    pub fn with_port(self, port: u16) -> Self {
        let host = self.authority.host();
        let authority = format!("{host}:{port}")
            .parse::<Authority>()
            .expect("host originated from a valid Authority and u16 is always a valid port");
        Self::from_parts(self.scheme, authority)
    }

    /// Checks if this origin uses the HTTPS scheme.
    ///
    /// This method returns `true` if the scheme is HTTPS, `false` otherwise.
    /// For more details, see [`BaseUri::is_https`](crate::BaseUri::is_https).
    pub fn is_https(&self) -> bool {
        self.scheme == Scheme::HTTPS
    }
}

impl std::str::FromStr for Origin {
    type Err = UriError;

    /// Parses an `Origin` from a string in the form `scheme://authority`.
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the string is not a valid origin
    /// (missing scheme or invalid authority).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uri: http::Uri = s.parse().map_err(UriError::from)?;
        let scheme = uri.scheme().ok_or_else(|| UriError::invalid_uri("missing scheme"))?.clone();
        let authority = uri.authority().ok_or_else(|| UriError::invalid_uri("missing authority"))?.clone();
        Ok(Self::from_parts(scheme, authority))
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for Origin {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Origin {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl Display for Origin {
    /// Formats the `Origin` as a string in the form `scheme://authority`.
    ///
    /// Default ports (80 for HTTP, 443 for HTTPS) are omitted from the display string.
    /// Custom ports are included when present.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::Origin;
    /// let origin = Origin::from_static("https://example.com:443");
    /// assert_eq!(format!("{}", origin), "https://example.com");
    ///
    /// let custom_port = Origin::from_static("https://example.com:8443");
    /// assert_eq!(format!("{}", custom_port), "https://example.com:8443");
    /// ```
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://", self.scheme)?;

        match (self.scheme.as_str(), self.authority.port_u16()) {
            (s, Some(HTTP_DEFAULT_PORT)) if s == Scheme::HTTP.as_str() => write!(f, "{}", self.authority.host()),
            (s, Some(HTTPS_DEFAULT_PORT)) if s == Scheme::HTTPS.as_str() => write!(f, "{}", self.authority.host()),
            _ => write!(f, "{}", self.authority),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_port() {
        let origin_implicit_http = Origin::from_parts(Scheme::HTTP, Authority::from_static("example.com"));
        assert_eq!(origin_implicit_http.port(), Some(80));

        let origin_implicit_https = Origin::from_parts(Scheme::HTTPS, Authority::from_static("example.com"));
        assert_eq!(origin_implicit_https.port(), Some(443));

        let origin_explicit = Origin::from_parts(Scheme::HTTP, Authority::from_static("example.com:8080"));
        assert_eq!(origin_explicit.port(), Some(8080));

        let origin_explicit = Origin::from_parts(Scheme::HTTPS, Authority::from_static("example.com:8443"));
        assert_eq!(origin_explicit.port(), Some(8443));
    }

    #[test]
    fn test_port_other_scheme() {
        // Non-HTTP schemes without an explicit port have no inferable default.
        let origin_no_port = Origin::from_parts(Scheme::from_str("ftp").unwrap(), Authority::from_static("example.com"));
        assert_eq!(origin_no_port.port(), None);

        // An explicit port is always reported regardless of the scheme.
        let origin_with_port = Origin::from_parts(Scheme::from_str("ftp").unwrap(), Authority::from_static("example.com:21"));
        assert_eq!(origin_with_port.port(), Some(21));
    }

    #[test]
    fn test_origin_display() {
        // Default ports omitted
        let origin_http = Origin::from_parts(Scheme::HTTP, Authority::from_static("example.com"));
        assert_eq!(format!("{origin_http}"), "http://example.com");

        let origin_https = Origin::from_parts(Scheme::HTTPS, Authority::from_static("example.com:443"));
        assert_eq!(format!("{origin_https}"), "https://example.com");

        // Custom ports included
        let origin_custom = Origin::from_parts(Scheme::HTTPS, Authority::from_static("example.com:8443"));
        assert_eq!(format!("{origin_custom}"), "https://example.com:8443");

        // IPv6 with custom port
        let origin_ipv6 = Origin::from_parts(Scheme::HTTPS, Authority::from_static("[::1]:8443"));
        assert_eq!(format!("{origin_ipv6}"), "https://[::1]:8443");

        // Other schemes round-trip the authority verbatim.
        let origin_ftp = Origin::from_parts(Scheme::from_str("ftp").unwrap(), Authority::from_static("example.com:21"));
        assert_eq!(format!("{origin_ftp}"), "ftp://example.com:21");
    }

    #[test]
    fn test_scheme_accessor() {
        let origin_http = Origin::from_parts(Scheme::HTTP, Authority::from_static("example.com"));
        assert_eq!(origin_http.scheme().as_str(), "http");

        let origin_https = Origin::from_parts(Scheme::HTTPS, Authority::from_static("example.com:8443"));
        assert_eq!(origin_https.scheme().as_str(), "https");
    }

    #[test]
    fn test_authority_accessor() {
        let origin = Origin::from_parts(Scheme::HTTPS, Authority::from_static("example.com:8443"));
        assert_eq!(origin.authority().as_str(), "example.com:8443");

        let origin_no_port = Origin::from_parts(Scheme::HTTP, Authority::from_static("example.com"));
        assert_eq!(origin_no_port.authority().as_str(), "example.com");

        let origin_ipv6 = Origin::from_parts(Scheme::HTTPS, Authority::from_static("[::1]:8080"));
        assert_eq!(origin_ipv6.authority().as_str(), "[::1]:8080");
    }

    #[test]
    fn test_into_parts() {
        let origin = Origin::from_parts(Scheme::HTTPS, Authority::from_static("example.com:8443"));
        let (scheme, authority) = origin.into_parts();

        assert_eq!(scheme.as_str(), "https");
        assert_eq!(authority.as_str(), "example.com:8443");
    }

    #[test]
    fn test_with_port() {
        let origin = Origin::from_parts(Scheme::HTTPS, Authority::from_static("example.com"));
        let with_port = origin.with_port(8443);
        assert_eq!(with_port.port(), Some(8443));
        assert_eq!(format!("{with_port}"), "https://example.com:8443");
    }

    #[test]
    fn from_str_valid() {
        let origin: Origin = "https://example.com:8443".parse().unwrap();
        assert_eq!(origin.scheme().as_str(), "https");
        assert_eq!(origin.authority().as_str(), "example.com:8443");
    }

    #[test]
    fn from_str_missing_scheme() {
        "example.com".parse::<Origin>().unwrap_err();
    }

    #[test]
    fn from_str_non_http_scheme() {
        // Non-HTTP schemes are accepted; only the scheme/authority shape is required.
        let origin: Origin = "ftp://example.com".parse().unwrap();
        assert_eq!(origin.scheme().as_str(), "ftp");
        assert_eq!(origin.authority().as_str(), "example.com");
        assert_eq!(origin.port(), None);
    }

    #[test]
    fn from_static_valid() {
        let origin = Origin::from_static("https://example.com:8443");
        assert_eq!(origin.scheme().as_str(), "https");
        assert_eq!(origin.authority().as_str(), "example.com:8443");
    }

    #[test]
    fn from_static_non_http_scheme() {
        // Non-HTTP schemes do not panic.
        let origin = Origin::from_static("ftp://example.com:21");
        assert_eq!(origin.scheme().as_str(), "ftp");
        assert_eq!(origin.port(), Some(21));
    }

    #[test]
    #[should_panic(expected = "invalid origin passed to Origin::from_static")]
    fn from_static_invalid() {
        // A plain hostname without a scheme is still rejected.
        let _ = Origin::from_static("example.com");
    }

    #[cfg(feature = "serde")]
    mod serde_tests {
        use super::*;

        #[test]
        fn origin_roundtrip() {
            let original = Origin::from_parts(Scheme::HTTPS, Authority::from_static("example.com:8443"));
            let json = serde_json::to_string(&original).unwrap();
            assert_eq!(json, r#""https://example.com:8443""#);
            let deserialized: Origin = serde_json::from_str(&json).unwrap();
            assert_eq!(original, deserialized);
        }
    }
}
