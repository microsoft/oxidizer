// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Write as _};
use std::net::Ipv6Addr;
use std::ops::Range;
use std::str::{self, FromStr as _};

use super::shared::{
    BYTE_CLASS, CLASS_AUTHORITY, CLASS_DIGIT, CLASS_DOMAIN, CLASS_HEX, CLASS_LABEL_EDGE, CLASS_SCHEME, invalid_syntax, trimmed_range,
};
use crate::sink::{EncodedValues, FieldSink, InsertError};
use crate::source::FieldSource;
use crate::{DecodeError, DecodeErrorKind, Field, FieldName, FieldValue, FieldValueRef};

/// Defines the `Access-Control-Allow-Origin` header.
///
/// Serialized tuple origins are limited to the `ftp`, `http`, `https`, `ws`,
/// and `wss` schemes. Origins that serialize opaquely are represented by the
/// case-sensitive `null` value rather than a scheme-and-host value.
///
/// # Specification
///
/// Defined by the Fetch standard's
/// [CORS protocol and credentials section](https://fetch.spec.whatwg.org/#http-access-control-allow-origin).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{AccessControlAllowOrigin, AccessControlAllowOriginOwned};
///
/// let mut map = HeaderMap::new();
/// AccessControlAllowOrigin::insert(&mut map, AccessControlAllowOriginOwned::wildcard())?;
/// assert!(AccessControlAllowOrigin::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct AccessControlAllowOrigin {
    _private: (),
}

/// Owned value for the `Access-Control-Allow-Origin` header.
///
/// # Specification
///
/// Defined by the Fetch standard's [CORS protocol and credentials section].
///
/// # Examples
///
/// ```rust
/// let value =
///     http_headers::headers::AccessControlAllowOriginOwned::try_from("https://example.com")?;
/// assert_eq!(value.origin()?, Some("https://example.com"));
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Access-Control-Allow-Origin: *` permits a wildcard origin,
/// `Access-Control-Allow-Origin: null` carries the opaque origin, and
/// `Access-Control-Allow-Origin: https://api.example.com:8443` carries a
/// serialized origin.
///
/// [CORS protocol and credentials section]: https://fetch.spec.whatwg.org/#http-access-control-allow-origin
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct AccessControlAllowOriginOwned {
    value: FieldValue,
    parsed: ParsedOrigin,
}

