// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::fmt::{self, Write as _};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::ops::Range;
use std::str::{self, FromStr as _};

use idna::domain_to_ascii;

use super::shared::{invalid, invalid_syntax};
use crate::{DecodeError, DecodeErrorKind, FieldName, FieldValue, FieldValueRef, SingleValueField, validate};

mod components;
pub use components::{HostKind, HostPortView, IpvFutureView, PortConversionError, PortConversionErrorKind, RegisteredNameView};

/// Defines the `Host` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 7.2](https://www.rfc-editor.org/rfc/rfc9110#section-7.2).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{Host, HostOwned};
///
/// let mut map = HeaderMap::new();
/// Host::insert(&mut map, HostOwned::try_from("example.com:8080")?)?;
/// assert!(Host::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct Host {
    _private: (),
}

/// Owned value for the `Host` header.
///
/// Debug output omits authority components when the field value is sensitive.
///
/// # Specification
///
/// Defined by [RFC 9110 section 7.2].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::HostOwned::try_from("example.com:8080")?;
/// assert_eq!(value.host()?, "example.com");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Host: example.com` names a host, `Host: example.com:8080` includes a port,
/// and `Host: [2001:db8::1]:443` uses an IPv6 literal.
///
/// [RFC 9110 section 7.2]: https://www.rfc-editor.org/rfc/rfc9110#section-7.2
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HostOwned {
    value: FieldValue,
    parsed: ParsedHost,
}

/// A view of the `Host` header with retained validated components.
///
/// Ordinary ASCII values borrow their storage without allocating. Relaxed
/// international names retain an owned IDNA normalization, separately from the
/// borrowed original bytes. [`HostOwned::as_view`] borrows that normalization
/// instead of copying it.
///
/// Debug output omits authority components when the field value is sensitive.
#[derive(Clone, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{Host, HostView};
/// use http_headers::{FieldValueRef, SingleValueField};
///
/// let view: HostView<'_> =
///     <Host as SingleValueField>::decode_view(FieldValueRef::new(b"example.com:443"))?;
/// assert_eq!(view.host(), "example.com");
/// assert_eq!(view.port(), Some("443"));
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct HostView<'a> {
    value: FieldValueRef<'a>,
    host: &'a str,
    port: Option<&'a str>,
    kind: ParsedHostKind,
    normalized: Option<Cow<'a, str>>,
    numeric_port: Option<Result<u16, PortConversionError>>,
}

impl fmt::Debug for HostOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("HostOwned");
        debug.field("value", &self.value);
        if self.value.is_sensitive() {
            debug.finish_non_exhaustive()
        } else {
            debug.field("parsed", &self.parsed).finish()
        }
    }
}

impl fmt::Debug for HostView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("HostView");
        debug.field("value", &self.value);
        if self.value.is_sensitive() {
            debug.finish_non_exhaustive()
        } else {
            debug
                .field("host", &self.host)
                .field("port", &self.port)
                .field("kind", &self.kind)
                .field("normalized", &self.normalized)
                .field("numeric_port", &self.numeric_port)
                .finish()
        }
    }
}

