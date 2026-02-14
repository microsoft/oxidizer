// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http::uri::{Authority, Scheme};
use std::fmt::Display;

use crate::ValidationError;

/// Represents the origin of a URI, consisting of the scheme and authority components.
///
/// This struct is useful for scenarios where you need to work with the base parts of a URI
/// without the path, query, or fragment components.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Origin {
    pub scheme: Scheme,
    pub authority: Authority,
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

    /// Returns the port of this origin.
    ///
    /// This method determines the port based on the same rules as [`BaseUri::port`](super::BaseUri::port).
    pub fn port(&self) -> u16 {
        if let Some(port) = self.authority.port_u16() {
            return port;
        }

        match self.scheme.as_str() {
            "http" => 80,
            "https" => 443,
            _ => unreachable!("the scheme is always either http or https"),
        }
    }

    /// Set port for this `Origin` instance.
    #[expect(
        clippy::expect_used,
        reason = "the host is always valid, and we are even stricter about valid port than http crate, so this should never fail"
    )]
    #[must_use]
    #[expect(clippy::missing_panics_doc, reason = "impossible panic")]
    pub fn with_port(self, port: u16) -> Self {
        let host = self.authority.host();
        Self::new(self.scheme, format!("{host}:{port}"))
            .expect("Scheme ahd host are already valid and port is a valid u16")
    }

    /// Checks if this origin uses the HTTPS scheme.
    ///
    /// This method returns `true` if the scheme is HTTPS, `false` otherwise.
    /// For more details, see [`BaseUri::is_https`](super::BaseUri::is_https).
    pub fn is_https(&self) -> bool {
        self.scheme == Scheme::HTTPS
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
    /// # use obscuri::{Origin, uri::Scheme};
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
}