/// Borrowed value for the `Access-Control-Allow-Origin` header.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), http_headers::DecodeError> {
/// use http::{HeaderMap, HeaderValue};
/// use http_headers::Field;
/// use http_headers::headers::{AccessControlAllowOrigin, AccessControlAllowOriginView};
///
/// let mut headers = HeaderMap::new();
/// headers.insert(
///     "access-control-allow-origin",
///     HeaderValue::from_static("null"),
/// );
/// let value: AccessControlAllowOriginView<'_> =
///     AccessControlAllowOrigin::view(&headers)?.expect("present");
/// assert!(value.is_null());
/// assert_eq!(value.as_str(), "null");
/// # Ok::<(), http_headers::DecodeError>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
pub struct AccessControlAllowOriginView<'a> {
    value: FieldValueRef<'a>,
    serialized: &'a str,
    start: usize,
    end: usize,
    kind: OriginKind,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ParsedOrigin {
    start: usize,
    end: usize,
    kind: OriginKind,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum OriginKind {
    Wildcard,
    Null,
    Origin,
}

impl fmt::Debug for AccessControlAllowOriginOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessControlAllowOriginOwned")
            .field("kind", &self.parsed.kind)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for AccessControlAllowOriginView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessControlAllowOriginView")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl AccessControlAllowOriginOwned {
    /// Constructs the wildcard value.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::AccessControlAllowOriginOwned;
    ///
    /// let value = AccessControlAllowOriginOwned::wildcard();
    /// assert!(value.is_wildcard());
    /// assert_eq!(value.as_str()?, "*");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn wildcard() -> Self {
        Self {
            value: FieldValue::from_static("*"),
            parsed: ParsedOrigin {
                start: 0,
                end: 1,
                kind: OriginKind::Wildcard,
            },
        }
    }

    /// Constructs the case-sensitive `null` origin value.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::AccessControlAllowOriginOwned;
    ///
    /// let value = AccessControlAllowOriginOwned::null();
    /// assert!(value.is_null());
    /// assert_eq!(value.as_str()?, "null");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn null() -> Self {
        Self {
            value: FieldValue::from_static("null"),
            parsed: ParsedOrigin {
                start: 0,
                end: 4,
                kind: OriginKind::Null,
            },
        }
    }

    /// Constructs a serialized origin.
    ///
    /// This accepts tuple origins with the `ftp`, `http`, `https`, `ws`, and
    /// `wss` schemes. Origins that serialize opaquely must use [`Self::null`].
    /// Paths, queries, fragments, user information, uppercase schemes or
    /// domains, and non-serialized IP addresses are rejected.
    ///
    /// # Errors
    ///
    /// Returns an error if `origin` is not a serialized origin.
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::AccessControlAllowOriginOwned;
    ///
    /// let value = AccessControlAllowOriginOwned::from_origin("https://example.com")?;
    /// assert_eq!(value.origin()?, Some("https://example.com"));
    /// assert!(!value.is_wildcard());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn from_origin(origin: impl AsRef<str>) -> Result<Self, DecodeError> {
        let parsed = Self::try_from(origin.as_ref())?;
        if parsed.parsed.kind == OriginKind::Origin {
            Ok(parsed)
        } else {
            Err(invalid_syntax(&FieldName::AccessControlAllowOrigin))
        }
    }

    #[cfg(all(feature = "serde", feature = "headers-cors"))]
    pub(crate) const fn field_value(&self) -> &FieldValue {
        &self.value
    }

    /// Returns whether this is the wildcard value.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::AccessControlAllowOriginOwned;
    ///
    /// let wildcard = AccessControlAllowOriginOwned::wildcard();
    /// let origin = AccessControlAllowOriginOwned::from_origin("https://example.com")?;
    /// assert!(wildcard.is_wildcard());
    /// assert!(!origin.is_wildcard());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn is_wildcard(&self) -> bool {
        matches!(self.parsed.kind, OriginKind::Wildcard)
    }

    /// Returns whether this is the `null` origin value.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::AccessControlAllowOriginOwned;
    ///
    /// let null = AccessControlAllowOriginOwned::null();
    /// let wildcard = AccessControlAllowOriginOwned::wildcard();
    /// assert!(null.is_null());
    /// assert!(!wildcard.is_null());
    /// ```
    pub const fn is_null(&self) -> bool {
        matches!(self.parsed.kind, OriginKind::Null)
    }

    /// Returns a serialized origin, excluding wildcard and `null`.
    ///
    /// # Errors
    ///
    /// Returns an error if stored metadata does not match the field value.
    /// # Examples
    ///
    /// ```rust
    /// let value =
    ///     http_headers::headers::AccessControlAllowOriginOwned::try_from("https://example.com")?;
    /// assert_eq!(value.origin()?, Some("https://example.com"));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn origin(&self) -> Result<Option<&str>, DecodeError> {
        if self.parsed.kind != OriginKind::Origin {
            return Ok(None);
        }
        semantic_str(
            &FieldName::AccessControlAllowOrigin,
            self.value.as_bytes(),
            self.parsed.start..self.parsed.end,
        )
        .map(Some)
    }

    /// Returns the semantic value after surrounding optional whitespace.
    ///
    /// # Errors
    ///
    /// Returns an error if stored metadata does not match the field value.
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::AccessControlAllowOriginOwned;
    ///
    /// let null = AccessControlAllowOriginOwned::try_from(" null ")?;
    /// let origin = AccessControlAllowOriginOwned::from_origin("https://example.com")?;
    /// assert_eq!(null.as_str()?, "null");
    /// assert_eq!(origin.as_str()?, "https://example.com");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_str(&self) -> Result<&str, DecodeError> {
        semantic_str(
            &FieldName::AccessControlAllowOrigin,
            self.value.as_bytes(),
            self.parsed.start..self.parsed.end,
        )
    }

    /// Returns reusable wire storage.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::AccessControlAllowOriginOwned;
    ///
    /// let value = AccessControlAllowOriginOwned::from_origin("https://example.com")?;
    /// let field_value = value.into_field_value();
    /// assert_eq!(field_value.as_bytes(), b"https://example.com");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }
}

super::super::shared::impl_field_value_conversion!(AccessControlAllowOriginOwned, |value| value.value);

impl fmt::Display for AccessControlAllowOriginOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str().map_err(|_invalid| fmt::Error)?)
    }
}