impl HostOwned {
    /// Constructs an authority from validated components without parsing them again.
    ///
    /// Registered names preserve their spelling and retained normalization.
    /// IP addresses use their standard textual representation; `IPvFuture` uses
    /// a lowercase `v` prefix and preserves its version and address spelling.
    ///
    /// # Errors
    ///
    /// Returns an error if adding a port to a relaxed international name would
    /// introduce a second port delimiter in its retained IDNA normalization.
    #[expect(
        clippy::missing_panics_doc,
        reason = "validated components and String formatting cannot violate field-value invariants"
    )]
    pub fn from_parts(host: HostKind<'_>, port: Option<HostPortView<'_>>) -> Result<Self, DecodeError> {
        if let HostKind::RegisteredName(name) = host
            && port.is_some()
            && name.normalized() != name.as_str()
        {
            let normalized = name.normalized();
            if normalized.starts_with('[') {
                if !normalized.ends_with(']') {
                    return Err(invalid(&FieldName::Host, DecodeErrorKind::InvalidNumber));
                }
            } else if normalized.contains(':') {
                return Err(invalid_syntax(&FieldName::Host));
            }
        }
        let host_capacity = match host {
            HostKind::RegisteredName(name) => name.as_str().len(),
            HostKind::Ipv4(_) => 15,
            HostKind::Ipv6(_) => 41,
            HostKind::IpvFuture(address) => address.version().len().saturating_add(address.address().len()).saturating_add(4),
        };
        let port_capacity = port.map_or(0, |port| port.as_str().len().saturating_add(1));
        let mut wire = String::with_capacity(host_capacity.saturating_add(port_capacity));
        let mut normalized = None;
        let kind = match host {
            HostKind::RegisteredName(name) => {
                wire.push_str(name.as_str());
                if name.normalized() != name.as_str() {
                    normalized = Some(name.normalized().to_owned());
                }
                ParsedHostKind::RegisteredName
            }
            HostKind::Ipv4(address) => {
                write!(wire, "{address}").expect("writing an IP address into a String cannot fail");
                ParsedHostKind::Ipv4(address)
            }
            HostKind::Ipv6(address) => {
                write!(wire, "[{address}]").expect("writing an IP address into a String cannot fail");
                ParsedHostKind::Ipv6(address)
            }
            HostKind::IpvFuture(address) => {
                write!(wire, "[{address}]").expect("writing a validated IPvFuture address into a String cannot fail");
                ParsedHostKind::IpvFuture {
                    dot: address.version().len() + 2,
                }
            }
        };
        let host_end = wire.len();
        let port_start = port.map(|port| {
            wire.push(':');
            wire.push_str(port.as_str());
            host_end + 1
        });
        Ok(Self {
            value: FieldValue::try_from(wire).expect("validated host and decimal port components are valid field value bytes"),
            parsed: ParsedHost {
                host_end,
                port_start,
                kind,
                normalized,
                numeric_port: port.map(HostPortView::to_u16),
            },
        })
    }

    /// Constructs an IPv4 authority with an optional checked network port.
    #[must_use]
    pub fn from_ipv4(address: Ipv4Addr, port: Option<u16>) -> Self {
        Self::from_address(HostKind::Ipv4(address), port)
    }

    /// Constructs a bracketed IPv6 authority with an optional checked network port.
    #[must_use]
    pub fn from_ipv6(address: Ipv6Addr, port: Option<u16>) -> Self {
        Self::from_address(HostKind::Ipv6(address), port)
    }

    fn from_address(host: HostKind<'_>, port: Option<u16>) -> Self {
        let mut buffer = itoa::Buffer::new();
        let port = port.map(|port| HostPortView {
            text: buffer.format(port),
            numeric: Ok(port),
        });
        Self::from_parts(host, port).expect("IP address construction cannot introduce an IDNA-normalized port delimiter")
    }

    /// Borrows the retained parsing results without repeating validation or IDNA.
    #[must_use]
    #[inline]
    #[expect(clippy::missing_panics_doc, reason = "the offsets are private and validated at construction")]
    pub fn as_view(&self) -> HostView<'_> {
        HostView {
            value: self.value.as_field_value_ref(),
            host: self.host().expect("host offsets were validated when constructing this value"),
            port: self.port().expect("port offsets were validated when constructing this value"),
            kind: self.parsed.kind,
            normalized: self.parsed.normalized.as_deref().map(Cow::Borrowed),
            numeric_port: self.parsed.numeric_port,
        }
    }

    /// Returns the validated host kind and retained address or name components.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::net::Ipv6Addr;
    ///
    /// use http_headers::headers::{HostKind, HostOwned};
    ///
    /// let value = HostOwned::from_ipv6(Ipv6Addr::LOCALHOST, Some(443));
    /// assert_eq!(value.kind(), HostKind::Ipv6(Ipv6Addr::LOCALHOST));
    /// assert_eq!(value.network_port(), Ok(Some(443)));
    /// ```
    #[must_use]
    #[inline]
    #[expect(clippy::missing_panics_doc, reason = "the offsets are private and validated at construction")]
    pub fn kind(&self) -> HostKind<'_> {
        self.parsed.kind.project(
            self.host().expect("host offsets were validated when constructing this value"),
            self.parsed.normalized.as_deref(),
        )
    }

    /// Returns the validated textual port, including an explicitly empty port.
    #[must_use]
    #[inline]
    #[expect(clippy::missing_panics_doc, reason = "the offsets are private and validated at construction")]
    pub fn port_view(&self) -> Option<HostPortView<'_>> {
        self.port()
            .expect("port offsets were validated when constructing this value")
            .zip(self.parsed.numeric_port)
            .map(|(text, numeric)| HostPortView { text, numeric })
    }

    /// Returns the checked network port, or `None` only when no port is present.
    ///
    /// # Errors
    ///
    /// Returns an error for an explicitly empty port or a value above `u16::MAX`.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::{HostOwned, PortConversionErrorKind};
    ///
    /// let value = HostOwned::try_from("example.com:65536")?;
    /// assert_eq!(value.port()?, Some("65536"));
    /// assert_eq!(
    ///     value.network_port().unwrap_err().kind(),
    ///     PortConversionErrorKind::Overflow
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[inline]
    pub fn network_port(&self) -> Result<Option<u16>, PortConversionError> {
        self.parsed.numeric_port.transpose()
    }

    /// Constructs a host without a port.
    ///
    /// # Errors
    ///
    /// Returns an error when `host` is not a valid URI host.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::HostOwned;
    ///
    /// let value = HostOwned::new("example.com")?;
    /// assert_eq!(value.host()?, "example.com");
    /// assert_eq!(value.port()?, None);
    ///
    /// assert!(HostOwned::new("example.com:443").is_err());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn new(host: impl AsRef<str>) -> Result<Self, DecodeError> {
        let parsed = Self::try_from(host.as_ref())?;
        if parsed.port_start().is_some() {
            return Err(invalid_syntax(&FieldName::Host));
        }
        Ok(parsed)
    }

    /// Constructs a host with a decimal port.
    ///
    /// IPv6 literals must include their square brackets.
    ///
    /// # Errors
    ///
    /// Returns an error when `host` is invalid.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::HostOwned;
    ///
    /// let value = HostOwned::with_port("[2001:db8::1]", 443)?;
    /// assert_eq!(value.host()?, "[2001:db8::1]");
    /// assert_eq!(value.port()?, Some("443"));
    /// assert_eq!(value.as_str()?, "[2001:db8::1]:443");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn with_port(host: impl AsRef<str>, port: u16) -> Result<Self, DecodeError> {
        let host = host.as_ref();
        let mut wire = String::with_capacity(host.len().saturating_add(6));
        wire.push_str(host);
        wire.push(':');
        write!(&mut wire, "{port}").map_err(|_invalid| invalid_syntax(&FieldName::Host))?;
        let mut parsed = match parse_host(host.as_bytes()) {
            Ok(parsed) if parsed.port_start.is_none() => parsed,
            _ => return Self::try_from(wire),
        };
        parsed.port_start = Some(host.len() + 1);
        parsed.numeric_port = Some(Ok(port));
        Ok(Self {
            value: FieldValue::try_from(wire).map_err(|_invalid| invalid_syntax(&FieldName::Host))?,
            parsed,
        })
    }

    /// Returns the URI host, including brackets around an IP literal.
    ///
    /// # Errors
    ///
    /// Returns an error if stored metadata does not match the wire value.
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::HostOwned::try_from("example.com:443")?;
    /// assert_eq!(value.host()?, "example.com");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn host(&self) -> Result<&str, DecodeError> {
        component(self.value.as_bytes(), 0..self.parsed.host_end)
    }

    /// Returns the optional decimal port.
    ///
    /// The port is textual because URI syntax does not impose a `u16` range.
    ///
    /// # Errors
    ///
    /// Returns an error if stored metadata does not match the wire value.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::HostOwned;
    ///
    /// let value = HostOwned::try_from("example.com:8443")?;
    /// assert_eq!(value.port()?, Some("8443"));
    ///
    /// let default_port = HostOwned::try_from("example.com")?;
    /// assert_eq!(default_port.port()?, None);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn port(&self) -> Result<Option<&str>, DecodeError> {
        self.port_start()
            .map(|start| component(self.value.as_bytes(), start..self.value.as_bytes().len()))
            .transpose()
    }

    /// Returns the retained port offset.
    const fn port_start(&self) -> Option<usize> {
        self.parsed.port_start
    }

    /// Returns the complete authority.
    ///
    /// # Errors
    ///
    /// Returns an error if the stored wire value is unexpectedly non-UTF-8.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::HostOwned;
    ///
    /// let value = HostOwned::with_port("example.com", 8443)?;
    /// assert_eq!(value.as_str()?, "example.com:8443");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_str(&self) -> Result<&str, DecodeError> {
        str::from_utf8(self.value.as_bytes()).map_err(|_invalid| invalid(&FieldName::Host, DecodeErrorKind::InvalidUtf8))
    }

    /// Returns the original field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::HostOwned;
    ///
    /// let value = HostOwned::try_from("example.com:443")?;
    /// assert_eq!(value.as_field_value().as_bytes(), b"example.com:443");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_field_value(&self) -> &FieldValue {
        &self.value
    }

    /// Returns reusable wire storage.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::HostOwned;
    ///
    /// let value = HostOwned::try_from("example.com")?;
    /// let field_value = value.into_field_value();
    /// assert_eq!(field_value.as_bytes(), b"example.com");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }
}

