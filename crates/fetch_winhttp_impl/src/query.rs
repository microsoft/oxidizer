// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Reads response metadata out of a live `WinHTTP` request handle.
//!
//! `WinHTTP` delivers response headers, the status code, and the negotiated
//! protocol through `WinHttpQueryHeaders` and `WinHttpQueryOption` rather than
//! through the completion callback. Those two entry points are synchronous
//! even on an asynchronous session, so this module is one of the few places
//! that calls into the operating system without arming an operation
//! (implementation.md section 2.1). Every function here therefore requires the
//! request handle to be live with no asynchronous operation outstanding, which
//! in practice means the caller holds the `RequestGuard` and is between
//! completions (implementation.md section 6.3).
//!
//! Each query pairs a native call with the validation defined in
//! [`crate::convert`]: `WinHTTP` reports lengths and buffer requirements
//! separately from the data, and a value that survives the call still has to
//! be proven well-formed before it becomes a response field. The two failure
//! categories stay distinguishable through [`QueryError`], because
//! [`crate::error::query_error`] maps an operating-system failure and
//! malformed native output to different `HttpError` classifications
//! (design.md section 7).

use std::fmt;
use std::ptr::NonNull;

use http::Version;
use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows::Win32::Networking::WinHttp::{ERROR_WINHTTP_HEADER_NOT_FOUND, WINHTTP_QUERY_FLAG_TRAILERS};

use crate::bindings::{
    Bindings as _, BindingsFacade, WINHTTP_OPTION_HTTP_PROTOCOL_USED, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_FLAG_WIRE_ENCODING,
    WINHTTP_QUERY_RAW_HEADERS_CRLF, WINHTTP_QUERY_STATUS_CODE, WINHTTP_QUERY_VERSION,
};
use crate::convert::{
    ConversionError, ReturnedLengthOutOfBoundsError, UnexpectedByteLengthError, dword_to_usize, header_buffer_units, parse_header_buffer,
    parse_protocol_used,
};
use crate::error::WinHttpError;
use crate::handle::RawHandle;

/// Byte length of a native `DWORD`, the width of every fixed-size query here.
///
/// `WinHttpQueryHeaders` with `WINHTTP_QUERY_FLAG_NUMBER` and the
/// `WINHTTP_OPTION_HTTP_PROTOCOL_USED` option both write exactly one `DWORD`
/// and report the length they wrote, so this value serves as both the supplied
/// capacity and the expected returned length.
const DWORD_BYTES: u32 = 4;

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

/// Every operand is a distinct bit, so `|` and `^` compute the same value and a
/// mutation between them is equivalent rather than a defect.
#[cfg_attr(test, mutants::skip)] // Disjoint-bit union: `|` and `^` are interchangeable, so operator mutants are equivalent.
const fn status_code_query_flags() -> u32 {
    WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER
}

/// Every operand is a distinct bit, so `|` and `^` compute the same value and a
/// mutation between them is equivalent rather than a defect.
#[cfg_attr(test, mutants::skip)] // Disjoint-bit union: `|` and `^` are interchangeable, so operator mutants are equivalent.
const fn raw_headers_query_flags() -> u32 {
    WINHTTP_QUERY_RAW_HEADERS_CRLF | WINHTTP_QUERY_FLAG_WIRE_ENCODING
}

/// Every operand is a distinct bit, so `|` and `^` compute the same value and a
/// mutation between them is equivalent rather than a defect.
#[cfg_attr(test, mutants::skip)] // Disjoint-bit union: `|` and `^` are interchangeable, so operator mutants are equivalent.
const fn raw_trailers_query_flags() -> u32 {
    WINHTTP_QUERY_RAW_HEADERS_CRLF | WINHTTP_QUERY_FLAG_TRAILERS | WINHTTP_QUERY_FLAG_WIRE_ENCODING
}

pub(crate) fn query_status_code(bindings: &BindingsFacade, request: RawHandle) -> Result<u32, QueryError> {
    let mut status_code = 0_u32;
    let mut byte_len = DWORD_BYTES;
    let buffer = NonNull::from(&mut status_code).cast();

    // SAFETY: callers query only a live request while its RequestGuard owns the
    // handle and no asynchronous operation is outstanding. status_code is a
    // writable DWORD and byte_len describes its exact capacity.
    unsafe { bindings.query_headers(request, status_code_query_flags(), Some(buffer), &mut byte_len) }?;

    if byte_len != DWORD_BYTES {
        return Err(ConversionError::from(UnexpectedByteLengthError::new("HTTP status code", DWORD_BYTES, byte_len)).into());
    }

    Ok(status_code)
}

