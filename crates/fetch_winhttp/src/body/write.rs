// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::future::poll_fn;
use std::pin::Pin;
use std::ptr::NonNull;

use bytesbuf::mem::{GlobalPool, HasMemory, Memory, MemoryShared};
use bytesbuf::{BytesBuf, BytesView};
use bytesbuf_io::Write;
use fetch::{HttpBody, HttpError, RecoveryInfo};
use http::header::{CONTENT_LENGTH, TRANSFER_ENCODING};
use http::{HeaderMap, HeaderValue};
use http_body::Body as _;

use crate::bindings::{Bindings as _, BindingsFacade};
use crate::context::{CompletionResult, OperationBuffer, OperationKind};
use crate::error_labels;
use crate::options::WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH;
use crate::request::RequestGuard;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Selects the `WinHTTP` framing strategy for one request body.
///
/// Known lengths that fit a `DWORD` are passed directly to
/// `WinHttpSendRequest`. Larger known bodies use the ignore-total sentinel and
/// require an exact normalized 64-bit `Content-Length` header. Unknown lengths
/// enable `WinHTTP` automatic chunking and require a final zero-length write
/// (implementation.md section 6.1).
///
/// Constructing the framing strategy is also the single point where the
/// caller's framing metadata is reconciled with the framing `WinHTTP` will
/// actually perform, before any handle is opened: every `Content-Length` header
/// must agree with the true body length and is collapsed into one canonical
/// decimal value, and a caller-supplied `Transfer-Encoding` is rejected. This
/// keeps exactly one framing directive on the wire.
pub(crate) struct RequestBodyFraming {
    total_length: u32,
    automatic_chunking: bool,
}

impl RequestBodyFraming {
    pub(crate) fn new(headers: &mut HeaderMap, content_length: Option<u64>) -> Result<Self, RequestBodyFramingError> {
        // WinHTTP always performs the request framing itself: it derives the
        // wire `Content-Length` from `dwTotalLength`, or chunks the body when
        // automatic chunking is enabled (implementation.md section 6.1). A
        // caller-supplied transfer coding can therefore never be honored - the
        // caller's already-encoded bytes would be encoded a second time - and
        // forwarding the header would put a second framing directive next to
        // WinHTTP's own, which RFC 9112 section 6.1 resolves in favor of
        // `Transfer-Encoding` and which is the classic request smuggling
        // primitive. Rejecting rather than stripping follows design.md
        // section 5, which fails requests carrying body metadata the transport
        // cannot honor instead of silently dropping it.
        if headers.contains_key(TRANSFER_ENCODING) {
            return Err(RequestBodyFramingError::UnsupportedTransferEncoding);
        }

        let content_length = match content_length {
            Some(length) => Some(length),
            None => Self::declared_content_length(headers)?,
        };

        match content_length {
            Some(length) => {
                let total_length = match u32::try_from(length) {
                    Ok(total_length) => {
                        // `dwTotalLength` is authoritative here, so a
                        // disagreeing caller header would travel next to a
                        // contradicting WinHTTP-generated length.
                        reconcile_content_length(
                            headers,
                            length,
                            RequestBodyFramingError::MismatchedContentLength { expected: length },
                        )?;
                        total_length
                    }
                    Err(_too_large) => {
                        reconcile_content_length(
                            headers,
                            length,
                            RequestBodyFramingError::InvalidLargeContentLength { expected: length },
                        )?;
                        // `dwTotalLength` cannot represent this length, so the
                        // header is the only framing source and must be present
                        // even when the caller supplied none.
                        if !headers.contains_key(CONTENT_LENGTH) {
                            headers.insert(CONTENT_LENGTH, canonical_content_length(length));
                        }
                        WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH
                    }
                };

                Ok(Self {
                    total_length,
                    automatic_chunking: false,
                })
            }
            None => Ok(Self {
                total_length: WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH,
                automatic_chunking: true,
            }),
        }
    }

    /// Reads the body length declared by a `Content-Length` header.
    ///
    /// This is the fallback for bodies that cannot report their own length.
    /// Duplicate headers must all decode to the same value; otherwise the
    /// declared length is ambiguous and no framing strategy can be chosen.
    /// Normalization is left to [`reconcile_content_length`], which runs for
    /// every known length regardless of where that length came from.
    fn declared_content_length(headers: &HeaderMap) -> Result<Option<u64>, RequestBodyFramingError> {
        let mut values = headers.get_all(CONTENT_LENGTH).iter();
        let Some(first) = values.next() else {
            return Ok(None);
        };
        let Some(length) = parse_content_length(first) else {
            return Err(RequestBodyFramingError::InvalidContentLength);
        };

        if values.any(|value| parse_content_length(value) != Some(length)) {
            return Err(RequestBodyFramingError::InvalidContentLength);
        }

        Ok(Some(length))
    }

