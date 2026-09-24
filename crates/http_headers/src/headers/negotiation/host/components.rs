// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

/// The validated kind of a URI host.
///
/// Registered names are not necessarily DNS names. Numeric-looking names that
/// are not strict dotted-decimal IPv4 addresses remain registered names.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HostKind<'a> {
    /// A URI registered name, without percent decoding or DNS resolution.
    RegisteredName(RegisteredNameView<'a>),
    /// A strict dotted-decimal IPv4 address.
    Ipv4(Ipv4Addr),
    /// A parsed IPv6 address; brackets remain in the header's textual accessor.
    Ipv6(Ipv6Addr),
    /// A validated `IPvFuture` literal.
    IpvFuture(IpvFutureView<'a>),
}

/// A validated URI registered name and its retained ASCII normalization.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RegisteredNameView<'a> {
    pub(super) original: &'a str,
    pub(super) normalized: &'a str,
}

impl<'a> RegisteredNameView<'a> {
    /// Returns the original name, including any percent escapes.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'a str {
        self.original
    }

    /// Returns the ASCII name produced by relaxed IDNA validation.
    ///
    /// For names accepted by strict parsing this is the unchanged original
    /// name, not a promise of DNS validity, lowercasing, or percent decoding.
    /// Relaxed IDNA mappings are retained exactly as validated; any mapped
    /// delimiters are not reinterpreted as ports in the original wire value.
    #[must_use]
    #[inline]
    pub const fn normalized(self) -> &'a str {
        self.normalized
    }
}

impl fmt::Display for RegisteredNameView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.original)
    }
}

/// Validated version and address components of an `IPvFuture` literal.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct IpvFutureView<'a> {
    pub(super) version: &'a str,
    pub(super) address: &'a str,
}

impl<'a> IpvFutureView<'a> {
    /// Returns the hexadecimal version, without imposing an integer size limit.
    #[must_use]
    #[inline]
    pub const fn version(self) -> &'a str {
        self.version
    }

    /// Returns the address, excluding the version and square brackets.
    #[must_use]
    #[inline]
    pub const fn address(self) -> &'a str {
        self.address
    }
}

impl fmt::Display for IpvFutureView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}.{}", self.version, self.address)
    }
}

/// A validated textual URI port, which may be empty or exceed `u16`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HostPortView<'a> {
    pub(super) text: &'a str,
    pub(super) numeric: Result<u16, PortConversionError>,
}

impl<'a> HostPortView<'a> {
    /// Validates a textual port independently of its network-port range.
    ///
    /// # Errors
    ///
    /// Returns a `Host` decode error for non-decimal text.
    pub fn new(text: &'a str) -> Result<Self, crate::DecodeError> {
        Ok(Self {
            text,
            numeric: super::validate_port(text.as_bytes())?,
        })
    }

    /// Returns the original decimal spelling, including leading zeros.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'a str {
        self.text
    }

    /// Returns the retained checked network-port conversion.
    ///
    /// # Errors
    ///
    /// Returns [`PortConversionErrorKind::Empty`] for an empty port, or
    /// [`PortConversionErrorKind::Overflow`] when its value exceeds `u16`.
    #[inline]
    pub const fn to_u16(self) -> Result<u16, PortConversionError> {
        self.numeric
    }
}

impl fmt::Display for HostPortView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text)
    }
}

/// Why a validated textual URI port is not a network port.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PortConversionErrorKind {
    /// The colon is present but its port text is empty.
    Empty,
    /// The decimal value exceeds `u16::MAX`.
    Overflow,
}

/// A checked conversion failure for a syntactically valid URI port.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PortConversionError {
    pub(super) kind: PortConversionErrorKind,
}

impl PortConversionError {
    /// Returns the reason conversion failed.
    #[must_use]
    #[inline]
    pub const fn kind(self) -> PortConversionErrorKind {
        self.kind
    }
}

impl fmt::Display for PortConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.kind {
            PortConversionErrorKind::Empty => "the URI port is empty",
            PortConversionErrorKind::Overflow => "the URI port exceeds u16::MAX",
        })
    }
}

impl Error for PortConversionError {}