super::super::shared::impl_field_value_conversion!(HostOwned, |value| value.value);

impl<'a> HostView<'a> {
    /// Returns the validated host kind and retained address or name components.
    ///
    /// A relaxed international name borrows its normalization from this view.
    #[must_use]
    #[inline]
    pub fn kind(&self) -> HostKind<'_> {
        self.kind.project(self.host, self.normalized.as_deref())
    }

    /// Returns the validated textual port, including an explicitly empty port.
    #[must_use]
    #[inline]
    pub fn port_view(&self) -> Option<HostPortView<'a>> {
        self.port
            .zip(self.numeric_port)
            .map(|(text, numeric)| HostPortView { text, numeric })
    }

    /// Returns the checked network port, or `None` only when no port is present.
    ///
    /// # Errors
    ///
    /// Returns an error for an explicitly empty port or a value above `u16::MAX`.
    #[inline]
    pub fn network_port(&self) -> Result<Option<u16>, PortConversionError> {
        self.numeric_port.transpose()
    }

    /// Returns the URI host, including brackets around an IP literal.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::HostOwned::try_from("example.com:443")?;
    /// assert_eq!(value.host()?, "example.com");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn host(&self) -> &'a str {
        self.host
    }

    /// Returns the optional decimal port.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::Host;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let with_port = <Host as SingleValueField>::decode_view(FieldValueRef::new(b"[::1]:8080"))?;
    /// assert_eq!(with_port.port(), Some("8080"));
    ///
    /// let without_port = <Host as SingleValueField>::decode_view(FieldValueRef::new(b"example.net"))?;
    /// assert_eq!(without_port.port(), None);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn port(&self) -> Option<&'a str> {
        self.port
    }

    /// Returns the complete authority.
    ///
    /// # Errors
    ///
    /// Returns an error if the wire value is unexpectedly non-UTF-8.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::Host;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view = <Host as SingleValueField>::decode_view(FieldValueRef::new(b"example.com:8080"))?;
    /// assert_eq!(view.as_str()?, "example.com:8080");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_str(&self) -> Result<&'a str, DecodeError> {
        self.value
            .to_str()
            .map_err(|_invalid| invalid(&FieldName::Host, DecodeErrorKind::InvalidUtf8))
    }

    /// Returns the original field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::Host;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view = <Host as SingleValueField>::decode_view(FieldValueRef::new(b"example.org"))?;
    /// assert_eq!(view.as_field_value().as_bytes(), b"example.org");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn as_field_value(&self) -> FieldValueRef<'a> {
        self.value
    }
}

