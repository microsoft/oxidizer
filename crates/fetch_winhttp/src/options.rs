// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::ptr::NonNull;
use std::time::Duration;

use http::{HeaderMap, Method, Version};
use thread_aware::ThreadAware;
use widestring::{U16CStr, U16CString};

use crate::bindings::{Bindings as _, Facade};
use crate::error::WinHttpError;
use crate::handle::RawHandle;
use crate::tls::WinHttpTlsConfig;

pub(crate) const WINHTTP_FLAG_ASYNC: u32 = 0x1000_0000;
pub(crate) const WINHTTP_FLAG_AUTOMATIC_CHUNKING: u32 = 0x0000_0200;
pub(crate) const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;

pub(crate) const WINHTTP_OPTION_CONTEXT_VALUE: u32 = 45;
pub(crate) const WINHTTP_OPTION_DECOMPRESSION: u32 = 118;
pub(crate) const WINHTTP_OPTION_DISABLE_FEATURE: u32 = 63;
pub(crate) const WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL: u32 = 133;
pub(crate) const WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED: u32 = 145;
pub(crate) const WINHTTP_OPTION_HTTP_PROTOCOL_USED: u32 = 134;
pub(crate) const WINHTTP_OPTION_HTTP2_KEEPALIVE: u32 = 164;
pub(crate) const WINHTTP_OPTION_HTTP3_KEEPALIVE: u32 = 188;
pub(crate) const WINHTTP_OPTION_REDIRECT_POLICY: u32 = 88;
pub(crate) const WINHTTP_OPTION_SECURITY_FLAGS: u32 = 31;

pub(crate) const WINHTTP_QUERY_RAW_HEADERS_CRLF: u32 = 22;
pub(crate) const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
pub(crate) const WINHTTP_QUERY_VERSION: u32 = 18;
const WINHTTP_QUERY_FLAG_TRAILERS: u32 = 0x0200_0000;
pub(crate) const WINHTTP_QUERY_FLAG_WIRE_ENCODING: u32 = 0x0100_0000;
const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;

const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_WINHTTP_HEADER_NOT_FOUND: u32 = 12150;
const WINHTTP_PROTOCOL_FLAG_HTTP2: u32 = 1;
const WINHTTP_PROTOCOL_FLAG_HTTP3: u32 = 2;
const WINHTTP_DISABLE_COOKIES: u32 = 1;
const WINHTTP_DISABLE_AUTHENTICATION: u32 = 4;
const WINHTTP_DECOMPRESSION_FLAG_GZIP: u32 = 1;
const WINHTTP_DECOMPRESSION_FLAG_DEFLATE: u32 = 2;
const SECURITY_FLAG_IGNORE_UNKNOWN_CA: u32 = 0x0100;
const SECURITY_FLAG_IGNORE_CERT_WRONG_USAGE: u32 = 0x0200;
const SECURITY_FLAG_IGNORE_CERT_CN_INVALID: u32 = 0x1000;
const SECURITY_FLAG_IGNORE_CERT_DATE_INVALID: u32 = 0x2000;
const HTTP2_KEEP_ALIVE_MINIMUM_MS: u32 = 5_000;
pub(crate) const WINHTTP_OPTION_REDIRECT_POLICY_NEVER: u32 = 0;
const UNLIMITED_TIMEOUT: i32 = -1;
const DWORD_BYTES: u32 = 4;

/// WinHTTP-specific transport options.
///
/// The default configuration leaves the native DNS resolution timeout
/// unlimited.
#[derive(Clone, Debug, Default, ThreadAware)]
#[non_exhaustive]
pub struct WinHttpOptions {
    resolve_timeout: Option<Duration>,
}

impl WinHttpOptions {
    /// Starts building WinHTTP-specific transport options.
    #[must_use]
    pub fn builder() -> WinHttpOptionsBuilder {
        WinHttpOptionsBuilder { options: Self::default() }
    }

    pub(crate) fn resolve_timeout(&self) -> Option<Duration> {
        self.resolve_timeout
    }
}

/// Builds [`WinHttpOptions`].
#[derive(Clone, Debug, ThreadAware)]
#[non_exhaustive]
pub struct WinHttpOptionsBuilder {
    options: WinHttpOptions,
}

impl WinHttpOptionsBuilder {
    /// Sets the native DNS resolution timeout.
    ///
    /// This timeout covers DNS resolution only. Other request deadlines are
    /// configured through the generic `fetch` client options.
    ///
    /// Configured values below one millisecond are rounded up to one
    /// millisecond. Values beyond `WinHTTP`'s finite range are clamped.
    #[must_use]
    pub fn resolve_timeout(mut self, timeout: Duration) -> Self {
        self.options.resolve_timeout = Some(timeout);
        self
    }

