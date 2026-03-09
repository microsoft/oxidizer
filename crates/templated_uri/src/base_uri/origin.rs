// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Display;

use http::uri::{Authority, Scheme};

use crate::ValidationError;

/// Represents the origin of a URI, consisting of the scheme and authority components.
///
/// This struct is useful for scenarios where you need to work with the base parts of a URI
/// without the path, query, or fragment components.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Origin {
    /// The URI scheme (e.g., HTTP or HTTPS).
    scheme: Scheme,
    /// The authority component (hostname and optional port).
    authority: Authority,
}

impl Origin {
    /// Creates a new `Origin` from the given scheme and authority.
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
    pub fn new(
        scheme: impl TryInto<Scheme, Error: Into<http::Error>>,
        authority: impl TryInto<Authority, Error: Into<http::Error>>,
    ) -> Result<Self, ValidationError> {
        let scheme: Scheme = scheme.try_into().map_err(Into::into)?;

        // Validate that the scheme is either HTTP or HTTPS
        if scheme != Scheme::HTTP && scheme != Scheme::HTTPS {
            return Err(ValidationError::caused_by(format!(
                "unsupported scheme: {scheme}, only HTTP and HTTPS schemes are supported",
            )));
        }

        Ok(Self {
            scheme,
            authority: authority.try_into().map_err(Into::into)?,
        })
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
    /// This method determines the port based on the same rules as [`BaseUri::port`](super::BaseUri::port).
    pub fn port(&self) -> u16 {
        if let Some(port) = self.authority.port_u16() {
            return port;
        }

        if self.scheme == Scheme::HTTP {
            return 80;
        }

        if self.scheme == Scheme::HTTPS {
            return 443;
        }

        unreachable!("the scheme is always either http or https")
    }

    /// Set port for this `Origin` instance.
    ///
    /// # Examples
    ///
    /// ```
    /// # use templated_uri::Origin;
    /// let origin = Origin::new("https", "example.com").unwrap();
    /// let origin_with_port = origin.with_port(8443);
    /// assert_eq!(origin_with_port.port(), 8443);
    /// assert_eq!(format!("{}", origin_with_port), "https://example.com:8443");
    /// ```
    #[expect(
        clippy::expect_used,
        reason = "the host is always valid, and we are even stricter about valid port than http crate, so this should never fail"
    )]
    #[must_use]
    #[expect(clippy::missing_panics_doc, reason = "impossible panic")]
    pub fn with_port(self, port: u16) -> Self {
        let host = self.authority.host();
        Self::new(self.scheme, format!("{host}:{port}")).expect("Scheme and host are already valid and port is a valid u16")
    }

    /// Checks if this origin uses the HTTPS scheme.
    ///
    /// This method returns `true` if the scheme is HTTPS, `false` otherwise.
    /// For more details, see [`BaseUri::is_https`](super::BaseUri::is_https).
    pub fn is_https(&self) -> bool {
        self.scheme == Scheme::HTTPS
    }
}

impl std::str::FromStr for Origin {
    type Err = ValidationError;

    /// Parses an `Origin` from a string in the form `scheme://authority`.
    ///
    /// # Errors
    ///
    /// Returns a [`ValidationError`] if the string is not a valid origin
    /// (missing scheme, unsupported scheme, or invalid authority).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uri: http::Uri = s.parse().map_err(ValidationError::caused_by)?;
        let scheme = uri.scheme().ok_or_else(|| ValidationError::caused_by("missing scheme"))?.clone();
        let authority = uri
            .authority()
            .ok_or_else(|| ValidationError::caused_by("missing authority"))?
            .clone();
        Self::new(scheme, authority)
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
    /// # use templated_uri::{Origin, uri::Scheme};
    /// let origin = Origin::new(Scheme::HTTPS, "example.com:443").unwrap();
    /// assert_eq!(format!("{}", origin), "https://example.com");
    ///
    /// let custom_port = Origin::new(Scheme::HTTPS, "example.com:8443").unwrap();
    /// assert_eq!(format!("{}", custom_port), "https://example.com:8443");
    /// ```
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://", self.scheme)?;