impl<'a> AccessControlAllowOriginView<'a> {
    /// Returns whether this is the wildcard value.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), http_headers::DecodeError> {
    /// use http::{HeaderMap, HeaderValue};
    /// use http_headers::Field;
    /// use http_headers::headers::AccessControlAllowOrigin;
    ///
    /// let mut headers = HeaderMap::new();
    /// headers.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    /// let value = AccessControlAllowOrigin::view(&headers)?.expect("present");
    /// assert!(value.is_wildcard());
    /// assert_eq!(value.origin(), None);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    pub const fn is_wildcard(self) -> bool {
        matches!(self.kind, OriginKind::Wildcard)
    }

    /// Returns whether this is the `null` origin value.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), http_headers::DecodeError> {
    /// use http::{HeaderMap, HeaderValue};
    /// use http_headers::Field;
    /// use http_headers::headers::AccessControlAllowOrigin;
    ///
    /// let mut headers = HeaderMap::new();
    /// headers.insert(
    ///     "access-control-allow-origin",
    ///     HeaderValue::from_static("null"),
    /// );
    /// let value = AccessControlAllowOrigin::view(&headers)?.expect("present");
    /// assert!(value.is_null());
    /// assert_eq!(value.origin(), None);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    pub const fn is_null(self) -> bool {
        matches!(self.kind, OriginKind::Null)
    }

    /// Returns a serialized origin, excluding wildcard and `null`.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// let value =
    ///     http_headers::headers::AccessControlAllowOriginOwned::try_from("https://example.com")?;
    /// assert_eq!(value.origin()?, Some("https://example.com"));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn origin(self) -> Option<&'a str> {
        if matches!(self.kind, OriginKind::Origin) {
            Some(self.serialized)
        } else {
            None
        }
    }

    /// Returns the semantic value after surrounding optional whitespace.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), http_headers::DecodeError> {
    /// use http::{HeaderMap, HeaderValue};
    /// use http_headers::Field;
    /// use http_headers::headers::AccessControlAllowOrigin;
    ///
    /// let mut headers = HeaderMap::new();
    /// headers.insert(
    ///     "access-control-allow-origin",
    ///     HeaderValue::from_static(" https://example.com "),
    /// );
    /// let value = AccessControlAllowOrigin::view(&headers)?.expect("present");
    /// assert_eq!(value.as_str(), "https://example.com");
    /// assert_eq!(value.origin(), Some("https://example.com"));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    pub const fn as_str(self) -> &'a str {
        self.serialized
    }

    /// Returns the original field value.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use http::{HeaderMap, HeaderValue};
    /// use http_headers::Field;
    /// use http_headers::headers::AccessControlAllowOrigin;
    ///
    /// let mut headers = HeaderMap::new();
    /// headers.insert(
    ///     "access-control-allow-origin",
    ///     HeaderValue::from_static(" https://example.com "),
    /// );
    /// let value = AccessControlAllowOrigin::view(&headers)?.expect("present");
    /// assert_eq!(value.as_field_value().as_str()?, " https://example.com ");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    pub const fn as_field_value(self) -> FieldValueRef<'a> {
        self.value
    }
}

impl Field for AccessControlAllowOrigin {
    type View<'a> = AccessControlAllowOriginView<'a>;
    type Owned = AccessControlAllowOriginOwned;

    fn name() -> &'static FieldName {
        &FieldName::AccessControlAllowOrigin
    }

    #[inline]
    fn view_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        lines.validate_custom_source()?;
        let value = lines.exactly_one()?;
        let (parsed, serialized) = parse_allow_origin_value(value)?;
        Ok(Some(AccessControlAllowOriginView {
            value,
            serialized,
            start: parsed.start,
            end: parsed.end,
            kind: parsed.kind,
        }))
    }

    /// Parses straight into owned storage, since the serialized origin a view
    /// borrows is not kept.
    fn owned_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        let owned = lines.exactly_one_owned()?;
        let parsed = parse_allow_origin_value(owned.as_field_value_ref())?.0;
        Ok(Some(AccessControlAllowOriginOwned { value: owned, parsed }))
    }

    fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        sink.set_values(Self::name(), EncodedValues::single(value.value))
    }
}

impl TryFrom<&str> for AccessControlAllowOriginOwned {
    type Error = DecodeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = FieldValue::from_str(value).map_err(|_invalid| invalid_syntax(&FieldName::AccessControlAllowOrigin))?;
        Self::try_from(value)
    }
}

impl TryFrom<String> for AccessControlAllowOriginOwned {
    type Error = DecodeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let value = FieldValue::try_from(value).map_err(|_invalid| invalid_syntax(&FieldName::AccessControlAllowOrigin))?;
        Self::try_from(value)
    }
}