    pub(crate) const fn total_length(self) -> u32 {
        self.total_length
    }

    pub(crate) const fn automatic_chunking(self) -> bool {
        self.automatic_chunking
    }
}

/// Reconciles caller-supplied `Content-Length` headers with the body length.
///
/// Any header value that disagrees with `expected` fails the request with
/// `mismatch`, because the caller would otherwise put a framing directive on
/// the wire that contradicts what WinHTTP sends.
///
/// A header that survives is rewritten as one canonical decimal value, so a set
/// of duplicate or non-canonical values (`007`, repeated values) collapses into
/// the single length WinHTTP is told to send. A caller that supplied no header
/// is left without one, because `WinHttpSendRequest` emits the header from
/// `dwTotalLength` in that case.
fn reconcile_content_length(
    headers: &mut HeaderMap,
    expected: u64,
    mismatch: RequestBodyFramingError,
) -> Result<(), RequestBodyFramingError> {
    if !headers.contains_key(CONTENT_LENGTH) {
        return Ok(());
    }

    if headers
        .get_all(CONTENT_LENGTH)
        .iter()
        .any(|value| parse_content_length(value) != Some(expected))
    {
        return Err(mismatch);
    }

    headers.insert(CONTENT_LENGTH, canonical_content_length(expected));

    Ok(())
}

fn canonical_content_length(length: u64) -> HeaderValue {
    HeaderValue::from_str(&length.to_string()).expect("a decimal u64 contains only ASCII digits and is always a valid HTTP header value")
}

fn parse_content_length(value: &HeaderValue) -> Option<u64> {
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    bytes.iter().try_fold(0_u64, |length, byte| {
        byte.is_ascii_digit()
            .then_some(())
            .and_then(|()| length.checked_mul(10))
            .and_then(|length| length.checked_add(u64::from(*byte - b'0')))
    })
}

fn write_completion(completion: CompletionResult) -> Result<BytesView, HttpError> {
    match completion {
        CompletionResult::WriteComplete { mut buffer, len: written } if written != 0 => {
            buffer.advance(written as usize);
            Ok(buffer)
        }
        CompletionResult::WriteComplete { .. } => Err(callback_protocol_error(
            "WinHTTP completed a nonempty request write without writing any bytes",
        )),
        CompletionResult::Error { error, .. } => Err(error.into_http_error()),
        CompletionResult::InvalidStatusInfo { status, len, .. } => Err(callback_protocol_error(format!(
            "WinHTTP returned invalid status information for callback 0x{status:08x} with {len} bytes"
        ))),
        unexpected => Err(callback_protocol_error(format!(
            "WinHTTP returned an unexpected completion for Write: {unexpected:?}"
        ))),
    }
}

fn end_chunked_completion(completion: CompletionResult) -> Result<(), HttpError> {
    match completion {
        CompletionResult::WriteComplete { len: 0, .. } => Ok(()),
        CompletionResult::WriteComplete { len, .. } => Err(callback_protocol_error(format!(
            "WinHTTP reported {len} bytes for the final zero-length request write"
        ))),
        CompletionResult::Error { error, .. } => Err(error.into_http_error()),
        CompletionResult::InvalidStatusInfo { status, len, .. } => Err(callback_protocol_error(format!(
            "WinHTTP returned invalid status information for callback 0x{status:08x} with {len} bytes"
        ))),
        unexpected => Err(callback_protocol_error(format!(
            "WinHTTP returned an unexpected completion for the final request write: {unexpected:?}"
        ))),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Identifies request framing metadata that cannot define safe upload framing.
///
/// The writer must know whether `WinHTTP` receives an exact `DWORD` total, an
/// explicit 64-bit length, or an unknown-length stream. Inconsistent, malformed
/// or untruthful `Content-Length` values make that choice ambiguous, and a
/// caller-supplied `Transfer-Encoding` contradicts the framing `WinHTTP`
/// performs. Both are rejected before any handle is opened, so a request never
/// reaches the wire carrying two framing directives.
pub(crate) enum RequestBodyFramingError {
    InvalidContentLength,
    MismatchedContentLength { expected: u64 },
    InvalidLargeContentLength { expected: u64 },
    UnsupportedTransferEncoding,
}

impl fmt::Display for RequestBodyFramingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidContentLength => f.write_str("the request Content-Length header is not one consistent decimal u64 value"),
            Self::MismatchedContentLength { expected } => write!(
                f,
                "the request Content-Length header must equal the exact request body length ({expected}) or be omitted"
            ),
            Self::InvalidLargeContentLength { expected } => write!(
                f,
                "a request body larger than u32::MAX requires Content-Length to equal its exact 64-bit length ({expected})"
            ),
            Self::UnsupportedTransferEncoding => {
                f.write_str("WinHTTP frames the request body itself, so a request Transfer-Encoding header must be removed")
            }
        }
    }
}