pub(crate) fn query_raw_headers(bindings: &BindingsFacade, request: RawHandle) -> Result<Vec<u8>, QueryError> {
    query_header_bytes(bindings, request, raw_headers_query_flags())
}

pub(crate) fn query_raw_trailers(bindings: &BindingsFacade, request: RawHandle) -> Result<Option<Vec<u8>>, QueryError> {
    match query_header_bytes(bindings, request, raw_trailers_query_flags()) {
        Err(QueryError::WinHttp(error)) if error.code() == ERROR_WINHTTP_HEADER_NOT_FOUND => Ok(None),
        // A successful zero-length block means no trailers were present; the
        // trailer parser rejects an empty buffer, so treat it as absent rather
        // than as a malformed trailer block after the body completed.
        Ok(raw) if raw.is_empty() => Ok(None),
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
    let mut required_bytes = 0_u32;

    // SAFETY: callers query only a live request while its RequestGuard owns the
    // handle and no asynchronous operation is outstanding. A null buffer with
    // zero capacity is the documented sizing query.
    match unsafe { bindings.query_headers(request, info_level, None, &mut required_bytes) } {
        Err(error) if error.code() == ERROR_INSUFFICIENT_BUFFER.0 => {}
        Err(error) => return Err(error.into()),
        Ok(()) if required_bytes == 0 => return Ok(String::new()),
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

    Ok(parse_header_buffer(&buffer, returned_bytes, value)?)
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::error::Error as _;
    use std::ffi::c_void;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::Arc;

    use http::Version;
    use mockall::Sequence;
    use ohno::ErrorExt as _;
    use static_assertions::assert_not_impl_any;

    use super::{
        ConversionError, DWORD_BYTES, ERROR_INSUFFICIENT_BUFFER, ERROR_WINHTTP_HEADER_NOT_FOUND, QueryError, UnexpectedByteLengthError,
        WINHTTP_OPTION_HTTP_PROTOCOL_USED, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_FLAG_TRAILERS, WINHTTP_QUERY_FLAG_WIRE_ENCODING,
        WINHTTP_QUERY_RAW_HEADERS_CRLF, WINHTTP_QUERY_STATUS_CODE, WINHTTP_QUERY_VERSION, query_protocol_used, query_raw_headers,
        query_raw_trailers, query_status_code,
    };
    use crate::bindings::{BindingsFacade, MockBindings};
    use crate::error::{WinHttpError, WinHttpOperation};
    use crate::handle::RawHandle;

    // The routing enumeration wraps errors that retain user-erased source state.
    assert_not_impl_any!(QueryError: UnwindSafe, RefUnwindSafe);

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
        let status = status
            .source()
            .and_then(|source| source.downcast_ref::<ConversionError>())
            .expect("a native buffer of the wrong width is a conversion failure, not a WinHTTP failure");
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
        let protocol = protocol
            .source()
            .and_then(|source| source.downcast_ref::<ConversionError>())
            .expect("a native buffer of the wrong width is a conversion failure, not a WinHTTP failure");
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

    #[test]
    fn empty_trailer_block_is_treated_as_absent() {
        let mut bindings = MockBindings::new();
        bindings
            .expect_query_headers()
            .withf(|_, info_level, buffer, byte_len| {
                *info_level == (WINHTTP_QUERY_RAW_HEADERS_CRLF | WINHTTP_QUERY_FLAG_TRAILERS | WINHTTP_QUERY_FLAG_WIRE_ENCODING)
                    && buffer.is_none()
                    && *byte_len == 0
            })
            .once()
            .returning(|_, _, _, byte_len| {
                *byte_len = 0;
                Ok(())
            });

        let trailers = query_raw_trailers(&BindingsFacade::mock(Arc::new(bindings)), raw_handle(1)).unwrap();

        assert_eq!(trailers, None);
    }

    #[test]
    fn the_routing_enumeration_forwards_display_and_source_to_the_wrapped_error() {
        let win_http = QueryError::from(WinHttpError::new(12002, WinHttpOperation::QueryHeaders));

        // The wrapper only routes; it must not decorate the error it carries.
        assert_eq!(
            win_http.to_string(),
            WinHttpError::new(12002, WinHttpOperation::QueryHeaders).to_string()
        );
        assert_eq!(win_http.source().unwrap().to_string(), win_http.to_string());

        let inner = ConversionError::from(UnexpectedByteLengthError::new("a status code", DWORD_BYTES, 3_u32));
        let expected = inner.to_string();
        let conversion = QueryError::from(inner);

        assert_eq!(conversion.to_string(), expected);
        assert_eq!(conversion.source().unwrap().to_string(), expected);
    }

    #[test]
    fn a_sizing_query_that_succeeds_with_no_bytes_yields_no_headers() {
        let mut bindings = MockBindings::new();
        bindings
            .expect_query_headers()
            .withf(|_, _, buffer, _| buffer.is_none())
            .once()
            .returning(|_, _, _, byte_len| {
                *byte_len = 0;
                Ok(())
            });

        assert!(
            query_raw_headers(&BindingsFacade::mock(Arc::new(bindings)), raw_handle(8))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_sizing_query_that_succeeds_with_a_length_still_reads_the_headers() {
        let mut sequence = Sequence::new();
        let mut bindings = MockBindings::new();
        bindings
            .expect_query_headers()
            .withf(|_, _, buffer, _| buffer.is_none())
            .once()
            .in_sequence(&mut sequence)
            .returning(|_, _, _, byte_len| {
                *byte_len = 4;
                Ok(())
            });
        bindings
            .expect_query_headers()
            .withf(|_, _, buffer, byte_len| buffer.is_some() && *byte_len == 4)
            .once()
            .in_sequence(&mut sequence)
            .returning(|_, _, buffer, byte_len| {
                let output = buffer.unwrap();
                // SAFETY: the caller sized this buffer for four bytes.
                unsafe { output.as_ptr().copy_from_nonoverlapping(b"ok\r\n".as_ptr(), 4) };
                *byte_len = 4;
                Ok(())
            });

        let headers = query_raw_headers(&BindingsFacade::mock(Arc::new(bindings)), raw_handle(9)).unwrap();

        assert_eq!(headers, b"ok\r\n");
    }

    #[test]
    fn a_header_read_returning_more_than_it_sized_for_is_rejected() {
        let mut sequence = Sequence::new();
        let mut bindings = MockBindings::new();
        bindings
            .expect_query_headers()
            .withf(|_, _, buffer, _| buffer.is_none())
            .once()
            .in_sequence(&mut sequence)
            .returning(|_, _, _, byte_len| {
                *byte_len = 8;
                Err(WinHttpError::new(ERROR_INSUFFICIENT_BUFFER.0, WinHttpOperation::QueryHeaders))
            });
        bindings
            .expect_query_headers()
            .withf(|_, _, buffer, _| buffer.is_some())
            .once()
            .in_sequence(&mut sequence)
            .returning(|_, _, _, byte_len| {
                // WinHTTP claims to have written more than the buffer holds.
                *byte_len = 4096;
                Ok(())
            });

        let error = query_raw_headers(&BindingsFacade::mock(Arc::new(bindings)), raw_handle(10)).unwrap_err();

        assert!(matches!(error, QueryError::Conversion(_)), "{error:?}");
    }

    #[test]
    fn a_version_sizing_query_that_succeeds_with_no_bytes_yields_no_version() {
        let mut sequence = Sequence::new();
        let mut bindings = MockBindings::new();
        expect_legacy_protocol_used(&mut bindings, &mut sequence);
        bindings
            .expect_query_headers()
            .withf(|_, info_level, buffer, _| *info_level == WINHTTP_QUERY_VERSION && buffer.is_none())
            .once()
            .in_sequence(&mut sequence)
            .returning(|_, _, _, byte_len| {
                *byte_len = 0;
                Ok(())
            });

        // An empty version string cannot name a supported protocol.
        let error = query_protocol_used(&BindingsFacade::mock(Arc::new(bindings)), raw_handle(11)).unwrap_err();

        assert!(matches!(error, QueryError::Conversion(_)), "{error:?}");
    }

    #[test]
    fn a_version_read_returning_more_than_it_sized_for_is_rejected() {
        let mut sequence = Sequence::new();
        let mut bindings = MockBindings::new();
        expect_legacy_protocol_used(&mut bindings, &mut sequence);
        bindings
            .expect_query_headers()
            .withf(|_, info_level, buffer, _| *info_level == WINHTTP_QUERY_VERSION && buffer.is_none())
            .once()
            .in_sequence(&mut sequence)
            .returning(|_, _, _, byte_len| {
                *byte_len = 8;
                Ok(())
            });
        bindings
            .expect_query_headers()
            .withf(|_, _, buffer, _| buffer.is_some())
            .once()
            .in_sequence(&mut sequence)
            .returning(|_, _, _, byte_len| {
                *byte_len = 4096;
                Ok(())
            });

        let error = query_protocol_used(&BindingsFacade::mock(Arc::new(bindings)), raw_handle(12)).unwrap_err();

        assert!(matches!(error, QueryError::Conversion(_)), "{error:?}");
    }

    #[test]
    fn a_version_containing_an_interior_zero_code_unit_is_rejected() {
        let mut sequence = Sequence::new();
        let mut bindings = MockBindings::new();
        expect_legacy_protocol_used(&mut bindings, &mut sequence);
        bindings
            .expect_query_headers()
            .withf(|_, info_level, buffer, _| *info_level == WINHTTP_QUERY_VERSION && buffer.is_none())
            .once()
            .in_sequence(&mut sequence)
            .returning(|_, _, _, byte_len| {
                *byte_len = 8;
                Err(WinHttpError::new(ERROR_INSUFFICIENT_BUFFER.0, WinHttpOperation::QueryHeaders))
            });
        bindings
            .expect_query_headers()
            .withf(|_, _, buffer, _| buffer.is_some())
            .once()
            .in_sequence(&mut sequence)
            .returning(|_, _, buffer, byte_len| {
                let output = buffer.unwrap().cast::<u16>();
                let units: [u16; 4] = [u16::from(b'H'), 0, u16::from(b'1'), u16::from(b'1')];
                // SAFETY: the caller sized this buffer for four UTF-16 units.
                unsafe { output.as_ptr().copy_from_nonoverlapping(units.as_ptr(), units.len()) };
                *byte_len = 8;
                Ok(())
            });

        let error = query_protocol_used(&BindingsFacade::mock(Arc::new(bindings)), raw_handle(13)).unwrap_err();

        assert!(matches!(error, QueryError::Conversion(_)), "{error:?}");
    }

    #[test]
    fn a_failed_version_sizing_query_reports_the_native_failure() {
        let mut sequence = Sequence::new();
        let mut bindings = MockBindings::new();
        expect_legacy_protocol_used(&mut bindings, &mut sequence);
        bindings
            .expect_query_headers()
            .withf(|_, info_level, buffer, _| *info_level == WINHTTP_QUERY_VERSION && buffer.is_none())
            .once()
            .in_sequence(&mut sequence)
            .returning(|_, _, _, _| Err(WinHttpError::new(12030, WinHttpOperation::QueryHeaders)));

        // Only an insufficient-buffer failure reports a size. Any other failure
        // means the header is unavailable, and the native code must survive
        // rather than be recast as malformed native output.
        let error = query_protocol_used(&BindingsFacade::mock(Arc::new(bindings)), raw_handle(14)).unwrap_err();

        assert!(
            matches!(error, QueryError::WinHttp(ref failure) if failure.code() == 12030),
            "{error:?}"
        );
    }

    /// Scripts the protocol-used option query to report the legacy value, which
    /// sends the caller on to the textual version header.
    fn expect_legacy_protocol_used(bindings: &mut MockBindings, sequence: &mut Sequence) {
        bindings
            .expect_query_option()
            .withf(|_, option, _, _| *option == WINHTTP_OPTION_HTTP_PROTOCOL_USED)
            .once()
            .in_sequence(sequence)
            .returning(|_, _, buffer, byte_len| {
                let protocol = buffer.unwrap().cast::<u32>();
                // SAFETY: query_protocol_used supplies a writable DWORD buffer.
                unsafe { protocol.as_ptr().write(0) };
                *byte_len = 4;
                Ok(())
            });
    }

    fn raw_handle(value: usize) -> RawHandle {
        RawHandle::new(value as *mut c_void).unwrap()
    }
}