impl TryFrom<FieldValue> for AccessControlAllowOriginOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        let parsed = parse_allow_origin_value(value.as_field_value_ref())?.0;
        Ok(Self { value, parsed })
    }
}
#[inline]
fn parse_allow_origin_value(value: FieldValueRef<'_>) -> Result<(ParsedOrigin, &str), DecodeError> {
    let range = trimmed_range(value.as_bytes());
    let serialized = visible_ascii_str(value, range.clone()).map_or_else(
        || semantic_str(&FieldName::AccessControlAllowOrigin, value.as_bytes(), range.clone()),
        Ok,
    )?;
    let kind = match serialized.as_bytes() {
        b"*" => OriginKind::Wildcard,
        b"null" => OriginKind::Null,
        origin if valid_serialized_origin(origin) => OriginKind::Origin,
        _ => return Err(invalid_syntax(&FieldName::AccessControlAllowOrigin)),
    };
    Ok((
        ParsedOrigin {
            start: range.start,
            end: range.end,
            kind,
        },
        serialized,
    ))
}

fn valid_serialized_origin(origin: &[u8]) -> bool {
    if let Some(valid) = common_http_origin(origin) {
        return valid;
    }
    let Some((scheme, authority)) = split_scheme(origin) else {
        return false;
    };
    if !matches!(scheme, b"ftp" | b"http" | b"https" | b"ws" | b"wss") {
        return false;
    }
    valid_serialized_authority(authority, scheme)
}

fn common_http_origin(origin: &[u8]) -> Option<bool> {
    let host = origin.strip_prefix(b"https://").or_else(|| origin.strip_prefix(b"http://"))?;
    let host = host.strip_suffix(b".").unwrap_or(host);
    if host.is_empty() || host.len() > 253 {
        return Some(false);
    }

    let mut label_start = 0_usize;
    let mut saw_non_digit = false;
    for (index, &byte) in host.iter().enumerate() {
        let class = BYTE_CLASS[usize::from(byte)];
        if class & CLASS_DOMAIN != 0 {
            saw_non_digit |= class & CLASS_DIGIT == 0;
            continue;
        }
        if byte == b'.' {
            if !valid_domain_label(&host[label_start..index]) {
                return Some(false);
            }
            label_start = index + 1;
            continue;
        }
        // A port or an IP-literal needs the general authority rules; any other
        // byte is outside `reg-name`, which those rules also reject.
        return (byte != b':' && byte != b'[').then_some(false);
    }
    if !saw_non_digit {
        return None;
    }
    Some(valid_domain_label(&host[label_start..]))
}

#[inline]
fn valid_domain_label(label: &[u8]) -> bool {
    let (Some(&first), Some(&last)) = (label.first(), label.last()) else {
        return false;
    };
    label.len() <= 63 && BYTE_CLASS[usize::from(first)] & CLASS_LABEL_EDGE != 0 && BYTE_CLASS[usize::from(last)] & CLASS_LABEL_EDGE != 0
}

/// Splits a serialized origin at its first `://` and validates the scheme.
fn split_scheme(origin: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut index = 0_usize;
    while let Some(&byte) = origin.get(index) {
        if byte == b':' {
            break;
        }
        if BYTE_CLASS[usize::from(byte)] & CLASS_SCHEME == 0 {
            return None;
        }
        index += 1;
    }
    let rest = index.checked_add(3)?;
    if !origin.first().is_some_and(u8::is_ascii_lowercase) || origin.get(index..rest) != Some(b"://".as_slice()) {
        return None;
    }
    Some((origin.get(..index)?, origin.get(rest..)?))
}

fn valid_serialized_authority(authority: &[u8], scheme: &[u8]) -> bool {
    let Some((&first, rest)) = authority.split_first() else {
        return false;
    };

    if first == b'[' {
        if rest
            .iter()
            .any(|byte| BYTE_CLASS[usize::from(*byte)] & CLASS_AUTHORITY == 0 && *byte != b':')
        {
            return false;
        }
        let Some(close) = rest.iter().position(|byte| *byte == b']') else {
            return false;
        };
        let (host, suffix) = rest.split_at(close);
        return valid_serialized_ipv6(host) && valid_serialized_port_suffix(suffix.get(1..).unwrap_or_default(), scheme);
    }

    let mut colon = None;
    for (index, &byte) in authority.iter().enumerate() {
        if BYTE_CLASS[usize::from(byte)] & CLASS_AUTHORITY != 0 {
            continue;
        }
        if byte != b':' || colon.is_some() {
            return false;
        }
        colon = Some(index);
    }

    let (host, port) = match colon {
        Some(index) => (authority.get(..index).unwrap_or_default(), authority.get(index.saturating_add(1)..)),
        None => (authority, None),
    };
    if host.is_empty() || port.is_some_and(|port| !valid_serialized_port(port, scheme)) {
        return false;
    }
    valid_serialized_host(host)
}