impl std::error::Error for RequestBodyFramingError {}

#[derive(Debug)]
/// Streams request frames through sequential WinHTTP write operations.
///
/// Every submitted span remains owned by the callback operation slot until
/// `WRITE_COMPLETE`, so WinHTTP never observes freed or mutable storage.
/// Segmented views are written one contiguous span at a time, and spans larger
/// than `u32::MAX` are split into multiple operations. Unknown-length uploads
/// finish with the required null-buffer, zero-length write before response
/// reception begins. Request trailer frames are rejected because WinHTTP has no
/// corresponding submission API.
pub(crate) struct WinHttpBodyWriter<'guard> {
    guard: &'guard mut RequestGuard,
    bindings: BindingsFacade,
    memory: GlobalPool,
}

impl<'guard> WinHttpBodyWriter<'guard> {
    pub(crate) const fn new(guard: &'guard mut RequestGuard, bindings: BindingsFacade, memory: GlobalPool) -> Self {
        Self { guard, bindings, memory }
    }

    async fn write_view(&mut self, mut data: BytesView) -> fetch::Result<()> {
        while !data.is_empty() {
            let span = data.first_slice();
            debug_assert!(!span.is_empty(), "a nonempty BytesView must have a nonempty first span");

            let len = next_write_len(span.len() as u64).expect("a nonempty span always produces a write length");
            let buffer = NonNull::from(span).cast::<u8>();
            let bindings = self.bindings.clone();
            let write = self
                .guard
                .submit(OperationKind::Write, OperationBuffer::write(data, len), move |request, _context| {
                    // SAFETY: submit() armed the write operation and
                    // transferred the live request handle into its future. The
                    // exact BytesView and contiguous span identified by
                    // buffer/len remain in the active operation until
                    // completion, and no operation overlaps this write.
                    unsafe { bindings.write_data(request, Some(buffer), len) }
                });
            let completion = write
                .await
                .map_err(|_disconnected| callback_protocol_error("the write completion channel disconnected"))?;

            data = write_completion(completion)?;
        }

        Ok(())
    }

    pub(crate) async fn end_automatic_chunking(&mut self) -> fetch::Result<()> {
        let bindings = self.bindings.clone();
        let write = self.guard.submit(
            OperationKind::Write,
            OperationBuffer::write(BytesView::new(), 0),
            move |request, _context| {
                // SAFETY: submit() armed the sole write operation and
                // transferred the live request handle into its future. WinHTTP
                // accepts a null buffer only for this zero-length end-of-body
                // write, whose completion is awaited before response receipt.
                unsafe { bindings.write_data(request, None, 0) }
            },
        );
        let completion = write
            .await
            .map_err(|_disconnected| callback_protocol_error("the final request write completion channel disconnected"))?;

        end_chunked_completion(completion)
    }
}

impl Memory for WinHttpBodyWriter<'_> {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.memory.reserve(min_bytes)
    }
}

impl HasMemory for WinHttpBodyWriter<'_> {
    fn memory(&self) -> impl MemoryShared {
        self.memory.clone()
    }
}

impl Write for WinHttpBodyWriter<'_> {
    type Error = HttpError;

    async fn write(&mut self, data: BytesView) -> Result<(), Self::Error> {
        self.write_view(data).await
    }
}

pub(crate) async fn send_body(body: &mut HttpBody, writer: &mut WinHttpBodyWriter<'_>, body_polled: &mut bool) -> fetch::Result<()> {
    loop {
        let frame = poll_fn(|cx| {
            *body_polled = true;
            Pin::new(&mut *body).poll_frame(cx)
        })
        .await;

        match frame {
            None => return Ok(()),
            Some(Err(error)) => return Err(error),
            Some(Ok(frame)) => match frame.into_data() {
                Ok(data) => writer.write(data).await?,
                Err(frame) => {
                    frame
                        .into_trailers()
                        .map_err(|_unsupported| invalid_request(RequestBodyError::UnsupportedFrame))?;
                    return Err(invalid_request(RequestBodyError::TrailersUnsupported));
                }
            },
        }
    }
}