        match (self.scheme.as_str(), self.port()) {
            ("http", 80) | ("https", 443) => write!(f, "{}", self.authority.host()),
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
        let origin_implicit_http = Origin::new("http", "example.com").unwrap();
        assert_eq!(origin_implicit_http.port(), 80);

        let origin_implicit_https = Origin::new("https", "example.com").unwrap();
        assert_eq!(origin_implicit_https.port(), 443);

        let origin_explicit = Origin::new("http", "example.com:8080").unwrap();
        assert_eq!(origin_explicit.port(), 8080);

        let origin_explicit = Origin::new("https", "example.com:8443").unwrap();
        assert_eq!(origin_explicit.port(), 8443);
    }

    #[test]
    fn test_origin_display() {
        // Default ports omitted
        let origin_http = Origin::new("http", "example.com").unwrap();
        assert_eq!(format!("{origin_http}"), "http://example.com");

        let origin_https = Origin::new("https", "example.com:443").unwrap();
        assert_eq!(format!("{origin_https}"), "https://example.com");

        // Custom ports included
        let origin_custom = Origin::new("https", "example.com:8443").unwrap();
        assert_eq!(format!("{origin_custom}"), "https://example.com:8443");

        // IPv6 with custom port
        let origin_ipv6 = Origin::new("https", "[::1]:8443").unwrap();
        assert_eq!(format!("{origin_ipv6}"), "https://[::1]:8443");
    }

    #[test]
    fn test_scheme_accessor() {
        let origin_http = Origin::new("http", "example.com").unwrap();
        assert_eq!(origin_http.scheme().as_str(), "http");

        let origin_https = Origin::new("https", "example.com:8443").unwrap();
        assert_eq!(origin_https.scheme().as_str(), "https");
    }

    #[test]
    fn test_authority_accessor() {
        let origin = Origin::new("https", "example.com:8443").unwrap();
        assert_eq!(origin.authority().as_str(), "example.com:8443");

        let origin_no_port = Origin::new("http", "example.com").unwrap();
        assert_eq!(origin_no_port.authority().as_str(), "example.com");

        let origin_ipv6 = Origin::new("https", "[::1]:8080").unwrap();
        assert_eq!(origin_ipv6.authority().as_str(), "[::1]:8080");
    }

    #[test]
    fn test_into_parts() {
        let origin = Origin::new("https", "example.com:8443").unwrap();
        let (scheme, authority) = origin.into_parts();

        assert_eq!(scheme.as_str(), "https");
        assert_eq!(authority.as_str(), "example.com:8443");
    }

    #[test]
    fn test_with_port() {
        let origin = Origin::new("https", "example.com").unwrap();
        let with_port = origin.with_port(8443);
        assert_eq!(with_port.port(), 8443);
        assert_eq!(format!("{with_port}"), "https://example.com:8443");
    }

    #[test]
    #[should_panic(expected = "entered unreachable code: the scheme is always either http or https")]
    fn test_with_impossible_scheme() {
        let origin = Origin {
            scheme: Scheme::from_str("ftp").unwrap(),
            authority: Authority::from_static("example.com"),
        };
        origin.port();
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
    fn from_str_unsupported_scheme() {
        "ftp://example.com".parse::<Origin>().unwrap_err();
    }

    #[cfg(feature = "serde")]
    mod serde_tests {
        use super::*;

        #[test]
        fn origin_roundtrip() {
            let original = Origin::new(Scheme::HTTPS, "example.com:8443").unwrap();
            let json = serde_json::to_string(&original).unwrap();
            assert_eq!(json, r#""https://example.com:8443""#);
            let deserialized: Origin = serde_json::from_str(&json).unwrap();
            assert_eq!(original, deserialized);
        }
    }
}
