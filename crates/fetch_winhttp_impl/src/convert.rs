// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Converts between `fetch` request values and the native representations
//! `WinHTTP` requires.
//!
//! `WinHTTP` describes a request through wide strings, `DWORD` option buffers,
//! and flag masks rather than through the typed `http` values a caller
//! supplies, so every request must cross that representation gap before any
//! handle is configured (implementation.md section 10). Concentrating the
//! mapping in one module that owns no handle and performs no FFI call keeps
//! each rule directly testable and keeps the option-application sites in
//! [`crate::request`] and [`crate::session`] free of encoding detail.
//!
//! Conversions run in both directions. Outbound conversions encode caller
//! values: timeouts, method, host, path, headers, requested protocol
//! versions, and the option and flag masks that select `WinHTTP`-managed
//! behavior. Inbound conversions validate what a successful native call
//! returned - byte lengths, UTF-16 well-formedness, and the negotiated
//! protocol mask - before the transport treats it as data. Both directions
//! fail through the [`ConversionError`] roll-up so callers propagate one type
//! while diagnostics retain the precise condition
//! (implementation.md section 1.1).
//!
//! The synchronous native calls that produce the inbound values live in
//! [`crate::query`], which pairs each call with the validation defined here.

use std::time::Duration;

use fetch::options::ConnectionIdleTimeout;
use http::{HeaderMap, Method, Version};
use widestring::U16CString;
use windows::Win32::Networking::WinHttp::{
    WINHTTP_DECOMPRESSION_FLAG_DEFLATE, WINHTTP_DECOMPRESSION_FLAG_GZIP, WINHTTP_DISABLE_AUTHENTICATION, WINHTTP_DISABLE_COOKIES,
    WINHTTP_PROTOCOL_FLAG_HTTP2, WINHTTP_PROTOCOL_FLAG_HTTP3,
};

use crate::bindings::{WINHTTP_FLAG_AUTOMATIC_CHUNKING, WINHTTP_FLAG_SECURE};
use crate::options::ProtocolOptions;

