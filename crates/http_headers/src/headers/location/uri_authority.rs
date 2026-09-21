// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Validated RFC 3986 authority components.

use std::fmt;
use std::net::Ipv6Addr;

use super::component::Component;
use super::invalid;
use crate::DecodeError;

/// Validated, encoded authority components of a URI-reference.
///
/// A host may be empty, a registered name, an IPv4 address, or a bracketed
/// IPv6/`IPvFuture` literal. Ports preserve their decimal spelling, including an
/// empty string, leading zeroes, or a value larger than `u16::MAX`. No DNS,
/// scheme-specific, or safe-redirect policy is implied.
///
/// Equality and hashing compare the encoded components exactly, including
/// the distinction between absent and empty userinfo or port. They do not
/// fold case, decode escapes, or apply scheme-specific equivalence rules.
///
/// # Examples
///
/// ```
/// use http_headers::headers::UriAuthority;
///
/// let authority = UriAuthority::new(Some("user%40name"), "[::1]", Some("00443"))?;
/// assert_eq!(authority.userinfo(), Some("user%40name"));
/// assert_eq!(authority.host(), "[::1]");
/// assert_eq!(authority.port(), Some("00443"));
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct UriAuthority<'a> {
    pub(super) userinfo: Option<&'a str>,
    pub(super) host: &'a str,
    pub(super) port: Option<&'a str>,
}

impl<'a> UriAuthority<'a> {
    /// Validates encoded authority components without allocating.
    ///
    /// Pass brackets as part of an IPv6 or `IPvFuture` host. Components are
    /// already encoded: a literal `@` in userinfo must be supplied as `%40`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::DecodeErrorKind::InvalidSyntax`] for invalid userinfo,
    /// host, or port grammar, including invalid percent escapes.
    pub fn new(userinfo: Option<&'a str>, host: &'a str, port: Option<&'a str>) -> Result<Self, DecodeError> {
        if let Some(userinfo) = userinfo {
            Component::Userinfo.validate(userinfo)?;
        }
        validate_host(host)?;
        if port.is_some_and(|port| !port.bytes().all(|byte| byte.is_ascii_digit())) {
            return Err(invalid());
        }
        Ok(Self { userinfo, host, port })
    }

    /// Returns encoded userinfo, distinguishing absent from present-but-empty.
    #[inline]
    #[must_use]
    pub const fn userinfo(self) -> Option<&'a str> {
        self.userinfo
    }

    /// Returns the encoded host, retaining brackets around IP literals.
    #[inline]
    #[must_use]
    pub const fn host(self) -> &'a str {
        self.host
    }

    /// Returns the decimal port spelling, distinguishing absent from empty.
    ///
    /// The generic URI grammar does not restrict ports to `u16` values.
    #[inline]
    #[must_use]
    pub const fn port(self) -> Option<&'a str> {
        self.port
    }
}

impl fmt::Debug for UriAuthority<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UriAuthority").field("redacted", &true).finish_non_exhaustive()
    }
}

fn validate_host(host: &str) -> Result<(), DecodeError> {
    if let Some(literal) = host.strip_prefix('[').and_then(|host| host.strip_suffix(']')) {
        if literal.starts_with(['v', 'V']) {
            let (version, address) = literal[1..].split_once('.').ok_or_else(invalid)?;
            if version.is_empty() || !version.bytes().all(|byte| byte.is_ascii_hexdigit()) || address.is_empty() {
                return Err(invalid());
            }
            Component::IpvFuture.validate(address)
        } else {
            literal.parse::<Ipv6Addr>().map(|_address| ()).map_err(|_invalid| invalid())
        }
    } else {
        Component::RegisteredName.validate(host)
    }
}
