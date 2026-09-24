// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

/// The semantic value of `Access-Control-Allow-Origin`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AccessControlAllowOriginKind<'a> {
    /// The wildcard token.
    Wildcard,
    /// The opaque-origin serialization `null`, not an opaque-origin identity.
    Null,
    /// A validated serialized tuple origin.
    Origin(SerializedOriginView<'a>),
}

/// A scheme accepted in serialized tuple origins.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OriginScheme {
    /// File Transfer Protocol.
    Ftp,
    /// HTTP.
    Http,
    /// HTTP over TLS.
    Https,
    /// WebSocket.
    Ws,
    /// WebSocket over TLS.
    Wss,
}

impl OriginScheme {
    /// Returns the lowercase scheme without `://`.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ftp => "ftp",
            Self::Http => "http",
            Self::Https => "https",
            Self::Ws => "ws",
            Self::Wss => "wss",
        }
    }

    /// Returns the default network port for this scheme.
    #[must_use]
    #[inline]
    pub const fn default_port(self) -> u16 {
        match self {
            Self::Ftp => 21,
            Self::Http | Self::Ws => 80,
            Self::Https | Self::Wss => 443,
        }
    }

    pub(super) fn parse(scheme: &[u8]) -> Option<Self> {
        match scheme {
            b"ftp" => Some(Self::Ftp),
            b"http" => Some(Self::Http),
            b"https" => Some(Self::Https),
            b"ws" => Some(Self::Ws),
            b"wss" => Some(Self::Wss),
            _ => None,
        }
    }
}

impl fmt::Display for OriginScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A validated origin host, distinct from the broader URI `Host` grammar.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OriginHost<'a> {
    /// A lowercase ASCII domain, possibly with a trailing dot.
    Domain(OriginDomainView<'a>),
    /// A dotted-decimal IPv4 address.
    Ipv4(Ipv4Addr),
    /// An IPv6 address.
    Ipv6(Ipv6Addr),
}

/// A validated, serialized ASCII domain in an origin.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OriginDomainView<'a> {
    pub(super) text: &'a str,
}

impl<'a> OriginDomainView<'a> {
    /// Validates a domain for typed origin construction.
    ///
    /// # Errors
    ///
    /// Rejects uppercase, Unicode, IP addresses, empty labels, and other text
    /// outside the existing serialized-origin domain grammar.
    pub fn new(text: &'a str) -> Result<Self, crate::DecodeError> {
        if super::parse_serialized_host(text.as_bytes()) == Some(super::ParsedOriginHost::Domain) {
            Ok(Self { text })
        } else {
            Err(super::invalid_syntax(&crate::FieldName::AccessControlAllowOrigin))
        }
    }

    /// Returns the validated domain, preserving any trailing dot.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'a str {
        self.text
    }
}

impl fmt::Display for OriginDomainView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text)
    }
}

/// The retained components of a serialized tuple origin.
///
/// Reading these components does not parse or normalize the original value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SerializedOriginView<'a> {
    pub(super) serialized: &'a str,
    pub(super) scheme: OriginScheme,
    pub(super) host: OriginHost<'a>,
    pub(super) port: Option<u16>,
}

impl<'a> SerializedOriginView<'a> {
    /// Returns the complete serialized origin, excluding surrounding whitespace.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'a str {
        self.serialized
    }

    /// Returns the validated scheme.
    #[must_use]
    #[inline]
    pub const fn scheme(self) -> OriginScheme {
        self.scheme
    }

    /// Returns the domain or retained IP address.
    #[must_use]
    #[inline]
    pub const fn host(self) -> OriginHost<'a> {
        self.host
    }

    /// Returns the explicitly serialized port, excluding an omitted default.
    #[must_use]
    #[inline]
    pub const fn port(self) -> Option<u16> {
        self.port
    }

    /// Returns the explicit port or the scheme's default.
    #[must_use]
    #[inline]
    pub const fn effective_port(self) -> u16 {
        match self.port {
            Some(port) => port,
            None => self.scheme.default_port(),
        }
    }
}

impl fmt::Display for SerializedOriginView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.serialized)
    }
}
