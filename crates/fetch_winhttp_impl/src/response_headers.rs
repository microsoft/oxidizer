// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Parses the raw response header and trailer blocks returned by `WinHTTP`.
//!
//! `WinHTTP` hands back response metadata as one CRLF-framed byte block rather
//! than as structured fields, so the transport must reconstruct a
//! [`HeaderMap`] from it before a response can be built. Response headers are
//! parsed once the headers-available completion arrives, and trailers are
//! parsed once the response body reaches end of stream (design.md section 5,
//! "Trailers"; implementation.md section 6.2 and section 6.3).
//!
//! This module is deliberately a pure function over `&[u8]`: it touches no
//! handle, no callback context, and no FFI entry point, which keeps the
//! parsing rules exhaustively testable without driving a request lifecycle.
//! Every rejection here is a malformed-transport-response failure rather than
//! an HTTP status failure, and reaches callers through
//! [`crate::error::invalid_response`] with the `request_winhttp` label and
//! non-recoverable classification (design.md section 7).

use http::HeaderMap;
use http::header::{HeaderName, HeaderValue};

/// Identifies malformed response metadata returned by `WinHTTP`.
///
/// Header parsing preserves repeated values but requires a valid HTTP status
/// line, CRLF framing, ASCII field names, and `http`-compatible values. These
/// errors indicate an invalid transport response rather than an HTTP status
/// failure.
///
/// Each condition is a separate `ohno` source type rolled up here through a
/// generated `From` implementation, matching the crate-wide convention for
/// conversion failures (implementation.md section 1.1).
#[ohno::error]
#[from(
    InvalidHeaderNameError,
    InvalidHeaderValueError,
    InvalidStatusLineError,
    MissingHeaderTerminatorError,
    MissingNameValueSeparatorError,
    NonAsciiHeaderNameError,
    TrailingHeaderDataError
)]
#[display("WinHTTP returned a malformed response header block")]
pub(crate) struct ResponseHeadersError;

/// Reports a header field name that `http` cannot represent.
#[ohno::error]
#[display("WinHTTP returned an invalid response header name: {detail}")]
struct InvalidHeaderNameError {
    detail: String,
}

/// Reports a header field value that `http` cannot represent.
#[ohno::error]
#[display("WinHTTP returned an invalid response header value: {detail}")]
struct InvalidHeaderValueError {
    detail: String,
}

/// Reports a header block whose first line is not an HTTP status line.
#[ohno::error]
#[display("WinHTTP returned a malformed response status line")]
struct InvalidStatusLineError;

/// Reports a header block that never reaches its terminating empty line.
#[ohno::error]
#[display("WinHTTP returned a response header block without a terminating empty line")]
struct MissingHeaderTerminatorError;

/// Reports a header field line that carries no name/value separator.
#[ohno::error]
#[display("WinHTTP returned a response header without a ':' separator")]
struct MissingNameValueSeparatorError;

/// Reports a header field name byte outside the ASCII range.
#[ohno::error]
#[display("WinHTTP returned a non-ASCII response header name byte: 0x{byte:02x}")]
struct NonAsciiHeaderNameError {
    byte: u8,
}

/// Reports bytes following the header block terminator.
#[ohno::error]
#[display("WinHTTP returned data after the response header terminator")]
struct TrailingHeaderDataError;

/// Parses a complete response header block, status line included.
///
/// `WINHTTP_QUERY_RAW_HEADERS_CRLF` always prefixes the field lines with the
/// status line, so it is consumed and validated before field parsing begins.
pub(crate) fn parse_response_headers(raw: &[u8]) -> Result<HeaderMap, ResponseHeadersError> {
    let mut cursor = 0;
    let status_line = take_crlf_line(raw, &mut cursor).ok_or_else(InvalidStatusLineError::new)?;
    if !status_line.starts_with(b"HTTP/") {
        return Err(InvalidStatusLineError::new().into());
    }

    parse_header_fields(raw, cursor)
}

/// Parses a response trailer block, which carries no status line.
pub(crate) fn parse_response_trailers(raw: &[u8]) -> Result<HeaderMap, ResponseHeadersError> {
    parse_header_fields(raw, 0)
}

