// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::ptr::NonNull;
use std::time::Duration;

use http::{HeaderMap, Method, Version};
use thread_aware::ThreadAware;
use widestring::U16CString;
use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows::Win32::Networking::WinHttp::{
    ERROR_WINHTTP_HEADER_NOT_FOUND, SECURITY_FLAG_IGNORE_CERT_CN_INVALID, SECURITY_FLAG_IGNORE_CERT_DATE_INVALID,
    SECURITY_FLAG_IGNORE_CERT_WRONG_USAGE, SECURITY_FLAG_IGNORE_UNKNOWN_CA, WINHTTP_DECOMPRESSION_FLAG_DEFLATE,
    WINHTTP_DECOMPRESSION_FLAG_GZIP, WINHTTP_DISABLE_AUTHENTICATION, WINHTTP_DISABLE_COOKIES, WINHTTP_PROTOCOL_FLAG_HTTP2,
    WINHTTP_PROTOCOL_FLAG_HTTP3, WINHTTP_QUERY_FLAG_TRAILERS,
};
pub(crate) use windows::Win32::Networking::WinHttp::{
    WINHTTP_FLAG_ASYNC, WINHTTP_FLAG_AUTOMATIC_CHUNKING, WINHTTP_FLAG_SECURE, WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH,
    WINHTTP_OPTION_CONTEXT_VALUE, WINHTTP_OPTION_DECOMPRESSION, WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL,
    WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED, WINHTTP_OPTION_HTTP_PROTOCOL_USED, WINHTTP_OPTION_HTTP2_KEEPALIVE,
    WINHTTP_OPTION_HTTP3_KEEPALIVE, WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER, WINHTTP_OPTION_SECURITY_FLAGS,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_FLAG_WIRE_ENCODING, WINHTTP_QUERY_RAW_HEADERS_CRLF, WINHTTP_QUERY_STATUS_CODE,
    WINHTTP_QUERY_VERSION,
};

use crate::bindings::{Bindings as _, BindingsFacade};
use crate::error::WinHttpError;
use crate::handle::RawHandle;
use crate::tls::WinHttpTlsConfig;

// WinHTTP documents a 5000 ms minimum for HTTP/2 and a 1 ms minimum for
// HTTP/3: https://learn.microsoft.com/windows/win32/winhttp/option-flags
const HTTP2_KEEP_ALIVE_MINIMUM_MS: u32 = 5_000;
const HTTP3_KEEP_ALIVE_MINIMUM_MS: u32 = 1;
// WinHttpSetTimeouts uses -1 for an unlimited timeout:
// https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpsettimeouts
const UNLIMITED_TIMEOUT: i32 = -1;
const DWORD_BYTES: u32 = 4;

/// Configures native behavior specific to the WinHTTP transport.
///
/// The resolve timeout is a native DNS-only deadline because generic `fetch`
/// options have no separately awaitable name-resolution stage. Generic connect,
/// response-header, body-idle, and pipeline request timeouts remain responsible
/// for their broader intervals and are not replaced by this option.
///
/// By default the native DNS resolution timeout is unlimited.
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

/// Builds [`WinHttpOptions`] without changing generic request deadlines.
///
/// It configures only the native DNS-resolution timer; all other timeout
/// responsibilities remain with the generic client and request options.
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
    /// millisecond. Values beyond the signed [`WinHttpSetTimeouts`] parameter
    /// range are clamped.
    ///
    /// [`WinHttpSetTimeouts`]: https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpsettimeouts
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

/// Unifies native value-conversion failures without erasing their causes.
///
/// Request construction and synchronous WinHTTP queries use this aggregate so
/// callers can propagate one error type while tests and diagnostics retain the
/// precise malformed input or returned-value condition.
#[ohno::error]
#[from(
    EmbeddedNulError,
    HeaderByteLengthIsOddError,
    InteriorZeroCodeUnitError,
    InvalidProtocolMaskError,
    InvalidProtocolVersionError,
    InvalidUtf16Error,
    RequestHeadersTooLargeError,
    ReturnedLengthOutOfBoundsError,
    UnexpectedByteLengthError,
    UnsupportedHttpVersionError
)]
#[display("WinHTTP value conversion failed")]
pub(crate) struct ConversionError;