/// Smallest HTTP/2 keep-alive interval WinHTTP accepts.
///
/// The `WINHTTP_OPTION_HTTP2_KEEPALIVE` entry states that callers cannot set a
/// timeout value less than 5000 milliseconds, so shorter caller intervals are
/// raised to this value instead of being rejected by the option call.
/// Ref: <https://learn.microsoft.com/windows/win32/winhttp/option-flags>
const HTTP2_KEEP_ALIVE_MINIMUM_MS: u32 = 5_000;
/// Smallest HTTP/3 keep-alive interval this transport forwards to WinHTTP.
///
/// WinHTTP documents no minimum for `WINHTTP_OPTION_HTTP3_KEEPALIVE`. A zero
/// option value is undefined (it may disable probes), so this transport floors
/// at one millisecond - the smallest unambiguous positive duration.
/// Ref: <https://learn.microsoft.com/windows/win32/winhttp/option-flags>
const HTTP3_KEEP_ALIVE_MINIMUM_MS: u32 = 1;
/// Smallest idle-reuse window WinHTTP accepts.
///
/// WinHTTP rejects a shorter window rather than clamping it. Flooring matches
/// [`http2_keep_alive_millis`] and keeps an aggressive idle policy from failing
/// session construction. The bound is documented with
/// [`crate::bindings::WINHTTP_OPTION_CONNECTION_IDLE_TIMEOUT`].
const CONNECTION_IDLE_TIMEOUT_MINIMUM_MS: u32 = 5_000;
/// Native sentinel that disables one `WinHttpSetTimeouts` deadline.
///
/// `WinHttpSetTimeouts` takes four signed millisecond parameters and reads -1
/// in any of them as "no timeout", so this single constant covers all four
/// deadlines [`crate::session`] deliberately delegates to the generic `fetch`
/// client (design.md section 6.1). Keeping one definition keeps the citation
/// reachable from every argument that encodes the sentinel.
/// Ref: <https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpsettimeouts>
pub(crate) const UNLIMITED_TIMEOUT: i32 = -1;

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
pub(crate) struct InteriorZeroCodeUnitError {
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
pub(crate) struct InvalidUtf16Error {
    value: &'static str,
}

/// Reports request headers whose UTF-16 representation cannot fit WinHTTP.
#[ohno::error]
#[display("the request headers are too large to materialize")]
struct RequestHeadersTooLargeError;

/// Reports a native returned length outside the supplied buffer.
#[ohno::error]
#[display("WinHTTP returned {returned} bytes for {value}, but its buffer holds {capacity} bytes")]
pub(crate) struct ReturnedLengthOutOfBoundsError {
    value: &'static str,
    capacity: usize,
    returned: usize,
}

/// Reports a fixed-size native value returned with the wrong byte length.
#[ohno::error]
#[display("WinHTTP returned {actual} bytes for {value}; expected {expected}")]
pub(crate) struct UnexpectedByteLengthError {
    pub(crate) value: &'static str,
    pub(crate) expected: u32,
    pub(crate) actual: u32,
}

/// Reports a caller-requested HTTP version that WinHTTP does not support.
#[ohno::error]
#[display("WinHTTP does not support requested HTTP version {version:?}")]
struct UnsupportedHttpVersionError {
    version: Version,
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

/// Encodes a caller's idle-reuse policy as the native option value.
///
/// `Unlimited` asks that pooled connections never be evicted for idleness, but
/// the option has no sentinel for that: its value is an unsigned millisecond
/// count with no reserved encoding, unlike the signed `WinHttpSetTimeouts`
/// fields where [`UNLIMITED_TIMEOUT`] means "no deadline". The largest
/// representable window is therefore the closest faithful encoding. It is safe
/// to use and it is genuinely long: the sweep compares a 64-bit elapsed-time
/// delta against this value, so the maximum neither overflows nor inverts, and
/// the window it describes exceeds forty-nine days.
///
/// A `Limited` window longer than that maximum reaches the same value through
/// [`dword_millis`], so the two arms converge rather than diverging at the top
/// of the range. Both approximations are caller-visible and are stated in
/// design.md section 2.2.
pub(crate) fn connection_idle_timeout_millis(idle_timeout: &ConnectionIdleTimeout) -> u32 {
    match idle_timeout {
        ConnectionIdleTimeout::Unlimited => u32::MAX,
        ConnectionIdleTimeout::Limited(duration) => dword_millis(*duration).max(CONNECTION_IDLE_TIMEOUT_MINIMUM_MS),
    }
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
        // WinHTTP's open-request path must be absolute; a query-only URI has no
        // path component, so supply the root "/" before the query string.
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
            // ": " and "\r\n" are appended per field below.
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

/// Every operand is a distinct bit, so `|` and `^` compute the same value and a
/// mutation between them is equivalent rather than a defect.
#[cfg_attr(test, mutants::skip)] // Disjoint-bit union: `|` and `^` are interchangeable, so operator mutants are equivalent.
pub(crate) const fn disable_feature_mask() -> u32 {
    WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION
}

/// Every operand is a distinct bit, so `|` and `^` compute the same value and a
/// mutation between them is equivalent rather than a defect.
#[cfg_attr(test, mutants::skip)] // Disjoint-bit union: `|` and `^` are interchangeable, so operator mutants are equivalent.
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

    Ok(ProtocolOptions::from_validated(advanced_mask, !allows_http11))
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

/// Decodes the UTF-16 region a WinHTTP header query wrote into a caller buffer.
///
/// WinHTTP reports the byte length it wrote rather than truncating the supplied
/// buffer, so that length is validated against the buffer before it bounds a
/// read. A zero code unit inside the reported region would silently truncate
/// the text a caller then parses, so it is rejected instead of accepted.
///
/// `value` names the queried item so a failure identifies which query produced
/// the malformed data.
pub(crate) fn parse_header_buffer(buffer: &[u16], returned_bytes: u32, value: &'static str) -> Result<String, ConversionError> {
    let units = header_buffer_units(returned_bytes)?;
    let content = buffer.get(..units).ok_or_else(|| {
        ConversionError::from(ReturnedLengthOutOfBoundsError::new(
            value,
            size_of_val(buffer),
            dword_to_usize(returned_bytes),
        ))
    })?;

    if content.contains(&0) {
        return Err(InteriorZeroCodeUnitError::new(value).into());
    }

    String::from_utf16(content).map_err(|_invalid_utf16| InvalidUtf16Error::new(value).into())
}

pub(crate) fn dword_to_usize(value: u32) -> usize {
    usize::try_from(value).expect("all supported Windows targets have at least a 32-bit usize")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::time::Duration;

    use http::{HeaderMap, HeaderValue, Method, Version};
    use ohno::ErrorExt as _;
    use static_assertions::assert_not_impl_any;

    use super::{
        CONNECTION_IDLE_TIMEOUT_MINIMUM_MS, ConnectionIdleTimeout, ConversionError, EmbeddedNulError, HeaderByteLengthIsOddError,
        InteriorZeroCodeUnitError, InvalidProtocolMaskError, InvalidProtocolVersionError, InvalidUtf16Error, RequestHeadersTooLargeError,
        ReturnedLengthOutOfBoundsError, UNLIMITED_TIMEOUT, UnexpectedByteLengthError, UnsupportedHttpVersionError,
        WINHTTP_DECOMPRESSION_FLAG_DEFLATE, WINHTTP_DECOMPRESSION_FLAG_GZIP, WINHTTP_DISABLE_AUTHENTICATION, WINHTTP_DISABLE_COOKIES,
        WINHTTP_FLAG_AUTOMATIC_CHUNKING, WINHTTP_FLAG_SECURE, connection_idle_timeout_millis, context_bytes, decompression_mask,
        disable_feature_mask, dword_bytes, dword_millis, header_buffer_units, headers_to_utf16, host_to_utf16, http2_keep_alive_millis,
        http3_keep_alive_millis, method_to_utf16, parse_header_buffer, parse_protocol_used, path_to_utf16, protocol_options,
        request_open_flags, validate_request_header_unit_count,
    };

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
        // Above the floor both protocols must carry the caller's interval through
        // unchanged. Without a case here, a conversion that always returned its floor
        // would satisfy every other expectation in this test.
        assert_eq!(http3_keep_alive_millis(Duration::from_secs(30)), 30_000);
    }

    /// Pins the sentinel `WinHttpSetTimeouts` reads as "no timeout".
    /// Session setup passes this constant for all four native deadlines and the session
    /// tests compare what was written against the same constant, so both sides move
    /// together. Only a literal expectation catches a sentinel that stopped being the
    /// documented one: a positive value would cap every request at that many
    /// milliseconds instead of deferring to the canonical `fetch` timeouts.
    #[test]
    fn unlimited_timeout_matches_the_native_sentinel() {
        assert_eq!(UNLIMITED_TIMEOUT, -1);
    }

    /// Pins the floor recorded on [`crate::bindings::WINHTTP_OPTION_CONNECTION_IDLE_TIMEOUT`].
    /// `WinHTTP` rejects a shorter window instead of clamping it, so a lower floor would
    /// make every session that configures a limited idle timeout fail to initialize, for
    /// every request, permanently. Only a literal expectation catches that: an expectation
    /// derived from the constant moves wherever the constant moves.
    #[test]
    fn idle_timeout_floor_matches_the_native_minimum() {
        assert_eq!(CONNECTION_IDLE_TIMEOUT_MINIMUM_MS, 5_000);
    }

    #[test]
    fn idle_timeout_conversion_floors_saturates_and_encodes_unlimited() {
        assert_eq!(connection_idle_timeout_millis(&ConnectionIdleTimeout::Unlimited), u32::MAX);
        assert_eq!(
            connection_idle_timeout_millis(&ConnectionIdleTimeout::Limited(Duration::ZERO)),
            5_000
        );
        assert_eq!(
            connection_idle_timeout_millis(&ConnectionIdleTimeout::Limited(Duration::from_millis(4_999))),
            5_000
        );
        assert_eq!(
            connection_idle_timeout_millis(&ConnectionIdleTimeout::Limited(Duration::from_millis(5_001))),
            5_001
        );
        assert_eq!(
            connection_idle_timeout_millis(&ConnectionIdleTimeout::Limited(Duration::from_mins(1))),
            60_000
        );
        assert_eq!(
            connection_idle_timeout_millis(&ConnectionIdleTimeout::Limited(Duration::MAX)),
            u32::MAX
        );
    }

    /// Pins the claim on [`crate::bindings::WINHTTP_OPTION_CONNECTION_IDLE_TIMEOUT`] that
    /// configuring nothing produces identical behavior whether or not the option is set.
    /// That claim holds only while `fetch`'s default equals `WinHTTP`'s own one-minute
    /// default, and nothing else in the crate would notice the two drifting apart: the
    /// session tests derive their expected value from this same conversion, so both sides
    /// would move together.
    #[test]
    fn default_idle_timeout_matches_the_native_default() {
        assert_eq!(connection_idle_timeout_millis(&ConnectionIdleTimeout::default()), 60_000);
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
    fn raw_header_primitives_validate_sizes_and_utf16() {
        assert_eq!(header_buffer_units(10).unwrap(), 5);
        let odd = header_buffer_units(3).unwrap_err();
        assert_eq!(odd.find_source::<HeaderByteLengthIsOddError>().unwrap().length, 3);
        assert_eq!(
            parse_header_buffer(&"a: b\r\n".encode_utf16().collect::<Vec<_>>(), 12, "header data").unwrap(),
            "a: b\r\n"
        );

        let invalid_utf16 = parse_header_buffer(&[0xd800], 2, "header data").unwrap_err();
        assert_eq!(invalid_utf16.find_source::<InvalidUtf16Error>().unwrap().value, "header data");

        let too_small = parse_header_buffer(&[u16::from(b'a')], 4, "header UTF-16 data").unwrap_err();
        let source = too_small.find_source::<ReturnedLengthOutOfBoundsError>().unwrap();
        assert_eq!(source.value, "header UTF-16 data");
        assert_eq!(source.capacity, 2);
        assert_eq!(source.returned, 4);

        let embedded_nul = parse_header_buffer(&[u16::from(b'a'), 0], 4, "header data").unwrap_err();
        assert_eq!(
            embedded_nul.find_source::<InteriorZeroCodeUnitError>().unwrap().value,
            "header data"
        );
    }
}