fn valid_serialized_port_suffix(suffix: &[u8], scheme: &[u8]) -> bool {
    match suffix.split_first() {
        None => true,
        Some((&b':', port)) => valid_serialized_port(port, scheme),
        Some(_) => false,
    }
}

fn valid_serialized_port(port: &[u8], scheme: &[u8]) -> bool {
    if port.is_empty() || port.len() > 5 || (port.len() > 1 && port.first() == Some(&b'0')) {
        return false;
    }
    let mut value = 0_u32;
    for &byte in port {
        if BYTE_CLASS[usize::from(byte)] & CLASS_DIGIT == 0 {
            return false;
        }
        value = value * 10 + u32::from(byte.wrapping_sub(b'0'));
    }
    if value > u32::from(u16::MAX) {
        return false;
    }
    let default_port = if scheme == b"ftp" {
        21
    } else if scheme == b"http" || scheme == b"ws" {
        80
    } else if scheme == b"https" || scheme == b"wss" {
        443
    } else {
        return true;
    };
    value != default_port
}

fn valid_serialized_host(host: &[u8]) -> bool {
    let host = match host.split_last() {
        Some((&b'.', head)) => head,
        _ => host,
    };
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    let mut label_count = 0_usize;
    let mut all_numeric = true;
    let mut valid_ipv4 = true;
    let mut valid_domain = true;
    let mut start = 0_usize;
    let mut common = u8::MAX;
    for (index, &byte) in host.iter().enumerate() {
        if byte != b'.' {
            common &= BYTE_CLASS[usize::from(byte)];
            continue;
        }
        let label = host.get(start..index).unwrap_or_default();
        let (numeric, ipv4, domain) = label_classes(label, common);
        all_numeric &= numeric;
        valid_ipv4 &= ipv4;
        valid_domain &= domain;
        label_count = label_count.saturating_add(1);
        start = index.saturating_add(1);
        common = u8::MAX;
    }
    let label = host.get(start..).unwrap_or_default();
    let (numeric, ipv4, domain) = label_classes(label, common);
    all_numeric &= numeric;
    valid_ipv4 &= ipv4;
    valid_domain &= domain;
    label_count = label_count.saturating_add(1);

    if all_numeric {
        label_count == 4 && valid_ipv4
    } else {
        valid_domain
    }
}

/// Classifies one host label as numeric, a valid IPv4 octet, and a valid domain label.
///
/// `common` is the intersection of the byte classes of every byte in `label`.
fn label_classes(label: &[u8], common: u8) -> (bool, bool, bool) {
    let (Some(&first), Some(&last)) = (label.first(), label.last()) else {
        return (false, false, false);
    };

    let numeric = common & CLASS_DIGIT != 0;
    let ipv4 = numeric && label.len() <= 3 && (label.len() == 1 || first != b'0') && {
        let mut value = 0_u32;
        for &byte in label {
            value = value * 10 + u32::from(byte.wrapping_sub(b'0'));
        }
        value <= 255
    };
    let domain = common & CLASS_DOMAIN != 0
        && label.len() <= 63
        && BYTE_CLASS[usize::from(first)] & CLASS_LABEL_EDGE != 0
        && BYTE_CLASS[usize::from(last)] & CLASS_LABEL_EDGE != 0;

    (numeric, ipv4, domain)
}

fn valid_serialized_ipv6(address: &[u8]) -> bool {
    if address.is_empty()
        || address
            .iter()
            .any(|byte| BYTE_CLASS[usize::from(*byte)] & CLASS_HEX == 0 && *byte != b':')
    {
        return false;
    }
    let text = str::from_utf8(address).expect("serialized IPv6 bytes are ASCII");
    let Ok(parsed) = Ipv6Addr::from_str(text) else {
        return false;
    };
    serialized_ipv6(parsed).as_bytes() == address
}

fn serialized_ipv6(address: Ipv6Addr) -> String {
    let segments = address.segments();
    let mut longest_start = None;
    let mut longest_len = 1_usize;
    let mut index = 0_usize;
    while index < segments.len() {
        if segments[index] != 0 {
            index += 1;
            continue;
        }
        let start = index;
        while index < segments.len() && segments[index] == 0 {
            index += 1;
        }
        let len = index - start;
        if len > longest_len {
            longest_start = Some(start);
            longest_len = len;
        }
    }

    let mut serialized = String::with_capacity(39);
    let mut index = 0_usize;
    while index < segments.len() {
        if longest_start == Some(index) {
            serialized.push_str("::");
            index += longest_len;
            continue;
        }
        if !serialized.is_empty() && !serialized.ends_with(':') {
            serialized.push(':');
        }
        write!(serialized, "{:x}", segments[index]).expect("writing an IPv6 segment to a string cannot fail");
        index += 1;
    }
    serialized
}