/// Reports an embedded zero that cannot be represented by a WinHTTP string.
#[ohno::error]
#[display("{field} contains an embedded zero character")]
struct EmbeddedNulError {
    field: &'static str,
}

/// Reports a WinHTTP UTF-16 byte count that cannot contain whole code units.
#[ohno::error]
#[display("WinHTTP returned an odd UTF-16 byte length: {length}")]
struct HeaderByteLengthIsOddError {
    length: u32,
}

/// Reports a zero code unit in a WinHTTP string that must not contain one.
#[ohno::error]
#[display("WinHTTP returned an interior zero code unit in {value}")]
struct InteriorZeroCodeUnitError {
    value: &'static str,
}

/// Reports a negotiated-protocol bitmask that does not identify one protocol.
#[ohno::error]
#[display("WinHTTP returned an invalid HTTP protocol mask: {mask}")]
struct InvalidProtocolMaskError {
    mask: u32,
}

/// Reports a textual HTTP version that the transport cannot represent.
#[ohno::error]
#[display("WinHTTP returned an unsupported HTTP version: {version}")]
struct InvalidProtocolVersionError {
    version: String,
}

/// Reports malformed UTF-16 returned by WinHTTP.
#[ohno::error]
#[display("WinHTTP returned invalid UTF-16 for {value}")]
struct InvalidUtf16Error {
    value: &'static str,
}

/// Reports request headers whose UTF-16 representation cannot fit WinHTTP.
#[ohno::error]
#[display("the request headers are too large to materialize")]
struct RequestHeadersTooLargeError;

/// Reports a native returned length outside the supplied buffer.
#[ohno::error]
#[display("WinHTTP returned {returned} bytes for {value}, but its buffer holds {capacity} bytes")]
struct ReturnedLengthOutOfBoundsError {
    value: &'static str,
    capacity: usize,
    returned: usize,
}

/// Reports a fixed-size native value returned with the wrong byte length.
#[ohno::error]
#[display("WinHTTP returned {actual} bytes for {value}; expected {expected}")]
struct UnexpectedByteLengthError {
    value: &'static str,
    expected: u32,
    actual: u32,
}

/// Reports a caller-requested HTTP version that WinHTTP does not support.
#[ohno::error]
#[display("WinHTTP does not support requested HTTP version {version:?}")]
struct UnsupportedHttpVersionError {
    version: Version,
}

/// Separates failed native queries from malformed values they return.
///
/// Query helpers preserve either the original [`WinHttpError`] or the precise
/// [`ConversionError`], allowing request processing to report the source without
/// conflating an operating-system failure with invalid native output.
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

/// Translates a supported HTTP version set into WinHTTP request options.
///
/// The mask enables HTTP/2 and HTTP/3 because HTTP/1.1 is the WinHTTP baseline.
/// `required` is set whenever HTTP/1.1 is absent, and must be applied with the
/// mask so WinHTTP fails negotiation instead of silently downgrading. Unsupported
/// versions are rejected before this value is constructed.
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
    dword_millis(duration).max(HTTP3_KEEP_ALIVE_MINIMUM_MS)
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
        return U16CString::from_vec(units).map_err(|_contains_nul| EmbeddedNulError::new("request path").into());
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
    let unit_count = unit_count.ok_or_else(|| ConversionError::from(RequestHeadersTooLargeError::new()))?;
    validate_request_header_unit_count(unit_count)?;
    let capacity = unit_count
        .checked_add(1)
        .ok_or_else(|| ConversionError::from(RequestHeadersTooLargeError::new()))?;
    let mut units = Vec::with_capacity(capacity);

    for (name, value) in headers {
        units.extend(name.as_str().bytes().map(u16::from));
        units.extend([u16::from(b':'), u16::from(b' ')]);
        units.extend(value.as_bytes().iter().copied().map(u16::from));
        units.extend([u16::from(b'\r'), u16::from(b'\n')]);
    }

    U16CString::from_vec(units).map_err(|_contains_nul| EmbeddedNulError::new("request headers").into())
}