fn parse_header_fields(raw: &[u8], mut cursor: usize) -> Result<HeaderMap, ResponseHeadersError> {
    let mut headers = HeaderMap::new();

    loop {
        let line = take_crlf_line(raw, &mut cursor).ok_or_else(MissingHeaderTerminatorError::new)?;
        if line.is_empty() {
            if cursor != raw.len() {
                return Err(TrailingHeaderDataError::new().into());
            }
            return Ok(headers);
        }

        let separator = line
            .iter()
            .position(|byte| *byte == b':')
            .filter(|separator| *separator != 0)
            .ok_or_else(MissingNameValueSeparatorError::new)?;
        let name = header_name(&line[..separator])?;
        let value = header_value(&line[separator + 1..])?;
        headers.append(name, value);
    }
}

fn take_crlf_line<'a>(raw: &'a [u8], cursor: &mut usize) -> Option<&'a [u8]> {
    let remaining = raw.get(*cursor..)?;
    let end = remaining.windows(2).position(|pair| pair == b"\r\n")?;
    let start = *cursor;
    // Advance by at least the CRLF so a mutated `+=` cannot leave the cursor
    // stuck and hang the parser (AGENTS.md, "Code must not hang even under
    // mutation testing").
    let next = start.checked_add(end)?.checked_add(2)?;
    debug_assert!(next > start, "CRLF line consumption must advance the cursor");
    *cursor = next;

    raw.get(start..start + end)
}

fn header_name(bytes: &[u8]) -> Result<HeaderName, ResponseHeadersError> {
    if let Some(byte) = bytes.iter().copied().find(|byte| !byte.is_ascii()) {
        return Err(NonAsciiHeaderNameError::new(byte).into());
    }

    HeaderName::from_bytes(bytes).map_err(|error| InvalidHeaderNameError::new(error.to_string()).into())
}

fn header_value(bytes: &[u8]) -> Result<HeaderValue, ResponseHeadersError> {
    let bytes = trim_optional_whitespace(bytes);

    HeaderValue::from_bytes(bytes).map_err(|error| InvalidHeaderValueError::new(error.to_string()).into())
}