#[cfg(test)]
fn valid_h16_sequence(sequence: &[u8]) -> Option<usize> {
    if sequence.is_empty() {
        return Some(0);
    }
    let mut count = 0_usize;
    for block in sequence.split(|byte| *byte == b':') {
        if block.is_empty()
            || block.len() > 4
            || (block.len() > 1 && block.first() == Some(&b'0'))
            || !block.iter().all(|byte| BYTE_CLASS[usize::from(*byte)] & CLASS_HEX != 0)
        {
            return None;
        }
        count = count.checked_add(1)?;
    }
    Some(count)
}

/// Borrows `range` of a field value made only of visible ASCII.
///
/// A serialized origin never leaves that alphabet, so the branch-free ASCII
/// reduction settles the conversion far more cheaply than the general UTF-8
/// validator, which is left to handle everything else.
fn visible_ascii_str(value: FieldValueRef<'_>, range: Range<usize>) -> Option<&str> {
    http_headers_simd::ascii_str(value.as_bytes().get(range)?)
}

fn semantic_str<'a>(name: &'static FieldName, bytes: &'a [u8], range: Range<usize>) -> Result<&'a str, DecodeError> {
    let bytes = bytes.get(range).ok_or_else(|| invalid_syntax(name))?;
    str::from_utf8(bytes).map_err(|_invalid| DecodeError::new(name, DecodeErrorKind::InvalidUtf8))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{
        AccessControlAllowOrigin, AccessControlAllowOriginOwned, OriginKind, ParsedOrigin, common_http_origin, label_classes, semantic_str,
        split_scheme, valid_domain_label, valid_h16_sequence, valid_serialized_authority, valid_serialized_host, valid_serialized_ipv6,
        valid_serialized_origin, valid_serialized_port, valid_serialized_port_suffix, visible_ascii_str,
    };
    use crate::headers::cors::test_support::TestMap;
    use crate::{DecodeErrorKind, FieldName, FieldValue};

    #[test]
    fn constructors_accessors_display_and_header_paths_preserve_wire_values() {
        let wildcard = AccessControlAllowOriginOwned::wildcard();
        assert!(wildcard.is_wildcard());
        assert!(!wildcard.is_null());
        assert_eq!(wildcard.origin().expect("origin accessor"), None);
        assert_eq!(wildcard.as_str().expect("wildcard text"), "*");

        let null = AccessControlAllowOriginOwned::null();
        assert!(!null.is_wildcard());
        assert!(null.is_null());
        assert_eq!(null.origin().expect("origin accessor"), None);
        assert_eq!(null.to_string(), "null");

        let origin = AccessControlAllowOriginOwned::from_origin("https://example.com").expect("origin");
        assert_eq!(origin.origin().expect("serialized origin"), Some("https://example.com"));
        assert_eq!(origin.as_str().expect("origin text"), "https://example.com");
        assert_eq!(origin.to_string(), "https://example.com");
        assert!(format!("{origin:?}").contains("Origin"));
        assert_eq!(origin.into_field_value(), "https://example.com");

        AccessControlAllowOriginOwned::from_origin("*").expect_err("wildcard is not an origin");
        AccessControlAllowOriginOwned::from_origin("null").expect_err("null is not an origin");
        let framed_null = AccessControlAllowOriginOwned::try_from(String::from(" null ")).expect("owned null");
        assert!(framed_null.is_null());
        assert_eq!(framed_null.as_str().expect("null text"), "null");
        assert_eq!(
            AccessControlAllowOriginOwned::try_from(FieldValue::from_static(" https://example.com "))
                .expect("field origin")
                .origin()
                .expect("origin accessor"),
            Some("https://example.com")
        );

        let source = TestMap::new(
            &FieldName::AccessControlAllowOrigin,
            vec![FieldValue::from_static(" https://example.com ")],
        );
        let view = AccessControlAllowOrigin::view(&source)
            .expect("valid origin view")
            .expect("present");
        assert!(!view.is_wildcard());
        assert!(!view.is_null());
        assert_eq!(view.origin(), Some("https://example.com"));
        assert_eq!(view.as_str(), "https://example.com");
        assert_eq!(view.as_field_value(), " https://example.com ");
        assert!(format!("{view:?}").contains("Origin"));

        let owned = AccessControlAllowOrigin::owned(&source)
            .expect("valid owned origin")
            .expect("present");
        let mut sink = TestMap::new(&FieldName::Accept, Vec::new());
        AccessControlAllowOrigin::insert(&mut sink, owned).expect("insert origin");
        assert_eq!(sink.name, &FieldName::AccessControlAllowOrigin);
        assert_eq!(sink.values, source.values);

        let wildcard_source = TestMap::new(&FieldName::AccessControlAllowOrigin, vec![FieldValue::from_static("*")]);
        assert!(
            AccessControlAllowOrigin::view(&wildcard_source)
                .expect("wildcard view")
                .expect("present")
                .is_wildcard()
        );
        assert_eq!(
            AccessControlAllowOrigin::view(&wildcard_source)
                .expect("wildcard view")
                .expect("present")
                .origin(),
            None
        );
        let null_source = TestMap::new(&FieldName::AccessControlAllowOrigin, vec![FieldValue::from_static("null")]);
        assert!(
            AccessControlAllowOrigin::view(&null_source)
                .expect("null view")
                .expect("present")
                .is_null()
        );

        let absent = TestMap::new(&FieldName::Accept, Vec::new());
        assert!(AccessControlAllowOrigin::view(&absent).expect("absent").is_none());
        assert!(AccessControlAllowOrigin::owned(&absent).expect("absent").is_none());
    }

    #[test]
    fn serialized_origin_validation_accepts_canonical_host_forms_and_ports() {
        for origin in [
            "http://example.com",
            "https://example.com.",
            "https://sub-domain.example",
            "https://127.0.0.1",
            "https://[2001:db8::1]",
            "https://[::1]:8443",
            "https://[2001::1:0:0:1:1]",
            "https://[2001:db8:0:1:2:3:4:5]",
            "https://[::ffff:c000:280]",
            "ftp://example.com:22",
            "ws://example.com:8080",
            "wss://example.com:8443",
        ] {
            assert!(valid_serialized_origin(origin.as_bytes()), "{origin}");
            assert!(AccessControlAllowOriginOwned::from_origin(origin).is_ok(), "{origin}");
        }

        // The `http`/`https` recognizer shortcuts the general authority rules,
        // so every answer it commits to must match what those rules decide.
        let alphabet = b"a0-.:[]_A%";
        for first in alphabet {
            for second in alphabet {
                for third in alphabet {
                    let mut origin = b"https://".to_vec();
                    origin.extend_from_slice(&[*first, *second, *third]);
                    let Some(recognized) = common_http_origin(&origin) else {
                        continue;
                    };
                    let general = split_scheme(&origin).is_some_and(|(scheme, authority)| valid_serialized_authority(authority, scheme));
                    assert_eq!(
                        recognized,
                        general,
                        "{:?}",
                        str::from_utf8(&origin).expect("the alphabet is ASCII only")
                    );
                }
            }
        }

        assert_eq!(common_http_origin(b"https://example.com"), Some(true));
        assert_eq!(common_http_origin(b"https://127.0.0.1"), None);
        assert_eq!(common_http_origin(b"custom://example.com"), None);
        assert_eq!(
            split_scheme(b"custom+v1://host"),
            Some((b"custom+v1".as_slice(), b"host".as_slice()))
        );
        assert!(valid_domain_label(b"example"));
        assert!(!valid_domain_label(b""));
        assert!(valid_serialized_authority(b"example.com", b"https"));
        assert!(valid_serialized_port_suffix(b"", b"https"));
        assert!(valid_serialized_port_suffix(b":8443", b"https"));
        assert!(valid_serialized_port(b"8443", b"https"));
        assert!(valid_serialized_port(b"80", b"custom"));
        assert!(valid_serialized_host(b"127.0.0.1"));
        assert!(valid_serialized_ipv6(b"2001:db8::1"));
        assert!(valid_serialized_ipv6(b"2001:db8:1:2:3:4:5:6"));
        assert_eq!(valid_h16_sequence(b"2001:db8"), Some(2));
        assert_eq!(valid_h16_sequence(b""), Some(0));

        let classes = label_classes(b"255", super::CLASS_DIGIT | super::CLASS_DOMAIN);
        assert_eq!(classes, (true, true, true));
    }

    #[test]
    fn serialized_origin_validation_rejects_noncanonical_and_malformed_forms() {
        for origin in [
            "",
            "*",
            "null",
            "HTTPS://example.com",
            "1http://example.com",
            "http:/example.com",
            "http://",
            "http://example.com/path",
            "http://example.com?query",
            "http://user@example.com",
            "http://-example.com",
            "http://example-.com",
            "http://example..com",
            "http://999.1.1.1",
            "http://01.2.3.4",
            "http://1.2.3",
            "http://example.com:80",
            "https://example.com:443",
            "ftp://example.com:21",
            "custom+v1://host:80",
            "https://example.com:",
            "https://example.com:0001",
            "https://example.com:65536",
            "https://example.com:port",
            "https://example.com:1:2",
            "https://[2001:0db8::1]",
            "https://[2001:db8:0:0:1:2:3:4]",
            "https://[2001:0:0:1::1:1]",
            "https://[2001:db8:0::1]",
            "https://[2001::db8::1]",
            "https://[2001:db8:1:2:3:4:5:6:7]",
            "https://[2001:db8::1",
            "https://[2001:db8::1]suffix",
        ] {
            assert!(!valid_serialized_origin(origin.as_bytes()), "{origin}");
        }

        assert_eq!(common_http_origin(b"https://"), Some(false));
        assert_eq!(common_http_origin(b"https://bad_host"), Some(false));
        assert_eq!(split_scheme(b"Nope://host"), None);
        assert_eq!(split_scheme(b"http:/host"), None);
        assert!(!valid_serialized_authority(b"", b"https"));
        assert!(!valid_serialized_authority(b"host/path", b"https"));
        assert!(!valid_serialized_authority(b"[::1]/", b"https"));
        assert!(!valid_serialized_port_suffix(b"suffix", b"https"));
        assert!(!valid_serialized_port(b"", b"https"));
        assert!(!valid_serialized_port(b"01", b"https"));
        assert!(!valid_serialized_port(b"65536", b"https"));
        assert!(!valid_serialized_port(b"x", b"https"));
        assert!(!valid_serialized_host(b""));
        assert!(valid_serialized_host(b"example.com."));
        assert!(!valid_serialized_host(b"1.2.3"));
        assert!(!valid_serialized_ipv6(b""));
        assert!(!valid_serialized_ipv6(b"xyz"));
        assert!(!valid_serialized_ipv6(b"1::2::3"));
        assert_eq!(valid_h16_sequence(b"0001"), None);
        assert_eq!(valid_h16_sequence(b"12345"), None);
        assert_eq!(valid_h16_sequence(b"1::2"), None);
        assert!(!valid_serialized_ipv6(b"2001::0db8"));
        assert_eq!(label_classes(b"", super::CLASS_DIGIT | super::CLASS_DOMAIN), (false, false, false));

        let error = AccessControlAllowOriginOwned::try_from(String::from("bad\norigin")).expect_err("invalid field bytes");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
        assert_eq!(
            AccessControlAllowOriginOwned::try_from("bad\norigin")
                .expect_err("invalid borrowed field bytes")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            AccessControlAllowOriginOwned::try_from("not-an-origin")
                .expect_err("invalid visible origin")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        let duplicate = TestMap::new(
            &FieldName::AccessControlAllowOrigin,
            vec![
                FieldValue::from_static("https://one.example"),
                FieldValue::from_static("https://two.example"),
            ],
        );
        assert_eq!(
            AccessControlAllowOrigin::view(&duplicate).expect_err("singleton header").kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
    }

    #[test]
    fn semantic_accessors_revalidate_private_metadata() {
        let out_of_bounds = AccessControlAllowOriginOwned {
            value: FieldValue::from_static("x"),
            parsed: ParsedOrigin {
                start: 2,
                end: 3,
                kind: OriginKind::Origin,
            },
        };
        assert_eq!(
            out_of_bounds.origin().expect_err("metadata range").kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let invalid_utf8 = FieldValue::from_bytes([0xff]).expect("non-ASCII field value byte is permitted");
        assert_eq!(
            super::parse_allow_origin_value(invalid_utf8.as_field_value_ref())
                .expect_err("invalid UTF-8 origin")
                .kind(),
            DecodeErrorKind::InvalidUtf8
        );
        let invalid_utf8 = AccessControlAllowOriginOwned {
            value: invalid_utf8,
            parsed: ParsedOrigin {
                start: 0,
                end: 1,
                kind: OriginKind::Origin,
            },
        };
        assert_eq!(
            invalid_utf8.as_str().expect_err("invalid UTF-8 metadata").kind(),
            DecodeErrorKind::InvalidUtf8
        );

        let value = FieldValue::from_static("visible");
        assert_eq!(visible_ascii_str(value.as_field_value_ref(), 0..7), Some("visible"));
        assert_eq!(visible_ascii_str(value.as_field_value_ref(), 9..10), None);
        assert_eq!(
            semantic_str(&FieldName::AccessControlAllowOrigin, b"value", 0..5).expect("semantic text"),
            "value"
        );
    }
}