fn validate_request_header_unit_count(unit_count: usize) -> Result<(), ConversionError> {
    u32::try_from(unit_count)
        .map(|_unit_count| ())
        .map_err(|_too_large| RequestHeadersTooLargeError::new().into())
}

fn string_to_utf16(value: &str, field: &'static str) -> Result<U16CString, ConversionError> {
    U16CString::from_str(value).map_err(|_contains_nul| EmbeddedNulError::new(field).into())
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
        flags |= WINHTTP_FLAG_SECURE.0;
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
                return Err(UnsupportedHttpVersionError::new(unsupported).into());
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
            legacy_version
                .ok_or_else(|| ConversionError::from(InvalidProtocolVersionError::new("missing WINHTTP_QUERY_VERSION value".to_owned())))?,
        ),
        invalid => Err(InvalidProtocolMaskError::new(invalid).into()),
    }
}

fn parse_legacy_version(version: &str) -> Result<Version, ConversionError> {
    match version.trim_end_matches('\0') {
        "HTTP/0.9" => Ok(Version::HTTP_09),
        "HTTP/1.0" => Ok(Version::HTTP_10),
        "HTTP/1.1" => Ok(Version::HTTP_11),
        invalid => Err(InvalidProtocolVersionError::new(invalid.to_owned()).into()),
    }
}

pub(crate) fn header_buffer_units(byte_len: u32) -> Result<usize, ConversionError> {
    if !byte_len.is_multiple_of(2) {
        return Err(HeaderByteLengthIsOddError::new(byte_len).into());
    }

    Ok(dword_to_usize(byte_len / 2))
}

#[cfg(test)]
pub(crate) fn parse_header_buffer(buffer: &[u16], returned_bytes: u32) -> Result<String, ConversionError> {
    let units = header_buffer_units(returned_bytes)?;
    let content = buffer.get(..units).ok_or_else(|| {
        ConversionError::from(ReturnedLengthOutOfBoundsError::new(
            "header UTF-16 data",
            size_of_val(buffer),
            dword_to_usize(returned_bytes),
        ))
    })?;

    if content.contains(&0) {
        return Err(InteriorZeroCodeUnitError::new("header data").into());
    }

    String::from_utf16(content).map_err(|_invalid_utf16| InvalidUtf16Error::new("header data").into())
}

pub(crate) fn query_status_code(bindings: &BindingsFacade, request: RawHandle) -> Result<u32, QueryError> {
    let mut status_code = 0_u32;
    let mut byte_len = DWORD_BYTES;
    let buffer = NonNull::from(&mut status_code).cast();

    // SAFETY: callers query only a live request while its RequestGuard owns the
    // handle and no asynchronous operation is outstanding. status_code is a
    // writable DWORD and byte_len describes its exact capacity.
    unsafe {
        bindings.query_headers(
            request,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            Some(buffer),
            &mut byte_len,
        )
    }?;

    if byte_len != DWORD_BYTES {
        return Err(ConversionError::from(UnexpectedByteLengthError::new("HTTP status code", DWORD_BYTES, byte_len)).into());
    }

    Ok(status_code)
}

pub(crate) fn query_raw_headers(bindings: &BindingsFacade, request: RawHandle) -> Result<Vec<u8>, QueryError> {
    query_header_bytes(bindings, request, WINHTTP_QUERY_RAW_HEADERS_CRLF | WINHTTP_QUERY_FLAG_WIRE_ENCODING)
}

pub(crate) fn query_raw_trailers(bindings: &BindingsFacade, request: RawHandle) -> Result<Option<Vec<u8>>, QueryError> {
    match query_header_bytes(
        bindings,
        request,
        WINHTTP_QUERY_RAW_HEADERS_CRLF | WINHTTP_QUERY_FLAG_TRAILERS | WINHTTP_QUERY_FLAG_WIRE_ENCODING,
    ) {
        Err(QueryError::WinHttp(error)) if error.code() == ERROR_WINHTTP_HEADER_NOT_FOUND => Ok(None),
        result => result.map(Some),
    }
}