fn trim_optional_whitespace(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(|byte| matches!(*byte, b' ' | b'\t')) {
        // Indexing from 1 shrinks the slice; a mutated subtract-from-len form is
        // not used here so the loop cannot stall (AGENTS.md, "Code must not hang
        // even under mutation testing").
        bytes = bytes.get(1..).expect("first() proved the slice is non-empty");
    }
    while bytes.last().is_some_and(|byte| matches!(*byte, b' ' | b'\t')) {
        bytes = bytes
            .get(..bytes.len().saturating_sub(1))
            .expect("last() proved the slice is non-empty");
    }

    bytes
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use http::HeaderValue;
    use static_assertions::assert_not_impl_any;

    use super::{
        InvalidHeaderNameError, InvalidHeaderValueError, InvalidStatusLineError, MissingHeaderTerminatorError,
        MissingNameValueSeparatorError, NonAsciiHeaderNameError, ResponseHeadersError, TrailingHeaderDataError, parse_response_headers,
        parse_response_trailers,
    };

    // Every `ohno` error owns a boxed source without unwind-safety bounds.
    assert_not_impl_any!(ResponseHeadersError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(InvalidHeaderNameError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(InvalidHeaderValueError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(InvalidStatusLineError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(MissingHeaderTerminatorError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(MissingNameValueSeparatorError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(NonAsciiHeaderNameError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(TrailingHeaderDataError: UnwindSafe, RefUnwindSafe);

    #[test]
    fn a_well_formed_block_preserves_order_repeats_and_opaque_bytes() {
        let raw = [
            b"HTTP/1.1 200 OK\r\ncontent-length: 123\r\nx-duplicate: one\r\nx-duplicate: ".as_slice(),
            &[0x80, 0xff],
            b"\r\n\r\n",
        ]
        .concat();

        let headers = parse_response_headers(&raw).unwrap();

        assert_eq!(headers.get("content-length"), Some(&HeaderValue::from_static("123")));
        assert_eq!(
            headers.get_all("x-duplicate").iter().map(HeaderValue::as_bytes).collect::<Vec<_>>(),
            [b"one".as_slice(), &[0x80, 0xff]]
        );
    }

    #[test]
    fn an_empty_block_terminator_yields_no_headers() {
        assert!(parse_response_headers(b"HTTP/1.1 204 No Content\r\n\r\n").unwrap().is_empty());
        assert!(parse_response_trailers(b"\r\n").unwrap().is_empty());
    }

    #[test]
    fn optional_whitespace_around_a_value_is_trimmed_but_interior_spacing_is_kept() {
        let headers = parse_response_headers(b"HTTP/1.1 200 OK\r\nx-spaced: \t padded value \t\r\n\r\n").unwrap();

        assert_eq!(headers.get("x-spaced"), Some(&HeaderValue::from_static("padded value")));
    }

    #[test]
    fn a_trailer_block_parses_without_a_status_line() {
        let headers = parse_response_trailers(b"grpc-status: 0\r\ngrpc-message: ok\r\n\r\n").unwrap();

        assert_eq!(headers.get("grpc-status"), Some(&HeaderValue::from_static("0")));
        assert_eq!(headers.get("grpc-message"), Some(&HeaderValue::from_static("ok")));
    }

    #[test]
    fn a_status_line_is_required_and_must_announce_http() {
        for raw in [b"".as_slice(), b"200 OK\r\n\r\n", b"HTTP/1.1 200 OK"] {
            let error = parse_response_headers(raw).unwrap_err();

            assert!(error.to_string().contains("malformed response status line"), "{error}");
        }

        // A trailer block has no status line, so the same bytes that fail as a
        // header block are simply an unterminated field block here.
        let error = parse_response_trailers(b"200 OK\r\n").unwrap_err();
        assert!(error.to_string().contains("without a ':' separator"), "{error}");
    }

    #[test]
    fn a_block_whose_final_line_lacks_crlf_is_unterminated() {
        let error = parse_response_headers(b"HTTP/1.1 200 OK\r\nx-name: value\r\n").unwrap_err();

        assert!(error.to_string().contains("without a terminating empty line"), "{error}");
    }

    #[test]
    fn a_bare_carriage_return_does_not_terminate_a_line() {
        let error = parse_response_headers(b"HTTP/1.1 200 OK\r\nx-name: value\r\r").unwrap_err();

        assert!(error.to_string().contains("without a terminating empty line"), "{error}");
    }

    #[test]
    fn a_field_line_requires_a_nonempty_name_and_a_separator() {
        let error = parse_response_headers(b"HTTP/1.1 200 OK\r\nmissing-colon\r\n\r\n").unwrap_err();
        assert!(error.to_string().contains("without a ':' separator"), "{error}");

        // An empty name is indistinguishable from a leading separator, so the
        // separator search rejects position zero rather than producing a
        // nameless field.
        let error = parse_response_headers(b"HTTP/1.1 200 OK\r\n: value\r\n\r\n").unwrap_err();
        assert!(error.to_string().contains("without a ':' separator"), "{error}");
    }

    #[test]
    fn field_names_must_be_ascii_and_valid_http_tokens() {
        let raw = [b"HTTP/1.1 200 OK\r\n".as_slice(), &[0x80], b": value\r\n\r\n"].concat();
        let error = parse_response_headers(&raw).unwrap_err();
        assert!(error.to_string().contains("non-ASCII response header name byte: 0x80"), "{error}");

        let error = parse_response_headers(b"HTTP/1.1 200 OK\r\nbad name: value\r\n\r\n").unwrap_err();
        assert!(error.to_string().contains("invalid response header name"), "{error}");
    }

    #[test]
    fn field_values_must_be_representable_by_http() {
        let error = parse_response_headers(b"HTTP/1.1 200 OK\r\nx-invalid: contains\nnewline\r\n\r\n").unwrap_err();

        assert!(error.to_string().contains("invalid response header value"), "{error}");
    }

    #[test]
    fn bytes_after_the_terminator_are_rejected() {
        let error = parse_response_headers(b"HTTP/1.1 200 OK\r\n\r\nleftover\r\n").unwrap_err();

        assert!(error.to_string().contains("data after the response header terminator"), "{error}");
    }
}