impl SingleValueField for Host {
    type View<'a> = HostView<'a>;
    type Owned = HostOwned;

    fn name() -> &'static FieldName {
        &FieldName::Host
    }

    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
        let bytes = value.as_bytes();
        let parsed = parse_host(bytes)?;
        let authority = value.to_str().expect("strict host validation accepts only ASCII");
        let (host, port) = match parsed.port_start {
            Some(start) => (&authority[..parsed.host_end], Some(&authority[start..])),
            // Without a port the host runs to the end of the authority.
            None => (authority, None),
        };
        Ok(HostView {
            value,
            host,
            port,
            kind: parsed.kind,
            normalized: parsed.normalized.map(Cow::Owned),
            numeric_port: parsed.numeric_port,
        })
    }

    #[expect(
        clippy::inline_always,
        reason = "measured: Criterion otherwise outlines this cross-crate hot path while Callgrind inlines it"
    )]
    #[inline(always)]
    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
        HostOwned::try_from(value)
    }

    fn decode_view_with(value: FieldValueRef<'_>, mode: crate::DecodeMode) -> Result<Self::View<'_>, DecodeError> {
        let authority = value
            .to_str()
            .map_err(|_invalid| invalid(&FieldName::Host, DecodeErrorKind::InvalidUtf8))?;
        let parsed = parse_host_with(value.as_bytes(), mode)?;
        let (host, port) = match parsed.port_start {
            Some(start) => (&authority[..parsed.host_end], Some(&authority[start..])),
            None => (authority, None),
        };
        Ok(HostView {
            value,
            host,
            port,
            kind: parsed.kind,
            normalized: parsed.normalized.map(Cow::Owned),
            numeric_port: parsed.numeric_port,
        })
    }

    fn decode_owned_with(value: FieldValue, mode: crate::DecodeMode) -> Result<Self::Owned, DecodeError> {
        let parsed = parse_host_with(value.as_bytes(), mode)?;
        Ok(HostOwned { value, parsed })
    }

    fn as_field_value(value: &Self::Owned) -> &FieldValue {
        &value.value
    }

    fn into_field_value(value: Self::Owned) -> FieldValue {
        value.value
    }
}

super::super::shared::impl_string_conversions!(HostOwned, &FieldName::Host, invalid_syntax, value);

impl TryFrom<FieldValue> for HostOwned {
    type Error = DecodeError;