pub(crate) fn query_protocol_used(bindings: &BindingsFacade, request: RawHandle) -> Result<Version, QueryError> {
    let mut protocol_mask = 0_u32;
    let mut byte_len = DWORD_BYTES;
    let buffer = NonNull::from(&mut protocol_mask).cast();

    // SAFETY: callers query only a live request while its RequestGuard owns the
    // handle and no asynchronous operation is outstanding. protocol_mask is a
    // writable DWORD and byte_len describes its exact capacity.
    unsafe { bindings.query_option(request, WINHTTP_OPTION_HTTP_PROTOCOL_USED, Some(buffer), &mut byte_len) }?;

    if byte_len != DWORD_BYTES {
        return Err(ConversionError::from(UnexpectedByteLengthError::new("negotiated HTTP protocol", DWORD_BYTES, byte_len)).into());
    }

    if protocol_mask == 0 {
        let version = query_header_string(bindings, request, WINHTTP_QUERY_VERSION, "HTTP version")?;
        Ok(parse_protocol_used(0, Some(&version))?)
    } else {
        Ok(parse_protocol_used(protocol_mask, None)?)
    }
}

fn query_header_string(bindings: &BindingsFacade, request: RawHandle, info_level: u32, value: &'static str) -> Result<String, QueryError> {
    let buffer = query_header_units(bindings, request, info_level, value)?;

    String::from_utf16(&buffer).map_err(|_invalid_utf16| QueryError::from(ConversionError::from(InvalidUtf16Error::new(value))))
}

fn query_header_bytes(bindings: &BindingsFacade, request: RawHandle, info_level: u32) -> Result<Vec<u8>, QueryError> {
    let mut required_bytes = 0_u32;

    // SAFETY: callers query only a live request while its RequestGuard owns the
    // handle and no asynchronous operation is outstanding. A null buffer with
    // zero capacity is the documented sizing query.
    match unsafe { bindings.query_headers(request, info_level, None, &mut required_bytes) } {
        Err(error) if error.code() == ERROR_INSUFFICIENT_BUFFER.0 => {}
        Err(error) => return Err(error.into()),
        Ok(()) if required_bytes == 0 => return Ok(Vec::new()),
        Ok(()) => {}
    }

    let capacity = dword_to_usize(required_bytes);
    let mut buffer = vec![0_u8; capacity];
    let output = NonNull::new(buffer.as_mut_ptr()).expect("Vec::as_mut_ptr is guaranteed to be nonnull");
    let mut returned_bytes = required_bytes;

    // SAFETY: the request remains live with no asynchronous operation
    // outstanding. output points to a writable buffer of required_bytes and
    // remains valid for the duration of this synchronous query.
    unsafe { bindings.query_headers(request, info_level, Some(output), &mut returned_bytes) }?;

    let returned_bytes = dword_to_usize(returned_bytes);
    if returned_bytes > buffer.len() {
        return Err(ConversionError::from(ReturnedLengthOutOfBoundsError::new(
            "wire-encoded headers",
            buffer.len(),
            returned_bytes,
        ))
        .into());
    }
    buffer.truncate(returned_bytes);

    Ok(buffer)
}

fn query_header_units(bindings: &BindingsFacade, request: RawHandle, info_level: u32, value: &'static str) -> Result<Vec<u16>, QueryError> {
    let mut required_bytes = 0_u32;

    // SAFETY: callers query only a live request while its RequestGuard owns the
    // handle and no asynchronous operation is outstanding. A null buffer with
    // zero capacity is the documented sizing query.
    match unsafe { bindings.query_headers(request, info_level, None, &mut required_bytes) } {
        Err(error) if error.code() == ERROR_INSUFFICIENT_BUFFER.0 => {}
        Err(error) => return Err(error.into()),
        Ok(()) if required_bytes == 0 => return Ok(Vec::new()),
        Ok(()) => {}
    }

    let units = header_buffer_units(required_bytes)?;
    let mut buffer = vec![0_u16; units];
    let output = NonNull::new(buffer.as_mut_ptr().cast::<u8>()).expect("Vec::as_mut_ptr is guaranteed to be nonnull");
    let mut returned_bytes = required_bytes;

    // SAFETY: the request remains live with no asynchronous operation
    // outstanding. output points to a writable buffer of required_bytes and
    // remains valid for the duration of this synchronous query.
    unsafe { bindings.query_headers(request, info_level, Some(output), &mut returned_bytes) }?;

    let returned_units = header_buffer_units(returned_bytes)?;
    if returned_units > buffer.len() {
        return Err(ConversionError::from(ReturnedLengthOutOfBoundsError::new(
            value,
            buffer.len() * size_of::<u16>(),
            dword_to_usize(returned_bytes),
        ))
        .into());
    }
    buffer.truncate(returned_units);

    if buffer.contains(&0) {
        return Err(ConversionError::from(InteriorZeroCodeUnitError::new(value)).into());
    }

    Ok(buffer)
}