    /// Builds the WinHTTP-specific transport options.
    #[must_use]
    pub fn build(self) -> WinHttpOptions {
        self.options
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ConversionError {
    EmbeddedNul(&'static str),
    HeaderBufferTooSmall,
    HeaderByteLengthIsOdd(u32),
    InvalidHeaderUtf16,
    InvalidProtocolMask(u32),
    InvalidProtocolVersion(String),
    RequestHeadersTooLarge,
    UnsupportedHttpVersion(Version),
}

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmbeddedNul(field) => write!(f, "{field} contains a NUL character"),
            Self::HeaderBufferTooSmall => f.write_str("the WinHTTP header buffer is smaller than the returned length"),
            Self::HeaderByteLengthIsOdd(length) => {
                write!(f, "WinHTTP returned an odd UTF-16 byte length: {length}")
            }
            Self::InvalidHeaderUtf16 => f.write_str("WinHTTP returned invalid UTF-16 header data"),
            Self::InvalidProtocolMask(mask) => {
                write!(f, "WinHTTP returned an invalid HTTP protocol mask: {mask}")
            }
            Self::InvalidProtocolVersion(version) => {
                write!(f, "WinHTTP returned an unsupported HTTP version: {version}")
            }
            Self::RequestHeadersTooLarge => f.write_str("the request headers are too large to materialize"),
            Self::UnsupportedHttpVersion(version) => {
                write!(f, "WinHTTP does not support requested HTTP version {version:?}")
            }
        }
    }
}

impl std::error::Error for ConversionError {}

#[derive(Debug)]
pub(crate) enum QueryError {
    Conversion(ConversionError),
    WinHttp(WinHttpError),
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conversion(error) => error.fmt(f),
            Self::WinHttp(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for QueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Conversion(error) => Some(error),
            Self::WinHttp(error) => Some(error),
        }
    }
}

impl From<ConversionError> for QueryError {
    fn from(error: ConversionError) -> Self {
        Self::Conversion(error)
    }
}