    #[expect(
        clippy::inline_always,
        reason = "measured: outlining this conversion leaves large Result traffic in Criterion"
    )]
    #[inline(always)]
    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        let parsed = parse_host(value.as_bytes())?;
        Ok(Self { value, parsed })
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct ParsedHost {
    host_end: usize,
    port_start: Option<usize>,
    kind: ParsedHostKind,
    normalized: Option<String>,
    numeric_port: Option<Result<u16, PortConversionError>>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum ParsedHostKind {
    RegisteredName,
    Ipv4(Ipv4Addr),
    Ipv6(Ipv6Addr),
    IpvFuture { dot: usize },
}

impl ParsedHostKind {
    fn project<'a>(self, host: &'a str, normalized: Option<&'a str>) -> HostKind<'a> {
        match self {
            Self::RegisteredName => HostKind::RegisteredName(RegisteredNameView {
                original: host,
                normalized: normalized.unwrap_or(host),
            }),
            Self::Ipv4(address) => HostKind::Ipv4(address),
            Self::Ipv6(address) => HostKind::Ipv6(address),
            Self::IpvFuture { dot } => HostKind::IpvFuture(IpvFutureView {
                version: &host[2..dot],
                address: &host[dot + 1..host.len() - 1],
            }),
        }
    }
}

#[expect(
    clippy::inline_always,
    reason = "measured: Callgrind inlines this parser while Criterion otherwise emits a real call"
)]
#[inline(always)]
fn parse_host(bytes: &[u8]) -> Result<ParsedHost, DecodeError> {
    let Some(first) = bytes.first().copied() else {
        return Err(invalid_syntax(&FieldName::Host));
    };
    if first == b'[' {
        return parse_ip_literal_host(bytes);
    }
    let mut index = 0;
    let mut ipv4_candidate = first.is_ascii_digit();
    while index < bytes.len() {
        // The table already covers the alphanumerics, `-`, and `.` that make
        // up nearly every host, so one load settles a byte where a range test
        // ahead of the load would cost several compares.
        match HOST_CLASS[usize::from(bytes[index])] {
            HOST_REG_NAME => {
                ipv4_candidate = ipv4_candidate && (bytes[index].is_ascii_digit() || bytes[index] == b'.');
                index += 1;
            }
            HOST_PERCENT => {
                if !bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit) || !bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit) {
                    return Err(invalid_syntax(&FieldName::Host));
                }
                ipv4_candidate = false;
                index += 3;
            }
            HOST_COLON => {
                if index == 0 {
                    return Err(invalid_syntax(&FieldName::Host));
                }
                let numeric_port = validate_port(&bytes[index + 1..])?;
                return Ok(ParsedHost {
                    host_end: index,
                    port_start: Some(index + 1),
                    kind: registered_host_kind(&bytes[..index], ipv4_candidate),
                    normalized: None,
                    numeric_port: Some(numeric_port),
                });
            }
            _ => return Err(invalid_syntax(&FieldName::Host)),
        }
    }
    Ok(ParsedHost {
        host_end: bytes.len(),
        port_start: None,
        kind: registered_host_kind(bytes, ipv4_candidate),
        normalized: None,
        numeric_port: None,
    })
}

fn registered_host_kind(bytes: &[u8], ipv4_candidate: bool) -> ParsedHostKind {
    if ipv4_candidate {
        let text = str::from_utf8(bytes).expect("the registered-name scan accepts only ASCII");
        if let Ok(address) = text.parse() {
            return ParsedHostKind::Ipv4(address);
        }
    }
    ParsedHostKind::RegisteredName
}

fn parse_host_with(bytes: &[u8], mode: crate::DecodeMode) -> Result<ParsedHost, DecodeError> {
    if mode == crate::DecodeMode::Strict {
        return parse_host(bytes);
    }
    if let Ok(parsed) = parse_host(bytes) {
        return Ok(parsed);
    }
    parse_international_host(bytes)
}

fn parse_international_host(bytes: &[u8]) -> Result<ParsedHost, DecodeError> {
    let authority = str::from_utf8(bytes).map_err(|_invalid| invalid(&FieldName::Host, DecodeErrorKind::InvalidUtf8))?;
    if authority.is_ascii() || authority.contains('@') || authority.starts_with('[') {
        return Err(invalid_syntax(&FieldName::Host));
    }
    let (host, port, port_start) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') && port.bytes().all(|byte| byte.is_ascii_digit()) => {
            (host, Some(port), Some(host.len() + 1))
        }
        Some(_) if authority.contains(':') => return Err(invalid_syntax(&FieldName::Host)),
        _ => (authority, None, None),
    };
    let mut normalized = domain_to_ascii(host).map_err(|_invalid| invalid_syntax(&FieldName::Host))?;
    let normalized_end = normalized.len();
    if let Some(port) = port {
        normalized.push(':');
        normalized.push_str(port);
    }
    let parsed = parse_host(normalized.as_bytes())?;
    normalized.truncate(normalized_end);
    Ok(ParsedHost {
        host_end: host.len(),
        port_start,
        kind: ParsedHostKind::RegisteredName,
        normalized: Some(normalized),
        numeric_port: port.and(parsed.numeric_port),
    })
}