fn dword_to_usize(value: u32) -> usize {
    usize::try_from(value).expect("all supported Windows targets have at least a 32-bit usize")
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::fmt::Debug;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::Arc;
    use std::time::Duration;

    use http::{HeaderMap, HeaderValue, Method, Version};
    use mockall::Sequence;
    use ohno::ErrorExt as _;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use thread_aware::ThreadAware;

    use super::{
        ConversionError, ERROR_INSUFFICIENT_BUFFER, ERROR_WINHTTP_HEADER_NOT_FOUND, EmbeddedNulError, HeaderByteLengthIsOddError,
        InteriorZeroCodeUnitError, InvalidProtocolMaskError, InvalidProtocolVersionError, InvalidUtf16Error, ProtocolOptions, QueryError,
        RequestHeadersTooLargeError, ReturnedLengthOutOfBoundsError, SECURITY_FLAG_IGNORE_CERT_CN_INVALID,
        SECURITY_FLAG_IGNORE_CERT_DATE_INVALID, SECURITY_FLAG_IGNORE_CERT_WRONG_USAGE, SECURITY_FLAG_IGNORE_UNKNOWN_CA,
        UnexpectedByteLengthError, UnsupportedHttpVersionError, WINHTTP_DECOMPRESSION_FLAG_DEFLATE, WINHTTP_DECOMPRESSION_FLAG_GZIP,
        WINHTTP_DISABLE_AUTHENTICATION, WINHTTP_DISABLE_COOKIES, WINHTTP_FLAG_AUTOMATIC_CHUNKING, WINHTTP_FLAG_SECURE,
        WINHTTP_OPTION_HTTP_PROTOCOL_USED, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_FLAG_TRAILERS, WINHTTP_QUERY_FLAG_WIRE_ENCODING,
        WINHTTP_QUERY_RAW_HEADERS_CRLF, WINHTTP_QUERY_STATUS_CODE, WINHTTP_QUERY_VERSION, WinHttpOptions, WinHttpOptionsBuilder,
        context_bytes, decompression_mask, disable_feature_mask, dword_bytes, dword_millis, header_buffer_units, headers_to_utf16,
        host_to_utf16, http2_keep_alive_millis, http3_keep_alive_millis, method_to_utf16, parse_header_buffer, parse_protocol_used,
        path_to_utf16, protocol_options, query_protocol_used, query_raw_headers, query_raw_trailers, query_status_code, request_open_flags,
        security_flags, timeout_millis, validate_request_header_unit_count,
    };
    use crate::WinHttpTlsConfig;
    use crate::bindings::{BindingsFacade, MockBindings};
    use crate::error::{WinHttpError, WinHttpOperation};
    use crate::handle::RawHandle;

    assert_impl_all!(WinHttpOptions: Send, Sync, Clone, Debug, Default, ThreadAware, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(WinHttpOptionsBuilder: Send, Sync, Clone, Debug, ThreadAware, UnwindSafe, RefUnwindSafe);
    // The generated error wrappers retain user-erased source error state.
    assert_not_impl_any!(ConversionError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedNulError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(HeaderByteLengthIsOddError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(InteriorZeroCodeUnitError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(InvalidProtocolMaskError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(InvalidProtocolVersionError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(InvalidUtf16Error: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(RequestHeadersTooLargeError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(ReturnedLengthOutOfBoundsError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(UnexpectedByteLengthError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(UnsupportedHttpVersionError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(QueryError: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ProtocolOptions: Send, Sync, Clone, Copy, Debug, Eq, PartialEq, UnwindSafe, RefUnwindSafe);

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
        let method = method_to_utf16(&Method::POST).unwrap();
        let host = host_to_utf16("example.com").unwrap();
        let path = path_to_utf16("/resource?q=1").unwrap();
        let query_only_path = path_to_utf16("?q=1").unwrap();
        let mut header_map = HeaderMap::new();
        header_map.append("x-duplicate", HeaderValue::from_static("first"));
        header_map.append("x-duplicate", HeaderValue::from_bytes(&[0x80, 0xff]).unwrap());
        let headers = headers_to_utf16(&header_map).unwrap();

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
        assert_eq!(
            headers.as_slice_with_nul().last().copied(),
            Some(0),
            "the backing string remains NUL-terminated"
        );
    }

    #[test]
    fn utf16_conversions_reject_embedded_nuls() {
        let host = host_to_utf16("exam\0ple.com").unwrap_err();
        assert_eq!(host.find_source::<EmbeddedNulError>().unwrap().field, "host");

        let path = path_to_utf16("/bad\0path").unwrap_err();
        assert_eq!(path.find_source::<EmbeddedNulError>().unwrap().field, "request path");

        let headers = super::string_to_utf16("x-test: bad\0value\r\n", "request headers").unwrap_err();
        assert_eq!(headers.find_source::<EmbeddedNulError>().unwrap().field, "request headers");
    }

    #[test]
    fn request_header_unit_count_must_fit_the_winhttp_dword_length() {
        let maximum = usize::try_from(u32::MAX).unwrap();
        validate_request_header_unit_count(maximum).unwrap();

        if let Ok(too_large) = usize::try_from(u64::from(u32::MAX) + 1) {
            let error = validate_request_header_unit_count(too_large).unwrap_err();
            assert!(error.find_source::<RequestHeadersTooLargeError>().is_some());
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
    fn option_masks_combine_windows_sdk_flags() {
        assert_eq!(disable_feature_mask(), WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION);
        assert_eq!(
            decompression_mask(),
            WINHTTP_DECOMPRESSION_FLAG_GZIP | WINHTTP_DECOMPRESSION_FLAG_DEFLATE
        );
        assert_eq!(
            request_open_flags(true, true),
            WINHTTP_FLAG_SECURE.0 | WINHTTP_FLAG_AUTOMATIC_CHUNKING
        );

        assert_eq!(security_flags(&WinHttpTlsConfig::default()), 0);
        assert_eq!(
            security_flags(&WinHttpTlsConfig::builder().accept_invalid_certs(true).build()),
            SECURITY_FLAG_IGNORE_UNKNOWN_CA | SECURITY_FLAG_IGNORE_CERT_WRONG_USAGE | SECURITY_FLAG_IGNORE_CERT_DATE_INVALID
        );
        assert_eq!(
            security_flags(&WinHttpTlsConfig::builder().accept_invalid_hostnames(true).build()),
            SECURITY_FLAG_IGNORE_CERT_CN_INVALID
        );
        assert_eq!(
            security_flags(
                &WinHttpTlsConfig::builder()
                    .accept_invalid_certs(true)
                    .accept_invalid_hostnames(true)
                    .build()
            ),
            SECURITY_FLAG_IGNORE_UNKNOWN_CA
                | SECURITY_FLAG_IGNORE_CERT_WRONG_USAGE
                | SECURITY_FLAG_IGNORE_CERT_CN_INVALID
                | SECURITY_FLAG_IGNORE_CERT_DATE_INVALID
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
            let options = protocol_options(versions).unwrap();
            assert_eq!(options.advanced_mask(), mask);
            assert_eq!(options.required(), required);
        }
    }

    #[test]
    fn protocol_options_reject_legacy_requested_versions() {
        let http09 = protocol_options(&[Version::HTTP_09]).unwrap_err();
        assert_eq!(
            http09.find_source::<UnsupportedHttpVersionError>().unwrap().version,
            Version::HTTP_09
        );

        let http10 = protocol_options(&[Version::HTTP_10]).unwrap_err();
        assert_eq!(
            http10.find_source::<UnsupportedHttpVersionError>().unwrap().version,
            Version::HTTP_10
        );
    }

    #[test]
    fn negotiated_legacy_protocol_uses_version_query_value() {
        assert_eq!(parse_protocol_used(0, Some("HTTP/1.0")).unwrap(), Version::HTTP_10);
        assert_eq!(parse_protocol_used(0, Some("HTTP/1.1")).unwrap(), Version::HTTP_11);

        let missing = parse_protocol_used(0, None).unwrap_err();
        assert_eq!(
            missing.find_source::<InvalidProtocolVersionError>().unwrap().version,
            "missing WINHTTP_QUERY_VERSION value"
        );

        let invalid_mask = parse_protocol_used(3, None).unwrap_err();
        assert_eq!(invalid_mask.find_source::<InvalidProtocolMaskError>().unwrap().mask, 3);
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
                let protocol = buffer.unwrap().cast::<u32>();
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
                let output = buffer.unwrap().cast::<u16>();
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

        let version = query_protocol_used(&BindingsFacade::mock(Arc::new(bindings)), raw_handle(3)).unwrap();

        assert_eq!(version, Version::HTTP_10);
    }

    #[test]
    fn raw_header_primitives_validate_sizes_and_utf16() {
        assert_eq!(header_buffer_units(10).unwrap(), 5);
        let odd = header_buffer_units(3).unwrap_err();
        assert_eq!(odd.find_source::<HeaderByteLengthIsOddError>().unwrap().length, 3);
        assert_eq!(
            parse_header_buffer(&"a: b\r\n".encode_utf16().collect::<Vec<_>>(), 12).unwrap(),
            "a: b\r\n"
        );

        let invalid_utf16 = parse_header_buffer(&[0xd800], 2).unwrap_err();
        assert_eq!(invalid_utf16.find_source::<InvalidUtf16Error>().unwrap().value, "header data");

        let too_small = parse_header_buffer(&[u16::from(b'a')], 4).unwrap_err();
        let source = too_small.find_source::<ReturnedLengthOutOfBoundsError>().unwrap();
        assert_eq!(source.value, "header UTF-16 data");
        assert_eq!(source.capacity, 2);
        assert_eq!(source.returned, 4);

        let embedded_nul = parse_header_buffer(&[u16::from(b'a'), 0], 4).unwrap_err();
        assert_eq!(
            embedded_nul.find_source::<InteriorZeroCodeUnitError>().unwrap().value,
            "header data"
        );
    }

    #[test]
    fn numeric_status_query_uses_a_dword_buffer() {
        let mut bindings = MockBindings::new();
        bindings
            .expect_query_headers()
            .withf(|_, info_level, buffer, byte_len| {
                *info_level == (WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER) && buffer.is_some() && *byte_len == 4
            })
            .once()
            .returning(|_, _, buffer, byte_len| {
                let output = buffer.unwrap().cast::<u32>();
                // SAFETY: query_status_code supplies a writable DWORD buffer.
                unsafe { output.as_ptr().write(503) };
                *byte_len = 4;
                Ok(())
            });

        assert_eq!(
            query_status_code(&BindingsFacade::mock(Arc::new(bindings)), raw_handle(4)).unwrap(),
            503
        );
    }

    #[test]
    fn fixed_width_queries_report_the_actual_native_length() {
        let mut status_bindings = MockBindings::new();
        status_bindings.expect_query_headers().once().returning(|_, _, _, byte_len| {
            *byte_len = 2;
            Ok(())
        });

        let status = query_status_code(&BindingsFacade::mock(Arc::new(status_bindings)), raw_handle(1)).unwrap_err();
        let status = match status {
            QueryError::Conversion(error) => error,
            QueryError::WinHttp(error) => panic!("unexpected WinHTTP error: {error}"),
        };
        let source = status.find_source::<UnexpectedByteLengthError>().unwrap();
        assert_eq!(source.value, "HTTP status code");
        assert_eq!(source.expected, 4);
        assert_eq!(source.actual, 2);

        let mut protocol_bindings = MockBindings::new();
        protocol_bindings.expect_query_option().once().returning(|_, _, _, byte_len| {
            *byte_len = 8;
            Ok(())
        });

        let protocol = query_protocol_used(&BindingsFacade::mock(Arc::new(protocol_bindings)), raw_handle(2)).unwrap_err();
        let protocol = match protocol {
            QueryError::Conversion(error) => error,
            QueryError::WinHttp(error) => panic!("unexpected WinHTTP error: {error}"),
        };
        let source = protocol.find_source::<UnexpectedByteLengthError>().unwrap();
        assert_eq!(source.value, "negotiated HTTP protocol");
        assert_eq!(source.expected, 4);
        assert_eq!(source.actual, 8);
    }

    #[test]
    fn wire_encoded_header_query_uses_byte_lengths_and_preserves_bytes() {
        let raw = b"HTTP/1.1 200 OK\r\nx-obs: \x80\xff\r\n\r\n".to_vec();
        let required = u32::try_from(raw.len() + 1).unwrap();
        let returned = u32::try_from(raw.len()).unwrap();
        let mut sequence = Sequence::new();
        let mut bindings = MockBindings::new();
        bindings
            .expect_query_headers()
            .withf(move |_, info_level, buffer, byte_len| {
                *info_level == (WINHTTP_QUERY_RAW_HEADERS_CRLF | WINHTTP_QUERY_FLAG_WIRE_ENCODING) && buffer.is_none() && *byte_len == 0
            })
            .once()
            .in_sequence(&mut sequence)
            .returning(move |_, _, _, byte_len| {
                *byte_len = required;
                Err(WinHttpError::new(ERROR_INSUFFICIENT_BUFFER.0, WinHttpOperation::QueryHeaders))
            });
        let expected = raw.clone();
        bindings
            .expect_query_headers()
            .withf(move |_, info_level, buffer, byte_len| {
                *info_level == (WINHTTP_QUERY_RAW_HEADERS_CRLF | WINHTTP_QUERY_FLAG_WIRE_ENCODING)
                    && buffer.is_some()
                    && *byte_len == required
            })
            .once()
            .in_sequence(&mut sequence)
            .returning(move |_, _, buffer, byte_len| {
                let output = buffer.unwrap();
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

        let actual = query_raw_headers(&BindingsFacade::mock(Arc::new(bindings)), raw_handle(5)).unwrap();

        assert_eq!(actual, raw);
    }

    #[test]
    fn absent_header_is_special_only_for_trailer_queries() {
        let mut trailer_bindings = MockBindings::new();
        trailer_bindings
            .expect_query_headers()
            .withf(|_, info_level, buffer, byte_len| {
                *info_level == (WINHTTP_QUERY_RAW_HEADERS_CRLF | WINHTTP_QUERY_FLAG_TRAILERS | WINHTTP_QUERY_FLAG_WIRE_ENCODING)
                    && buffer.is_none()
                    && *byte_len == 0
            })
            .once()
            .returning(|_, _, _, _| Err(WinHttpError::new(ERROR_WINHTTP_HEADER_NOT_FOUND, WinHttpOperation::QueryHeaders)));
        let trailers = query_raw_trailers(&BindingsFacade::mock(Arc::new(trailer_bindings)), raw_handle(1)).unwrap();
        assert_eq!(trailers, None);

        let mut header_bindings = MockBindings::new();
        header_bindings
            .expect_query_headers()
            .withf(|_, info_level, buffer, byte_len| {
                *info_level == (WINHTTP_QUERY_RAW_HEADERS_CRLF | WINHTTP_QUERY_FLAG_WIRE_ENCODING) && buffer.is_none() && *byte_len == 0
            })
            .once()
            .returning(|_, _, _, _| Err(WinHttpError::new(ERROR_WINHTTP_HEADER_NOT_FOUND, WinHttpOperation::QueryHeaders)));
        let error = query_raw_headers(&BindingsFacade::mock(Arc::new(header_bindings)), raw_handle(2)).unwrap_err();
        assert!(matches!(error, QueryError::WinHttp(error) if error.code() == ERROR_WINHTTP_HEADER_NOT_FOUND));
    }

    fn raw_handle(value: usize) -> RawHandle {
        RawHandle::new(value as *mut c_void).unwrap()
    }
}
