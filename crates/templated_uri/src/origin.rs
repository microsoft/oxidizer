// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Display;

use http::uri::{Authority, Scheme};

use crate::UriError;

/// Default TCP port for the `http` scheme as defined by [RFC 9110, section 4.2.1](https://www.rfc-editor.org/rfc/rfc9110.html#section-4.2.1).
pub(crate) const HTTP_DEFAULT_PORT: u16 = 80;
/// Default TCP port for the `https` scheme as defined by [RFC 9110, section 4.2.2](https://www.rfc-editor.org/rfc/rfc9110.html#section-4.2.2).
pub(crate) const HTTPS_DEFAULT_PORT: u16 = 443;

/// Scheme and authority components of a URI.
///
/// Use [`Origin`] when you need to work with the base parts of a URI without
/// the path, query, or fragment components.
///
/// `Origin` accepts any valid URI scheme. The port is only reported when it is
/// explicitly present in the authority; default ports are not inferred from the
/// scheme.
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
    #[must_use]
    pub fn from_parts(scheme: Scheme, authority: Authority) -> Self {
        Self { scheme, authority }
    }

    /// Creates a new `Origin` from values convertible into a [`Scheme`] and [`Authority`].
    ///
    /// For pre-typed [`Scheme`] and [`Authority`] values, prefer the infallible
    /// [`Origin::from_parts`].
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

    /// Returns the explicit port of this origin, if any.
    ///
    /// Returns the explicit port from the authority when present, or `None` when
    /// the authority does not specify a port.
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        self.authority.port_u16()
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
        assert_eq!(origin_implicit_http.port(), None);

        let origin_implicit_https = Origin::from_parts(Scheme::HTTPS, Authority::from_static("example.com"));
        assert_eq!(origin_implicit_https.port(), None);

        let origin_explicit = Origin::from_parts(Scheme::HTTP, Authority::from_static("example.com:8080"));
        assert_eq!(origin_explicit.port(), Some(8080));

        let origin_explicit = Origin::from_parts(Scheme::HTTPS, Authority::from_static("example.com:8443"));
        assert_eq!(origin_explicit.port(), Some(8443));
    }

    #[test]
    fn test_port_other_scheme() {
        // Without an explicit port, no port is reported regardless of the scheme.
        let origin_no_port = Origin::from_parts(Scheme::from_str("ftp").unwrap(), Authority::from_static("example.com"));
        assert_eq!(origin_no_port.port(), None);

        // An explicit port is always reported regardless of the scheme.
        let origin_with_port = Origin::from_parts(Scheme::from_str("ftp").unwrap(), Authority::from_static("example.com:21"));
        assert_eq!(origin_with_port.port(), Some(21));
    }

    #[test]
    fn test_origin_display() {
        // Default ports omitted
        let origin = Origin::from_parts(Scheme::HTTP, Authority::from_static("example.com"));
        assert_eq!(format!("{origin}"), "http://example.com");

        // Explicit HTTP default port is also omitted
        let origin = Origin::from_parts(Scheme::HTTP, Authority::from_static("example.com:80"));
        assert_eq!(format!("{origin}"), "http://example.com");

        let origin = Origin::from_parts(Scheme::HTTPS, Authority::from_static("example.com:443"));
        assert_eq!(format!("{origin}"), "https://example.com");

        // Custom ports included
        let origin = Origin::from_parts(Scheme::HTTPS, Authority::from_static("example.com:8443"));
        assert_eq!(format!("{origin}"), "https://example.com:8443");

        // IPv6 with custom port
        let origin = Origin::from_parts(Scheme::HTTPS, Authority::from_static("[::1]:8443"));
        assert_eq!(format!("{origin}"), "https://[::1]:8443");

        // IPv6 with explicit default HTTPS port: brackets preserved, port stripped.
        let origin = Origin::from_parts(Scheme::HTTPS, Authority::from_static("[2001:db8::1]:443"));
        assert_eq!(format!("{origin}"), "https://[2001:db8::1]");

        // IPv6 with explicit default HTTP port: brackets preserved, port stripped.
        let origin = Origin::from_parts(Scheme::HTTP, Authority::from_static("[::1]:80"));
        assert_eq!(format!("{origin}"), "http://[::1]");

        // IPv6 without an explicit port.
        let origin = Origin::from_parts(Scheme::HTTPS, Authority::from_static("[::1]"));
        assert_eq!(format!("{origin}"), "https://[::1]");

        // Other schemes round-trip the authority verbatim.
        let origin = Origin::from_parts(Scheme::from_str("ftp").unwrap(), Authority::from_static("example.com:21"));
        assert_eq!(format!("{origin}"), "ftp://example.com:21");
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
    fn test_with_port_ipv6() {
        // Replacing the port on an IPv6 origin must preserve the brackets.
        let origin = Origin::from_parts(Scheme::HTTPS, Authority::from_static("[2001:db8::1]:8080"));
        let with_port = origin.with_port(9090);
        assert_eq!(with_port.authority().as_str(), "[2001:db8::1]:9090");
        assert_eq!(with_port.port(), Some(9090));
        assert_eq!(format!("{with_port}"), "https://[2001:db8::1]:9090");

        // Adding a port to a bracketed IPv6 authority that had none.
        let origin = Origin::from_parts(Scheme::HTTPS, Authority::from_static("[::1]"));
        let with_port = origin.with_port(8443);
        assert_eq!(with_port.authority().as_str(), "[::1]:8443");
        assert_eq!(format!("{with_port}"), "https://[::1]:8443");
    }

    #[test]
    fn try_from_parts_valid() {
        // Exercises both `TryInto<Scheme>` and `TryInto<Authority>` via `&str`.
        let origin = Origin::try_from_parts("https", "example.com:8443").unwrap();
        assert_eq!(origin.scheme().as_str(), "https");
        assert_eq!(origin.authority().as_str(), "example.com:8443");

        // Pre-typed values also work.
        let origin = Origin::try_from_parts(Scheme::HTTP, Authority::from_static("example.com")).unwrap();
        assert_eq!(origin.scheme(), &Scheme::HTTP);
        assert_eq!(origin.authority().as_str(), "example.com");
    }

    #[test]
    fn try_from_parts_invalid_scheme() {
        Origin::try_from_parts("not a scheme", "example.com").unwrap_err();
    }

    #[test]
    fn try_from_parts_invalid_authority() {
        Origin::try_from_parts("https", "not a host").unwrap_err();
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
    fn display_non_http_scheme_with_http_default_port_keeps_port() {
        // Catches mutation that replaces the `s == Scheme::HTTP.as_str()` match
        // guard with `true`: a non-HTTP scheme using port 80 must still display
        // the explicit port rather than being treated as HTTP.
        let origin = Origin::from_parts(Scheme::from_str("ftp").unwrap(), Authority::from_static("example.com:80"));
        assert_eq!(format!("{origin}"), "ftp://example.com:80");

        // Same idea, but ensures the HTTPS scheme with port 80 still keeps the
        // port (the first arm must not match an https scheme just because the
        // port happens to be 80).
        let origin = Origin::from_parts(Scheme::HTTPS, Authority::from_static("example.com:80"));
        assert_eq!(format!("{origin}"), "https://example.com:80");
    }

    #[test]
    fn display_non_http_scheme_with_https_default_port_keeps_port() {
        // Catches mutation that replaces the `s == Scheme::HTTPS.as_str()` match
        // guard with `true`: a non-HTTPS scheme using port 443 must still display
        // the explicit port.
        let origin = Origin::from_parts(Scheme::from_str("ftp").unwrap(), Authority::from_static("example.com:443"));
        assert_eq!(format!("{origin}"), "ftp://example.com:443");

        // HTTP scheme on port 443 must keep the port too.
        let origin = Origin::from_parts(Scheme::HTTP, Authority::from_static("example.com:443"));
        assert_eq!(format!("{origin}"), "http://example.com:443");
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