/// Checks the authority's port, mirroring the whole-value checks it replaces.
///
/// A byte that a field value may never contain, a byte outside ASCII, and a
/// second colon are syntax errors wherever they appear, so they outrank the
/// numeric error reported for a merely non-decimal port.
fn validate_port(bytes: &[u8]) -> Result<Result<u16, PortConversionError>, DecodeError> {
    let mut decimal = true;
    let mut numeric = Some(0_u16);
    for byte in bytes {
        if byte.is_ascii_digit() {
            // A u16 accumulator and a decimal digit produce at most 655_359.
            numeric = numeric.and_then(|value| u16::try_from(u32::from(value) * 10 + u32::from(*byte - b'0')).ok());
            continue;
        }
        if *byte == b':' || *byte == b'@' || *byte == 0x7f || (*byte < b' ' && *byte != b'\t') {
            return Err(invalid_syntax(&FieldName::Host));
        }
        if !byte.is_ascii() {
            return Err(invalid_syntax(&FieldName::Host));
        }
        decimal = false;
    }
    if decimal {
        Ok(if bytes.is_empty() {
            Err(PortConversionError {
                kind: PortConversionErrorKind::Empty,
            })
        } else {
            numeric.ok_or(PortConversionError {
                kind: PortConversionErrorKind::Overflow,
            })
        })
    } else {
        Err(invalid(&FieldName::Host, DecodeErrorKind::InvalidNumber))
    }
}

/// Byte classes recognized while scanning an authority's registered name.
/// Consecutive small integers keep the 256-byte lookup table compact.
const HOST_OTHER: u8 = 0;
const HOST_REG_NAME: u8 = 1;
const HOST_PERCENT: u8 = 2;
const HOST_COLON: u8 = 3;

/// Maps every byte to its role in an authority's registered name.
static HOST_CLASS: [u8; 256] = {
    let mut table = [HOST_OTHER; 256];
    let mut index = 0_u8;
    loop {
        table[index as usize] = if is_unreserved(index) || is_sub_delim(index) {
            HOST_REG_NAME
        } else if index == b'%' {
            HOST_PERCENT
        } else if index == b':' {
            HOST_COLON
        } else {
            HOST_OTHER
        };
        if index == u8::MAX {
            break;
        }
        index += 1;
    }
    table
};

fn parse_ip_literal_host(bytes: &[u8]) -> Result<ParsedHost, DecodeError> {
    if !validate::field_value(bytes) || !bytes.is_ascii() || bytes.contains(&b'@') {
        return Err(invalid_syntax(&FieldName::Host));
    }
    let close = bytes
        .iter()
        .position(|byte| *byte == b']')
        .ok_or_else(|| invalid_syntax(&FieldName::Host))?;
    let literal = &bytes[1..close];
    let kind = parse_ip_literal(literal).ok_or_else(|| invalid_syntax(&FieldName::Host))?;
    let suffix = &bytes[close + 1..];
    let mut numeric_port = None;
    let port_start = if suffix.is_empty() {
        None
    } else if let Some(port) = suffix.strip_prefix(b":") {
        let mut numeric = Some(0_u16);
        for byte in port {
            if !byte.is_ascii_digit() {
                return Err(invalid(&FieldName::Host, DecodeErrorKind::InvalidNumber));
            }
            // A u16 accumulator and a decimal digit produce at most 655_359.
            numeric = numeric.and_then(|value| u16::try_from(u32::from(value) * 10 + u32::from(*byte - b'0')).ok());
        }
        numeric_port = Some(if port.is_empty() {
            Err(PortConversionError {
                kind: PortConversionErrorKind::Empty,
            })
        } else {
            numeric.ok_or(PortConversionError {
                kind: PortConversionErrorKind::Overflow,
            })
        });
        Some(close + 2)
    } else {
        return Err(invalid_syntax(&FieldName::Host));
    };
    Ok(ParsedHost {
        host_end: close + 1,
        port_start,
        kind,
        normalized: None,
        numeric_port,
    })
}

fn parse_ip_literal(bytes: &[u8]) -> Option<ParsedHostKind> {
    let value = str::from_utf8(bytes).ok()?;
    let Some(versioned) = bytes.strip_prefix(b"v").or_else(|| bytes.strip_prefix(b"V")) else {
        return Ipv6Addr::from_str(value).ok().map(ParsedHostKind::Ipv6);
    };
    let dot = versioned.iter().position(|byte| *byte == b'.')?;
    let version = &versioned[..dot];
    let address = &versioned[dot + 1..];
    let valid = !version.is_empty()
        && version.iter().all(u8::is_ascii_hexdigit)
        && !address.is_empty()
        && address
            .iter()
            .copied()
            .all(|byte| is_unreserved(byte) || is_sub_delim(byte) || byte == b':');
    valid.then_some(ParsedHostKind::IpvFuture { dot: dot + 2 })
}