impl From<WinHttpError> for QueryError {
    fn from(error: WinHttpError) -> Self {
        Self::WinHttp(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProtocolOptions {
    advanced_mask: u32,
    required: bool,
}

impl ProtocolOptions {
    pub(crate) const fn advanced_mask(self) -> u32 {
        self.advanced_mask
    }

    pub(crate) const fn required(self) -> bool {
        self.required
    }
}

pub(crate) fn timeout_millis(timeout: Option<Duration>) -> i32 {
    match timeout {
        None => UNLIMITED_TIMEOUT,
        Some(duration) => i32::try_from(ceil_millis(duration).clamp(1, u128::from(i32::MAX.cast_unsigned())))
            .expect("value is clamped to i32::MAX before conversion"),
    }
}

pub(crate) fn dword_millis(duration: Duration) -> u32 {
    u32::try_from(ceil_millis(duration).min(u128::from(u32::MAX))).expect("value is clamped to u32::MAX before conversion")
}

pub(crate) fn http2_keep_alive_millis(duration: Duration) -> u32 {
    dword_millis(duration).max(HTTP2_KEEP_ALIVE_MINIMUM_MS)
}

pub(crate) fn http3_keep_alive_millis(duration: Duration) -> u32 {
    dword_millis(duration).max(1)
}

fn ceil_millis(duration: Duration) -> u128 {
    let whole = duration.as_millis();

    if duration.subsec_nanos().is_multiple_of(1_000_000) {
        whole
    } else {
        whole + 1
    }
}

pub(crate) fn method_to_utf16(method: &Method) -> Result<U16CString, ConversionError> {
    string_to_utf16(method.as_str(), "HTTP method")
}

pub(crate) fn host_to_utf16(host: &str) -> Result<U16CString, ConversionError> {
    string_to_utf16(host, "host")
}

pub(crate) fn path_to_utf16(path: &str) -> Result<U16CString, ConversionError> {
    if path.starts_with('?') {
        let mut units = Vec::with_capacity(path.len().saturating_add(1));
        units.push(u16::from(b'/'));
        units.extend(path.encode_utf16());
        return U16CString::from_vec(units).map_err(|_contains_nul| ConversionError::EmbeddedNul("request path"));
    }

    string_to_utf16(path, "request path")
}

pub(crate) fn headers_to_utf16(headers: &HeaderMap) -> Result<U16CString, ConversionError> {
    let unit_count = headers.iter().try_fold(0_usize, |unit_count, (name, value)| {
        unit_count
            .checked_add(name.as_str().len())
            .and_then(|unit_count| unit_count.checked_add(value.as_bytes().len()))
            .and_then(|unit_count| unit_count.checked_add(4))
    });
    let unit_count = unit_count.ok_or(ConversionError::RequestHeadersTooLarge)?;
    validate_request_header_unit_count(unit_count)?;
    let capacity = unit_count.checked_add(1).ok_or(ConversionError::RequestHeadersTooLarge)?;
    let mut units = Vec::with_capacity(capacity);

    for (name, value) in headers {
        units.extend(name.as_str().bytes().map(u16::from));
        units.extend([u16::from(b':'), u16::from(b' ')]);
        units.extend(value.as_bytes().iter().copied().map(u16::from));
        units.extend([u16::from(b'\r'), u16::from(b'\n')]);
    }

    U16CString::from_vec(units).map_err(|_contains_nul| ConversionError::EmbeddedNul("request headers"))
}

fn validate_request_header_unit_count(unit_count: usize) -> Result<(), ConversionError> {
    u32::try_from(unit_count)
        .map(|_unit_count| ())
        .map_err(|_too_large| ConversionError::RequestHeadersTooLarge)
}

fn string_to_utf16(value: &str, field: &'static str) -> Result<U16CString, ConversionError> {
    U16CString::from_str(value).map_err(|_contains_nul| ConversionError::EmbeddedNul(field))
}

pub(crate) fn header_units_without_nul(headers: &U16CStr) -> &[u16] {
    headers.as_slice()
}

pub(crate) const fn dword_bytes(value: u32) -> [u8; size_of::<u32>()] {
    value.to_ne_bytes()
}

pub(crate) const fn context_bytes(value: usize) -> [u8; size_of::<usize>()] {
    value.to_ne_bytes()
}

pub(crate) const fn disable_feature_mask() -> u32 {
    WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION
}

pub(crate) const fn decompression_mask() -> u32 {
    WINHTTP_DECOMPRESSION_FLAG_GZIP | WINHTTP_DECOMPRESSION_FLAG_DEFLATE
}

pub(crate) const fn request_open_flags(secure: bool, automatic_chunking: bool) -> u32 {
    let mut flags = 0;

    if secure {
        flags |= WINHTTP_FLAG_SECURE;
    }
    if automatic_chunking {
        flags |= WINHTTP_FLAG_AUTOMATIC_CHUNKING;
    }

    flags
}

pub(crate) fn security_flags(config: &WinHttpTlsConfig) -> u32 {
    let mut flags = 0;

    if config.accepts_invalid_certs() {
        flags |= SECURITY_FLAG_IGNORE_UNKNOWN_CA | SECURITY_FLAG_IGNORE_CERT_WRONG_USAGE | SECURITY_FLAG_IGNORE_CERT_DATE_INVALID;
    }
    if config.accepts_invalid_hostnames() {
        flags |= SECURITY_FLAG_IGNORE_CERT_CN_INVALID;
    }

    flags
}

pub(crate) fn protocol_options(versions: &[Version]) -> Result<ProtocolOptions, ConversionError> {
    let versions = if versions.is_empty() {
        &[Version::HTTP_11, Version::HTTP_2][..]
    } else {
        versions
    };
    let mut advanced_mask = 0;
    let mut allows_http11 = false;

    for version in versions {
        match *version {
            Version::HTTP_11 => allows_http11 = true,
            Version::HTTP_2 => advanced_mask |= WINHTTP_PROTOCOL_FLAG_HTTP2,
            Version::HTTP_3 => advanced_mask |= WINHTTP_PROTOCOL_FLAG_HTTP3,
            unsupported => {
                return Err(ConversionError::UnsupportedHttpVersion(unsupported));
            }
        }
    }

    Ok(ProtocolOptions {
        advanced_mask,
        required: !allows_http11,
    })
}

pub(crate) fn parse_protocol_used(protocol_mask: u32, legacy_version: Option<&str>) -> Result<Version, ConversionError> {
    match protocol_mask {
        WINHTTP_PROTOCOL_FLAG_HTTP2 => Ok(Version::HTTP_2),
        WINHTTP_PROTOCOL_FLAG_HTTP3 => Ok(Version::HTTP_3),
        0 => parse_legacy_version(
            legacy_version.ok_or_else(|| ConversionError::InvalidProtocolVersion("missing WINHTTP_QUERY_VERSION value".to_owned()))?,
        ),
        invalid => Err(ConversionError::InvalidProtocolMask(invalid)),
    }
}

fn parse_legacy_version(version: &str) -> Result<Version, ConversionError> {
    match version.trim_end_matches('\0') {
        "HTTP/0.9" => Ok(Version::HTTP_09),
        "HTTP/1.0" => Ok(Version::HTTP_10),
        "HTTP/1.1" => Ok(Version::HTTP_11),
        invalid => Err(ConversionError::InvalidProtocolVersion(invalid.to_owned())),
    }
}

pub(crate) fn header_buffer_units(byte_len: u32) -> Result<usize, ConversionError> {
    if !byte_len.is_multiple_of(2) {
        return Err(ConversionError::HeaderByteLengthIsOdd(byte_len));
    }

    usize::try_from(byte_len / 2).map_err(|_too_large| ConversionError::HeaderBufferTooSmall)
}

pub(crate) fn parse_header_buffer(buffer: &[u16], returned_bytes: u32) -> Result<String, ConversionError> {
    let units = header_buffer_units(returned_bytes)?;
    let content = buffer.get(..units).ok_or(ConversionError::HeaderBufferTooSmall)?;

    if content.contains(&0) {
        return Err(ConversionError::InvalidHeaderUtf16);
    }

    String::from_utf16(content).map_err(|_invalid_utf16| ConversionError::InvalidHeaderUtf16)
}

pub(crate) fn query_status_code(bindings: &Facade, request: RawHandle) -> Result<u32, QueryError> {
    let mut status_code = 0_u32;
    let mut byte_len = DWORD_BYTES;
    let buffer = NonNull::from(&mut status_code).cast();

    // SAFETY: status_code is a writable DWORD and byte_len describes its exact
    // capacity for the duration of the synchronous query.
    unsafe {
        bindings.query_headers(
            request,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            Some(buffer),
            &mut byte_len,
        )
    }?;

    if byte_len != DWORD_BYTES {
        return Err(ConversionError::HeaderBufferTooSmall.into());
    }

    Ok(status_code)
}

pub(crate) fn query_raw_headers(bindings: &Facade, request: RawHandle) -> Result<Vec<u8>, QueryError> {
    query_header_bytes(bindings, request, WINHTTP_QUERY_RAW_HEADERS_CRLF | WINHTTP_QUERY_FLAG_WIRE_ENCODING)
}

pub(crate) fn query_raw_trailers(bindings: &Facade, request: RawHandle) -> Result<Option<Vec<u8>>, QueryError> {
    match query_header_bytes(
        bindings,
        request,
        WINHTTP_QUERY_RAW_HEADERS_CRLF | WINHTTP_QUERY_FLAG_TRAILERS | WINHTTP_QUERY_FLAG_WIRE_ENCODING,
    ) {
        Err(QueryError::WinHttp(error)) if error.code() == ERROR_WINHTTP_HEADER_NOT_FOUND => Ok(None),
        result => result.map(Some),
    }
}

pub(crate) fn query_protocol_used(bindings: &Facade, request: RawHandle) -> Result<Version, QueryError> {
    let mut protocol_mask = 0_u32;
    let mut byte_len = DWORD_BYTES;
    let buffer = NonNull::from(&mut protocol_mask).cast();

    // SAFETY: protocol_mask is a writable DWORD and byte_len describes its
    // exact capacity for the duration of the synchronous query.
    unsafe { bindings.query_option(request, WINHTTP_OPTION_HTTP_PROTOCOL_USED, Some(buffer), &mut byte_len) }?;

    if byte_len != DWORD_BYTES {
        return Err(ConversionError::HeaderBufferTooSmall.into());
    }

    if protocol_mask == 0 {
        let version = query_header_string(bindings, request, WINHTTP_QUERY_VERSION)?;
        Ok(parse_protocol_used(0, Some(&version))?)
    } else {
        Ok(parse_protocol_used(protocol_mask, None)?)
    }
}

fn query_header_string(bindings: &Facade, request: RawHandle, info_level: u32) -> Result<String, QueryError> {
    let buffer = query_header_units(bindings, request, info_level)?;

    String::from_utf16(&buffer).map_err(|_invalid_utf16| ConversionError::InvalidHeaderUtf16.into())
}

fn query_header_bytes(bindings: &Facade, request: RawHandle, info_level: u32) -> Result<Vec<u8>, QueryError> {
    let mut required_bytes = 0_u32;

    // SAFETY: a null buffer with zero capacity is the documented sizing query.
    match unsafe { bindings.query_headers(request, info_level, None, &mut required_bytes) } {
        Err(error) if error.code() == ERROR_INSUFFICIENT_BUFFER => {}
        Err(error) => return Err(error.into()),
        Ok(()) if required_bytes == 0 => return Ok(Vec::new()),
        Ok(()) => {}
    }

    let capacity = usize::try_from(required_bytes).map_err(|_too_large| ConversionError::HeaderBufferTooSmall)?;
    let mut buffer = vec![0_u8; capacity];
    let output = NonNull::new(buffer.as_mut_ptr()).ok_or(ConversionError::HeaderBufferTooSmall)?;
    let mut returned_bytes = required_bytes;

    // SAFETY: output points to a writable buffer of required_bytes and remains
    // valid for the duration of the synchronous query.
    unsafe { bindings.query_headers(request, info_level, Some(output), &mut returned_bytes) }?;

    let returned_bytes = usize::try_from(returned_bytes).map_err(|_too_large| ConversionError::HeaderBufferTooSmall)?;
    if returned_bytes > buffer.len() {
        return Err(ConversionError::HeaderBufferTooSmall.into());
    }
    buffer.truncate(returned_bytes);

    Ok(buffer)
}

fn query_header_units(bindings: &Facade, request: RawHandle, info_level: u32) -> Result<Vec<u16>, QueryError> {
    let mut required_bytes = 0_u32;

    // SAFETY: a null buffer with zero capacity is the documented sizing query.
    match unsafe { bindings.query_headers(request, info_level, None, &mut required_bytes) } {
        Err(error) if error.code() == ERROR_INSUFFICIENT_BUFFER => {}
        Err(error) => return Err(error.into()),
        Ok(()) if required_bytes == 0 => return Ok(Vec::new()),
        Ok(()) => {}
    }

    let units = header_buffer_units(required_bytes)?;
    let mut buffer = vec![0_u16; units];
    let output = NonNull::new(buffer.as_mut_ptr().cast::<u8>()).ok_or(ConversionError::HeaderBufferTooSmall)?;
    let mut returned_bytes = required_bytes;

    // SAFETY: output points to a writable buffer of required_bytes and remains
    // valid for the duration of the synchronous query.
    unsafe { bindings.query_headers(request, info_level, Some(output), &mut returned_bytes) }?;

    let returned_units = header_buffer_units(returned_bytes)?;
    if returned_units > buffer.len() {
        return Err(ConversionError::HeaderBufferTooSmall.into());
    }
    buffer.truncate(returned_units);

    if buffer.contains(&0) {
        return Err(ConversionError::InvalidHeaderUtf16.into());
    }

    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::fmt::Debug;
    use std::sync::Arc;
    use std::time::Duration;

    use http::{HeaderMap, HeaderValue, Method, Version};
    use mockall::Sequence;
    use static_assertions::assert_impl_all;
    use thread_aware::ThreadAware;

    use super::{
        ConversionError, WINHTTP_FLAG_ASYNC, WINHTTP_FLAG_AUTOMATIC_CHUNKING, WINHTTP_FLAG_SECURE, WINHTTP_OPTION_CONTEXT_VALUE,
        WINHTTP_OPTION_DECOMPRESSION, WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL,
        WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED, WINHTTP_OPTION_HTTP_PROTOCOL_USED, WINHTTP_OPTION_HTTP2_KEEPALIVE,
        WINHTTP_OPTION_HTTP3_KEEPALIVE, WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER,
        WINHTTP_OPTION_SECURITY_FLAGS, WINHTTP_QUERY_FLAG_WIRE_ENCODING, WINHTTP_QUERY_STATUS_CODE, WINHTTP_QUERY_VERSION, WinHttpOptions,
        WinHttpOptionsBuilder, context_bytes, decompression_mask, disable_feature_mask, dword_bytes, dword_millis, header_buffer_units,
        header_units_without_nul, headers_to_utf16, host_to_utf16, http2_keep_alive_millis, http3_keep_alive_millis, method_to_utf16,
        parse_header_buffer, parse_protocol_used, path_to_utf16, protocol_options, query_protocol_used, query_raw_headers,
        query_raw_trailers, query_status_code, request_open_flags, security_flags, timeout_millis, validate_request_header_unit_count,
    };
    use crate::WinHttpTlsConfig;
    use crate::bindings::{Facade, MockBindings};
    use crate::error::{WinHttpError, WinHttpOperation};
    use crate::handle::RawHandle;

    assert_impl_all!(WinHttpOptions: Send, Sync, Clone, Debug, Default, ThreadAware);
    assert_impl_all!(WinHttpOptionsBuilder: Send, Sync, Clone, Debug, ThreadAware);

    #[test]
    fn default_leaves_resolve_timeout_unlimited() {
        assert_eq!(WinHttpOptions::default().resolve_timeout(), None);
    }

    #[test]
    fn builder_sets_resolve_timeout() {
        let timeout = Duration::from_secs(10);
        let options = WinHttpOptions::builder().resolve_timeout(timeout).build();

        assert_eq!(options.resolve_timeout(), Some(timeout));
    }

    #[test]
    fn signed_timeout_conversion_covers_boundaries() {
        assert_eq!(timeout_millis(None), -1);
        assert_eq!(timeout_millis(Some(Duration::ZERO)), 1);
        assert_eq!(timeout_millis(Some(Duration::from_nanos(1))), 1);
        assert_eq!(timeout_millis(Some(Duration::from_millis(i32::MAX as u64))), i32::MAX);
        assert_eq!(timeout_millis(Some(Duration::from_millis(i32::MAX as u64 + 1))), i32::MAX);
        assert_eq!(timeout_millis(Some(Duration::MAX)), i32::MAX);
    }

    #[test]
    fn dword_timeout_conversion_ceils_and_clamps() {
        assert_eq!(dword_millis(Duration::ZERO), 0);
        assert_eq!(dword_millis(Duration::from_nanos(1)), 1);
        assert_eq!(dword_millis(Duration::from_millis(u64::from(u32::MAX))), u32::MAX);
        assert_eq!(dword_millis(Duration::from_millis(u64::from(u32::MAX) + 1)), u32::MAX);
    }

    #[test]
    fn keep_alive_floor_applies_only_to_http2() {
        assert_eq!(http2_keep_alive_millis(Duration::from_millis(1)), 5_000);
        assert_eq!(http2_keep_alive_millis(Duration::from_millis(5_001)), 5_001);
        assert_eq!(http3_keep_alive_millis(Duration::ZERO), 1);
        assert_eq!(http3_keep_alive_millis(Duration::from_millis(1)), 1);
    }

    #[test]
    fn utf16_conversions_are_nul_terminated_without_embedded_nuls() {
        let method = method_to_utf16(&Method::POST).expect("method is valid");
        let host = host_to_utf16("example.com").expect("host is valid");
        let path = path_to_utf16("/resource?q=1").expect("path is valid");
        let query_only_path = path_to_utf16("?q=1").expect("query-only path is valid");
        let mut header_map = HeaderMap::new();
        header_map.append("x-duplicate", HeaderValue::from_static("first"));
        header_map.append("x-duplicate", HeaderValue::from_bytes(&[0x80, 0xff]).expect("obs-text is valid"));
        let headers = headers_to_utf16(&header_map).expect("headers are valid");

        assert_eq!(method.as_slice(), "POST".encode_utf16().collect::<Vec<_>>());
        assert_eq!(host.as_slice(), "example.com".encode_utf16().collect::<Vec<_>>());
        assert_eq!(path.as_slice(), "/resource?q=1".encode_utf16().collect::<Vec<_>>());
        assert_eq!(query_only_path.as_slice(), "/?q=1".encode_utf16().collect::<Vec<_>>());
        assert_eq!(
            headers.as_slice(),
            [
                "x-duplicate: first\r\nx-duplicate: ".encode_utf16().collect::<Vec<_>>(),
                vec![0x80, 0xff],
                "\r\n".encode_utf16().collect(),
            ]
            .concat()
        );
        assert_eq!(header_units_without_nul(&headers), headers.as_slice());
        assert_eq!(
            headers.as_slice_with_nul().last().copied(),
            Some(0),
            "the backing string remains NUL-terminated"
        );
    }

    #[test]
    fn utf16_conversions_reject_embedded_nuls() {
        assert!(matches!(host_to_utf16("exam\0ple.com"), Err(ConversionError::EmbeddedNul("host"))));
        assert!(matches!(
            path_to_utf16("/bad\0path"),
            Err(ConversionError::EmbeddedNul("request path"))
        ));
        assert!(matches!(
            super::string_to_utf16("x-test: bad\0value\r\n", "request headers"),
            Err(ConversionError::EmbeddedNul("request headers"))
        ));
    }

    #[test]
    fn request_header_unit_count_must_fit_the_winhttp_dword_length() {
        let maximum = usize::try_from(u32::MAX).expect("usize represents every DWORD length");
        assert_eq!(validate_request_header_unit_count(maximum), Ok(()));

        if let Ok(too_large) = usize::try_from(u64::from(u32::MAX) + 1) {
            assert_eq!(
                validate_request_header_unit_count(too_large),
                Err(ConversionError::RequestHeadersTooLarge)
            );
        }
    }

    #[test]
    fn context_uses_pointer_sized_native_bytes() {
        let bytes = context_bytes(0x1234);

        assert_eq!(bytes.len(), size_of::<usize>());
        assert_eq!(usize::from_ne_bytes(bytes), 0x1234);
        assert_eq!(u32::from_ne_bytes(dword_bytes(0x1234_5678)), 0x1234_5678);
    }

    #[test]
    fn option_masks_are_combined_once() {
        assert_eq!(WINHTTP_FLAG_ASYNC, 0x1000_0000);
        assert_eq!(WINHTTP_OPTION_CONTEXT_VALUE, 45);
        assert_eq!(WINHTTP_OPTION_DECOMPRESSION, 118);
        assert_eq!(WINHTTP_OPTION_DISABLE_FEATURE, 63);
        assert_eq!(WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL, 133);
        assert_eq!(WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED, 145);
        assert_eq!(WINHTTP_OPTION_HTTP_PROTOCOL_USED, 134);
        assert_eq!(WINHTTP_OPTION_HTTP2_KEEPALIVE, 164);
        assert_eq!(WINHTTP_OPTION_HTTP3_KEEPALIVE, 188);
        assert_eq!(WINHTTP_OPTION_REDIRECT_POLICY, 88);
        assert_eq!(WINHTTP_OPTION_REDIRECT_POLICY_NEVER, 0);
        assert_eq!(WINHTTP_OPTION_SECURITY_FLAGS, 31);
        assert_eq!(WINHTTP_QUERY_FLAG_WIRE_ENCODING, 0x0100_0000);
        assert_eq!(WINHTTP_QUERY_STATUS_CODE, 19);
        assert_eq!(disable_feature_mask(), 1 | 4);
        assert_eq!(decompression_mask(), 1 | 2);
        assert_eq!(
            request_open_flags(true, true),
            WINHTTP_FLAG_SECURE | WINHTTP_FLAG_AUTOMATIC_CHUNKING
        );

        assert_eq!(security_flags(&WinHttpTlsConfig::default()), 0);
        assert_eq!(
            security_flags(&WinHttpTlsConfig::builder().accept_invalid_certs(true).build()),
            0x2300
        );
        assert_eq!(
            security_flags(&WinHttpTlsConfig::builder().accept_invalid_hostnames(true).build()),
            0x1000
        );
        assert_eq!(
            security_flags(
                &WinHttpTlsConfig::builder()
                    .accept_invalid_certs(true)
                    .accept_invalid_hostnames(true)
                    .build()
            ),
            0x3300
        );
    }

    #[test]
    fn protocol_options_cover_supported_combinations() {
        let cases = [
            (&[Version::HTTP_11][..], 0, false),
            (&[Version::HTTP_11, Version::HTTP_2][..], 1, false),
            (&[Version::HTTP_11, Version::HTTP_3][..], 2, false),
            (&[Version::HTTP_11, Version::HTTP_2, Version::HTTP_3][..], 3, false),
            (&[Version::HTTP_2][..], 1, true),
            (&[Version::HTTP_3][..], 2, true),
            (&[Version::HTTP_2, Version::HTTP_3][..], 3, true),
            (&[][..], 1, false),
        ];

        for (versions, mask, required) in cases {
            let options = protocol_options(versions).expect("supported versions");
            assert_eq!(options.advanced_mask(), mask);
            assert_eq!(options.required(), required);
        }
    }

    #[test]
    fn protocol_options_reject_legacy_requested_versions() {
        assert!(matches!(
            protocol_options(&[Version::HTTP_09]),
            Err(ConversionError::UnsupportedHttpVersion(Version::HTTP_09))
        ));
        assert!(matches!(
            protocol_options(&[Version::HTTP_10]),
            Err(ConversionError::UnsupportedHttpVersion(Version::HTTP_10))
        ));
    }

    #[test]
    fn negotiated_legacy_protocol_uses_version_query_value() {
        assert_eq!(parse_protocol_used(0, Some("HTTP/1.0")).expect("valid version"), Version::HTTP_10);
        assert_eq!(parse_protocol_used(0, Some("HTTP/1.1")).expect("valid version"), Version::HTTP_11);
        assert!(matches!(
            parse_protocol_used(0, None),
            Err(ConversionError::InvalidProtocolVersion(_))
        ));
        assert!(matches!(parse_protocol_used(3, None), Err(ConversionError::InvalidProtocolMask(3))));
    }

    #[test]
    fn zero_protocol_used_queries_and_parses_legacy_version() {
        let mut sequence = Sequence::new();
        let mut bindings = MockBindings::new();
        bindings
            .expect_query_option()
            .withf(|_, option, buffer, byte_len| *option == WINHTTP_OPTION_HTTP_PROTOCOL_USED && buffer.is_some() && *byte_len == 4)
            .once()
            .in_sequence(&mut sequence)
            .returning(|_, _, buffer, byte_len| {
                let protocol = buffer.expect("protocol query supplies a DWORD").cast::<u32>();
                // SAFETY: the mock receives a writable DWORD buffer from
                // query_protocol_used and writes exactly one DWORD.
                unsafe { protocol.as_ptr().write(0) };
                *byte_len = 4;
                Ok(())
            });
        bindings
            .expect_query_headers()
            .withf(|_, info_level, buffer, byte_len| *info_level == WINHTTP_QUERY_VERSION && buffer.is_none() && *byte_len == 0)
            .once()
            .in_sequence(&mut sequence)
            .returning(|_, _, _, byte_len| {
                *byte_len = 18;
                Err(WinHttpError::new(122, WinHttpOperation::QueryHeaders))
            });
        let version_units = "HTTP/1.0".encode_utf16().collect::<Vec<_>>();
        bindings
            .expect_query_headers()
            .withf(|_, info_level, buffer, byte_len| *info_level == WINHTTP_QUERY_VERSION && buffer.is_some() && *byte_len == 18)
            .once()
            .in_sequence(&mut sequence)
            .returning(move |_, _, buffer, byte_len| {
                let output = buffer.expect("version query supplies a UTF-16 buffer").cast::<u16>();
                // SAFETY: the mock receives an 18-byte buffer and copies the
                // eight UTF-16 units of "HTTP/1.0" into it.
                unsafe {
                    output
                        .as_ptr()
                        .copy_from_nonoverlapping(version_units.as_ptr(), version_units.len());
                }
                *byte_len = 16;
                Ok(())
            });

        let version = query_protocol_used(&Facade::mock(Arc::new(bindings)), raw_handle(3)).expect("legacy protocol query succeeds");

        assert_eq!(version, Version::HTTP_10);
    }

    #[test]
    fn raw_header_primitives_validate_sizes_and_utf16() {
        assert_eq!(header_buffer_units(10).expect("even length"), 5);
        assert!(matches!(header_buffer_units(3), Err(ConversionError::HeaderByteLengthIsOdd(3))));
        assert_eq!(
            parse_header_buffer(&"a: b\r\n".encode_utf16().collect::<Vec<_>>(), 12).expect("valid header"),
            "a: b\r\n"
        );
        assert!(matches!(
            parse_header_buffer(&[0xd800], 2),
            Err(ConversionError::InvalidHeaderUtf16)
        ));
        assert!(matches!(
            parse_header_buffer(&[u16::from(b'a')], 4),
            Err(ConversionError::HeaderBufferTooSmall)
        ));
        assert!(matches!(
            parse_header_buffer(&[u16::from(b'a'), 0], 4),
            Err(ConversionError::InvalidHeaderUtf16)
        ));
    }

    #[test]
    fn numeric_status_query_uses_a_dword_buffer() {
        let mut bindings = MockBindings::new();
        bindings
            .expect_query_headers()
            .withf(|_, info_level, buffer, byte_len| {
                *info_level == (WINHTTP_QUERY_STATUS_CODE | 0x2000_0000) && buffer.is_some() && *byte_len == 4
            })
            .once()
            .returning(|_, _, buffer, byte_len| {
                let output = buffer.expect("status query supplies a DWORD").cast::<u32>();
                // SAFETY: query_status_code supplies a writable DWORD buffer.
                unsafe { output.as_ptr().write(503) };
                *byte_len = 4;
                Ok(())
            });

        assert_eq!(
            query_status_code(&Facade::mock(Arc::new(bindings)), raw_handle(4)).expect("status query succeeds"),
            503
        );
    }

    #[test]
    fn wire_encoded_header_query_uses_byte_lengths_and_preserves_bytes() {
        let raw = b"HTTP/1.1 200 OK\r\nx-obs: \x80\xff\r\n\r\n".to_vec();
        let required = u32::try_from(raw.len() + 1).expect("test header buffer length fits a DWORD");
        let returned = u32::try_from(raw.len()).expect("test header length fits a DWORD");
        let mut sequence = Sequence::new();
        let mut bindings = MockBindings::new();
        bindings
            .expect_query_headers()
            .withf(move |_, info_level, buffer, byte_len| *info_level == (0x16 | 0x0100_0000) && buffer.is_none() && *byte_len == 0)
            .once()
            .in_sequence(&mut sequence)
            .returning(move |_, _, _, byte_len| {
                *byte_len = required;
                Err(WinHttpError::new(122, WinHttpOperation::QueryHeaders))
            });
        let expected = raw.clone();
        bindings
            .expect_query_headers()
            .withf(move |_, info_level, buffer, byte_len| *info_level == (0x16 | 0x0100_0000) && buffer.is_some() && *byte_len == required)
            .once()
            .in_sequence(&mut sequence)
            .returning(move |_, _, buffer, byte_len| {
                let output = buffer.expect("raw-header query supplies a byte buffer");
                // SAFETY: the sizing query reserved required bytes, which is
                // enough for every returned byte and the trailing NUL.
                unsafe { output.as_ptr().copy_from_nonoverlapping(expected.as_ptr(), expected.len()) };
                // SAFETY: required includes one byte after the copied content.
                let terminator = unsafe { output.as_ptr().add(expected.len()) };
                // SAFETY: terminator points to the final writable byte.
                unsafe { terminator.write(0) };
                *byte_len = returned;
                Ok(())
            });

        let actual = query_raw_headers(&Facade::mock(Arc::new(bindings)), raw_handle(5)).expect("wire header query succeeds");

        assert_eq!(actual, raw);
    }

    #[test]
    fn absent_header_is_special_only_for_trailer_queries() {
        let mut trailer_bindings = MockBindings::new();
        trailer_bindings
            .expect_query_headers()
            .withf(|_, info_level, buffer, byte_len| {
                *info_level == (0x16 | 0x0200_0000 | 0x0100_0000) && buffer.is_none() && *byte_len == 0
            })
            .once()
            .returning(|_, _, _, _| Err(WinHttpError::new(12150, WinHttpOperation::QueryHeaders)));
        let trailers =
            query_raw_trailers(&Facade::mock(Arc::new(trailer_bindings)), raw_handle(1)).expect("an absent trailer block is not an error");
        assert_eq!(trailers, None);

        let mut header_bindings = MockBindings::new();
        header_bindings
            .expect_query_headers()
            .withf(|_, info_level, buffer, byte_len| *info_level == (0x16 | 0x0100_0000) && buffer.is_none() && *byte_len == 0)
            .once()
            .returning(|_, _, _, _| Err(WinHttpError::new(12150, WinHttpOperation::QueryHeaders)));
        let error = query_raw_headers(&Facade::mock(Arc::new(header_bindings)), raw_handle(2))
            .expect_err("an absent ordinary header block is an error");
        assert!(matches!(error, super::QueryError::WinHttp(error) if error.code() == 12150));
    }

    fn raw_handle(value: usize) -> RawHandle {
        RawHandle::new(value as *mut c_void).expect("test handle values are nonzero")
    }
}