fn next_write_len(remaining: u64) -> Option<u32> {
    if remaining == 0 {
        return None;
    }

    Some(u32::try_from(remaining.min(u64::from(u32::MAX))).expect("the write length is bounded by u32::MAX"))
}

fn invalid_request(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> HttpError {
    HttpError::other(error, RecoveryInfo::never(), error_labels::INVALID_REQUEST)
}

fn callback_protocol_error(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> HttpError {
    HttpError::other(error, RecoveryInfo::never(), error_labels::REQUEST_WINHTTP)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Identifies body frame shapes that WinHTTP cannot submit.
enum RequestBodyError {
    TrailersUnsupported,
    UnsupportedFrame,
}

impl fmt::Display for RequestBodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrailersUnsupported => f.write_str("WinHTTP cannot submit request trailer frames"),
            Self::UnsupportedFrame => f.write_str("the request body produced an unsupported HTTP body frame"),
        }
    }
}

impl std::error::Error for RequestBodyError {}

#[cfg(test)]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use bytesbuf::BytesView;
    use bytesbuf::mem::GlobalPool;
    use http::HeaderValue;
    use ohno::Labeled as _;
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::{
        RequestBodyError, RequestBodyFraming, RequestBodyFramingError, WinHttpBodyWriter, end_chunked_completion, next_write_len,
        write_completion,
    };
    use crate::context::CompletionResult;
    use crate::options::WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH;

    assert_impl_all!(RequestBodyFraming: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RequestBodyFramingError: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RequestBodyError: UnwindSafe, RefUnwindSafe);
    // The writer mutably borrows a guard pointing at the callback context's UnsafeCell state.
    assert_not_impl_any!(WinHttpBodyWriter<'static>: UnwindSafe, RefUnwindSafe);

    #[test]
    fn request_body_framing_maps_known_and_unknown_lengths() {
        let mut headers = http::HeaderMap::new();
        let small = RequestBodyFraming::new(&mut headers, Some(42)).unwrap();
        assert_eq!(small.total_length(), 42);
        assert!(!small.automatic_chunking());

        let large_length = u64::from(u32::MAX) + 1;
        let large = RequestBodyFraming::new(&mut headers, Some(large_length)).unwrap();
        assert_eq!(large.total_length(), WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH);
        assert!(!large.automatic_chunking());
        assert_eq!(
            headers.get(http::header::CONTENT_LENGTH),
            Some(&HeaderValue::from_static("4294967296"))
        );

        headers.remove(http::header::CONTENT_LENGTH);
        let unknown = RequestBodyFraming::new(&mut headers, None).unwrap();
        assert_eq!(unknown.total_length(), WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH);
        assert!(unknown.automatic_chunking());
    }

    #[test]
    fn known_length_reconciles_the_caller_supplied_content_length() {
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::CONTENT_LENGTH, HeaderValue::from_static("005"));
        headers.append(http::header::CONTENT_LENGTH, HeaderValue::from_static("5"));

        let framing = RequestBodyFraming::new(&mut headers, Some(5)).unwrap();

        assert_eq!(framing.total_length(), 5);
        assert!(!framing.automatic_chunking());
        assert_eq!(
            headers.get_all(http::header::CONTENT_LENGTH).iter().collect::<Vec<_>>(),
            [&HeaderValue::from_static("5")]
        );

        for disagreeing in ["99", "4", "not-a-number", "18446744073709551616"] {
            let mut headers = http::HeaderMap::new();
            headers.insert(http::header::CONTENT_LENGTH, HeaderValue::from_str(disagreeing).unwrap());

            assert_eq!(
                RequestBodyFraming::new(&mut headers, Some(5)),
                Err(RequestBodyFramingError::MismatchedContentLength { expected: 5 })
            );
        }

        let mut headers = http::HeaderMap::new();
        headers.append(http::header::CONTENT_LENGTH, HeaderValue::from_static("5"));
        headers.append(http::header::CONTENT_LENGTH, HeaderValue::from_static("6"));
        assert_eq!(
            RequestBodyFraming::new(&mut headers, Some(5)),
            Err(RequestBodyFramingError::MismatchedContentLength { expected: 5 })
        );
    }

    #[test]
    fn a_known_length_body_without_a_header_stays_without_one() {
        let mut headers = http::HeaderMap::new();

        let framing = RequestBodyFraming::new(&mut headers, Some(5)).unwrap();

        assert_eq!(framing.total_length(), 5);
        assert!(!headers.contains_key(http::header::CONTENT_LENGTH));
    }

    #[test]
    fn a_caller_supplied_transfer_encoding_is_rejected_before_a_framing_mode_is_selected() {
        // The rejection is the first statement in `RequestBodyFraming::new`, so today all three
        // content lengths below take an identical path and no framing mode is ever computed -
        // that is exactly what this test asserts. The parameterization is retained as a guard:
        // if a future refactor moves the check into a per-mode branch of the length match, the
        // unknown-length and above-`DWORD` cases would stop rejecting and this test would fail.
        for content_length in [None, Some(5), Some(u64::from(u32::MAX) + 1)] {
            let mut headers = http::HeaderMap::new();
            headers.insert(http::header::TRANSFER_ENCODING, HeaderValue::from_static("chunked"));

            assert_eq!(
                RequestBodyFraming::new(&mut headers, content_length),
                Err(RequestBodyFramingError::UnsupportedTransferEncoding)
            );
        }
    }

    #[test]
    fn large_content_length_is_validated_and_canonicalized() {
        let expected = u64::from(u32::MAX) + 1;
        let mut headers = http::HeaderMap::new();
        headers.append(http::header::CONTENT_LENGTH, HeaderValue::from_static("04294967296"));
        headers.append(http::header::CONTENT_LENGTH, HeaderValue::from_static("4294967296"));

        RequestBodyFraming::new(&mut headers, Some(expected)).unwrap();

        assert_eq!(
            headers.get_all(http::header::CONTENT_LENGTH).iter().collect::<Vec<_>>(),
            [&HeaderValue::from_static("4294967296")]
        );

        for invalid in ["4294967295", "not-a-number", "18446744073709551616"] {
            let mut headers = http::HeaderMap::new();
            headers.insert(http::header::CONTENT_LENGTH, HeaderValue::from_str(invalid).unwrap());

            assert_eq!(
                RequestBodyFraming::new(&mut headers, Some(expected)),
                Err(RequestBodyFramingError::InvalidLargeContentLength { expected })
            );
        }
    }

    #[test]
    fn content_length_header_declares_an_otherwise_unknown_body_length() {
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::CONTENT_LENGTH, HeaderValue::from_static("10"));

        let framing = RequestBodyFraming::new(&mut headers, None).unwrap();
        assert_eq!(framing.total_length(), 10);
        assert!(!framing.automatic_chunking());

        for invalid in ["invalid", "18446744073709551616"] {
            let mut headers = http::HeaderMap::new();
            headers.insert(http::header::CONTENT_LENGTH, HeaderValue::from_str(invalid).unwrap());
            assert_eq!(
                RequestBodyFraming::new(&mut headers, None),
                Err(RequestBodyFramingError::InvalidContentLength)
            );
        }

        let mut headers = http::HeaderMap::new();
        headers.append(http::header::CONTENT_LENGTH, HeaderValue::from_static("10"));
        headers.append(http::header::CONTENT_LENGTH, HeaderValue::from_static("11"));
        assert_eq!(
            RequestBodyFraming::new(&mut headers, None),
            Err(RequestBodyFramingError::InvalidContentLength)
        );
    }

    #[test]
    fn write_length_splitting_covers_dword_boundaries_without_buffers() {
        fn split(mut remaining: u64) -> Vec<u32> {
            let mut chunks = Vec::new();
            while let Some(next) = next_write_len(remaining) {
                chunks.push(next);
                remaining -= u64::from(next);
            }
            chunks
        }

        assert_eq!(split(0), []);
        assert_eq!(split(1), [1]);
        assert_eq!(split(u64::from(u32::MAX)), [u32::MAX]);
        assert_eq!(split(u64::from(u32::MAX) + 1), [u32::MAX, 1]);
        assert_eq!(split(u64::from(u32::MAX) * 2), [u32::MAX, u32::MAX]);
    }

    #[test]
    fn unexpected_write_completions_are_rejected() {
        let error = write_completion(CompletionResult::HeadersAvailable).unwrap_err();

        assert_eq!(error.label(), "request_winhttp");
        assert!(error.to_string().contains("unexpected completion for Write"));

        let error = write_completion(CompletionResult::WriteComplete {
            buffer: BytesView::copied_from_slice(b"x", &GlobalPool::new()),
            len: 0,
        })
        .unwrap_err();

        assert_eq!(error.label(), "request_winhttp");
        assert!(error.to_string().contains("without writing any bytes"));

        end_chunked_completion(CompletionResult::WriteComplete {
            buffer: BytesView::new(),
            len: 0,
        })
        .unwrap();

        let error = end_chunked_completion(CompletionResult::WriteComplete {
            buffer: BytesView::new(),
            len: 1,
        })
        .unwrap_err();
        assert_eq!(error.label(), "request_winhttp");
        assert!(error.to_string().contains("reported 1 bytes"));
    }
}