#[cfg(test)]
fn valid_ip_literal(bytes: &[u8]) -> bool {
    parse_ip_literal(bytes).is_some()
}
const fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

const fn is_sub_delim(byte: u8) -> bool {
    matches!(byte, b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=')
}

fn component(bytes: &[u8], range: Range<usize>) -> Result<&str, DecodeError> {
    let bytes = bytes.get(range).ok_or_else(|| invalid_syntax(&FieldName::Host))?;
    str::from_utf8(bytes).map_err(|_invalid| invalid(&FieldName::Host, DecodeErrorKind::InvalidUtf8))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{
        Host, HostOwned, HostView, component, parse_host, parse_host_with, parse_international_host, valid_ip_literal, validate_port,
    };
    use crate::{DecodeErrorKind, DecodeMode, FieldName, FieldValue, FieldValueRef, SingleValueField};

    #[test]
    fn constructors_and_accessors_preserve_authority_components() {
        let plain = HostOwned::new("example.com").expect("host without port");
        assert_eq!(plain.host(), Ok("example.com"));
        assert_eq!(plain.port(), Ok(None));
        assert_eq!(plain.as_str(), Ok("example.com"));
        assert_eq!(plain.as_field_value().as_bytes(), b"example.com");
        assert_eq!(plain.into_field_value().as_bytes(), b"example.com");

        let with_port = HostOwned::with_port("[2001:db8::1]", 443).expect("IPv6 and port");
        assert_eq!(with_port.host(), Ok("[2001:db8::1]"));
        assert_eq!(with_port.port(), Ok(Some("443")));
        assert_eq!(
            HostOwned::new("example.com:80").expect_err("new excludes ports").kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let view = <Host as SingleValueField>::decode_view(FieldValueRef::new(b"example.com:8080")).expect("borrowed authority");
        assert_eq!(view.host(), "example.com");
        assert_eq!(view.port(), Some("8080"));
        assert_eq!(view.as_str(), Ok("example.com:8080"));
        assert_eq!(view.as_field_value().as_bytes(), b"example.com:8080");

        let no_port = <Host as SingleValueField>::decode_view(FieldValueRef::new(b"example.net")).expect("borrowed authority without port");
        assert_eq!(no_port.host(), "example.net");
        assert_eq!(no_port.port(), None);

        let decoded = <Host as SingleValueField>::decode_owned(FieldValue::from_static("example.org:8443")).expect("owned authority");
        assert_eq!(<Host as SingleValueField>::as_field_value(&decoded).as_bytes(), b"example.org:8443");
        assert_eq!(
            <Host as SingleValueField>::into_field_value(decoded).as_bytes(),
            b"example.org:8443"
        );
        assert_eq!(<Host as SingleValueField>::name(), &FieldName::Host);
    }

    #[test]
    fn strict_parser_covers_registered_names_ports_and_literal_forms() {
        for valid in [
            b"example.com".as_slice(),
            b"exa_mple~host",
            b"example%20host",
            b"example.com:",
            b"[2001:db8::1]",
            b"[v1.alpha:beta]:443",
        ] {
            assert!(parse_host(valid).is_ok(), "{valid:?}");
        }
        for invalid in [
            b"".as_slice(),
            b":80",
            b"host%2",
            b"host%zz",
            b"host@name",
            b"host:bad",
            b"host:8:0",
            b"[",
            b"[]",
            b"[2001:db8::1",
            b"[2001:db8::1]tail",
            b"[2001:db8::1]:bad",
        ] {
            assert!(parse_host(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn relaxed_parser_accepts_idna_but_not_ambiguous_authorities() {
        let parsed = parse_host_with("münich.example:443".as_bytes(), DecodeMode::Relaxed).expect("international host");
        assert_eq!(parsed.host_end, "münich.example".len());
        assert_eq!(parsed.port_start, Some("münich.example:".len()));

        let strict = parse_host_with(b"example.com", DecodeMode::Strict).expect("strict host");
        assert_eq!(strict.host_end, 11);
        assert_eq!(strict.port_start, None);
        let relaxed_ascii = parse_host_with(b"example.net", DecodeMode::Relaxed).expect("strict syntax fast path");
        assert_eq!(relaxed_ascii.host_end, 11);

        for invalid in [
            b"bad host".as_slice(),
            "münich@example".as_bytes(),
            "[münich]".as_bytes(),
            "münich:bad".as_bytes(),
            b"\xff",
        ] {
            assert!(parse_host_with(invalid, DecodeMode::Relaxed).is_err(), "{invalid:?}");
        }
        assert_eq!(
            <Host as SingleValueField>::decode_view_with(FieldValueRef::new(b"\xff"), DecodeMode::Relaxed,)
                .expect_err("relaxed views still require UTF-8")
                .kind(),
            DecodeErrorKind::InvalidUtf8
        );
        assert!(
            parse_international_host("\u{200d}.example".as_bytes()).is_err(),
            "context-invalid IDNA label"
        );
        assert!(
            parse_international_host("\u{00ad}".as_bytes()).is_err(),
            "IDNA mappings may not erase the complete host"
        );

        let value = FieldValue::from_bytes("münich.example:443".as_bytes()).expect("UTF-8 field value");
        let view =
            <Host as SingleValueField>::decode_view_with(value.as_field_value_ref(), DecodeMode::Relaxed).expect("relaxed borrowed host");
        assert_eq!(view.host(), "münich.example");
        assert_eq!(view.port(), Some("443"));
        drop(view);
        let owned = <Host as SingleValueField>::decode_owned_with(value, DecodeMode::Relaxed).expect("relaxed owned host");
        assert_eq!(owned.as_str(), Ok("münich.example:443"));
        assert_eq!(owned.host(), Ok("münich.example"));
        assert_eq!(owned.port(), Ok(Some("443")));

        let value = FieldValue::from_bytes("münich.example".as_bytes()).expect("UTF-8 field value");
        let view = <Host as SingleValueField>::decode_view_with(value.as_field_value_ref(), DecodeMode::Relaxed)
            .expect("relaxed host without port");
        assert_eq!(view.host(), "münich.example");
        assert_eq!(view.port(), None);
        drop(view);
        let owned = <Host as SingleValueField>::decode_owned_with(value, DecodeMode::Relaxed).expect("relaxed owned host without port");
        assert_eq!(owned.host(), Ok("münich.example"));
        assert_eq!(owned.port(), Ok(None));
    }

    #[test]
    fn private_component_validators_report_precise_failures() {
        for valid in [b"".as_slice(), b"0", b"65536"] {
            assert!(validate_port(valid).is_ok(), "{valid:?}");
        }
        for syntax in [b"8:0".as_slice(), b"user@", b"\x7f", b"\x01", b"\xff"] {
            assert_eq!(
                validate_port(syntax).expect_err("syntax error").kind(),
                DecodeErrorKind::InvalidSyntax
            );
        }
        assert_eq!(
            validate_port(b"http").expect_err("not decimal").kind(),
            DecodeErrorKind::InvalidNumber
        );

        for valid in [b"2001:db8::1".as_slice(), b"vF.alpha:beta"] {
            assert!(valid_ip_literal(valid), "{valid:?}");
        }
        for invalid in [b"\xff".as_slice(), b"not-ip", b"v.", b"v1", b"v1.", b"v1.bad?"] {
            assert!(!valid_ip_literal(invalid), "{invalid:?}");
        }

        assert_eq!(component(b"abc", 0..3), Ok("abc"));
        assert_eq!(
            component(b"abc", 0..4).expect_err("out of bounds").kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            component(b"\xff", 0..1).expect_err("invalid UTF-8").kind(),
            DecodeErrorKind::InvalidUtf8
        );
        assert_eq!(
            HostOwned::try_from(String::from("line\nbreak"))
                .expect_err("invalid field value")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            HostOwned::try_from("line\nbreak").expect_err("invalid borrowed field value").kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            HostOwned::try_from(FieldValue::from_static("bad host"))
                .expect_err("invalid authority")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }

    #[test]
    fn defensive_owned_and_view_accessors_reject_invalid_private_storage() {
        let malformed = HostOwned {
            value: FieldValue::from_static("short"),
            parsed: super::ParsedHost {
                host_end: 6,
                port_start: Some(6),
                kind: super::ParsedHostKind::RegisteredName,
                normalized: None,
                numeric_port: None,
            },
        };
        assert_eq!(
            malformed.host().expect_err("invalid host metadata").kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            malformed.port().expect_err("invalid port metadata").kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let non_utf8 = FieldValue::from_bytes(b"\xff").expect("obs-text is a valid field value");
        let malformed = HostOwned {
            value: non_utf8.clone(),
            parsed: super::ParsedHost {
                host_end: 1,
                port_start: None,
                kind: super::ParsedHostKind::RegisteredName,
                normalized: None,
                numeric_port: None,
            },
        };
        assert_eq!(malformed.as_str().expect_err("invalid UTF-8").kind(), DecodeErrorKind::InvalidUtf8);
        let view = HostView {
            value: non_utf8.as_field_value_ref(),
            host: "",
            port: None,
            kind: super::ParsedHostKind::RegisteredName,
            normalized: None,
            numeric_port: None,
        };
        assert_eq!(view.as_str().expect_err("invalid UTF-8 view").kind(), DecodeErrorKind::InvalidUtf8);
    }
}
