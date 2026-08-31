// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::future::poll_fn;
use std::num::{NonZeroU32, NonZeroU64};
use std::pin::Pin;
use std::ptr::NonNull;

use bytesbuf::mem::{GlobalPool, HasMemory, Memory, MemoryShared};
use bytesbuf::{BytesBuf, BytesView};
use bytesbuf_io::Write;
use fetch::{HttpBody, HttpError};
use http::header::{CONTENT_LENGTH, TRANSFER_ENCODING};
use http::{HeaderMap, HeaderValue};
use http_body::Body as _;

use crate::bindings::{Bindings as _, BindingsFacade, WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH};
use crate::context::{CompletionResult, OperationBuffer, OperationKind};
use crate::error::{callback_protocol_error, invalid_request};
use crate::operation::RequestGuard;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Selects the `WinHTTP` framing strategy for one request body.
///
/// Known lengths that fit a `DWORD` are passed directly to
/// `WinHttpSendRequest`. Larger known bodies use the ignore-total sentinel and
/// carry their length in a normalized 64-bit `Content-Length` header. Unknown
/// lengths enable `WinHTTP` automatic chunking and require a final zero-length
/// write (implementation.md section 6.1).
///
/// A zero-length body encodes as the sentinel value itself, so `WinHTTP` frames
/// it as no body rather than as a declared length of zero. Those describe the
/// same empty payload on the wire, so the collision needs no special handling
/// (implementation.md section 6.1).
///
/// Constructing the framing strategy is also the single point where the
/// caller's framing metadata is reconciled with the framing `WinHTTP` will
/// actually perform, before any handle is opened. A body that reports its own
/// length is authoritative: every `Content-Length` header must equal that
/// length, and a disagreement fails the request locally. A body that cannot
/// report one takes its declared length from the header instead, which the
/// transport frames against on the caller's word and does not check against the
/// bytes the body goes on to produce. Either way the surviving header is
/// collapsed into one canonical decimal value and a caller-supplied
/// `Transfer-Encoding` is rejected, so exactly one framing directive reaches
/// the wire.
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
            return Err(UnsupportedTransferEncodingError::new().into());
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
                        reconcile_content_length(headers, length, || MismatchedContentLengthError::new(length).into())?;
                        total_length
                    }
                    Err(_too_large) => {
                        reconcile_content_length(headers, length, || LargeContentLengthMismatchError::new(length).into())?;
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
    /// This is the fallback for bodies that cannot report their own length, so
    /// the value it returns is the caller's word and becomes the length
    /// `WinHTTP` frames against. Duplicate headers must all decode to the same
    /// value; otherwise the declared length is ambiguous and no framing
    /// strategy can be chosen. Normalization is left to
    /// [`reconcile_content_length`], which runs for every known length
    /// regardless of where that length came from.
    fn declared_content_length(headers: &HeaderMap) -> Result<Option<u64>, RequestBodyFramingError> {
        let mut values = headers.get_all(CONTENT_LENGTH).iter();
        let Some(first) = values.next() else {
            return Ok(None);
        };
        let Some(length) = parse_content_length(first) else {
            return Err(InconsistentContentLengthError::new().into());
        };

        if values.any(|value| parse_content_length(value) != Some(length)) {
            return Err(InconsistentContentLengthError::new().into());
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

/// Reconciles caller-supplied `Content-Length` headers with the length
/// `WinHTTP` frames against.
///
/// Any header value that disagrees with `expected` fails the request with the
/// error `mismatch` produces, because the caller would otherwise put a framing
/// directive on the wire that contradicts what `WinHTTP` sends. That comparison
/// discriminates where `expected` came from the body itself; where a header
/// supplied it, the values already agree and this call only collapses them into
/// the canonical form. The error is built lazily because an `ohno` error
/// allocates its source chain, and the overwhelmingly common case is a header
/// that agrees.
///
/// A header that survives is rewritten as one canonical decimal value, so a set
/// of duplicate or non-canonical values (`007`, repeated values) collapses into
/// the single length `WinHTTP` is told to send. A caller that supplied no header
/// is left without one, because `WinHttpSendRequest` emits the header from
/// `dwTotalLength` in that case.
fn reconcile_content_length(
    headers: &mut HeaderMap,
    expected: u64,
    mismatch: impl FnOnce() -> RequestBodyFramingError,
) -> Result<(), RequestBodyFramingError> {
    if !headers.contains_key(CONTENT_LENGTH) {
        return Ok(());
    }

    if headers
        .get_all(CONTENT_LENGTH)
        .iter()
        .any(|value| parse_content_length(value) != Some(expected))
    {
        return Err(mismatch());
    }

    headers.insert(CONTENT_LENGTH, canonical_content_length(expected));

    Ok(())
}

fn canonical_content_length(length: u64) -> HeaderValue {
    HeaderValue::from(length)
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
        CompletionResult::WriteComplete { mut buffer, len: written } => {
            // `NonZeroU32` rejects a no-progress completion without a bare
            // `== 0` / `!= 0` comparison a mutant can invert into an infinite
            // write loop (AGENTS.md, "Code must not hang even under mutation
            // testing").
            let written = NonZeroU32::new(written)
                .ok_or_else(|| callback_protocol_error("WinHTTP completed a nonempty request write without writing any bytes"))?;
            buffer.advance(usize::try_from(written.get()).expect("u32 fits usize"));
            Ok(buffer)
        }
        other => Err(other.into_failure("Write")),
    }
}

fn end_chunked_completion(completion: CompletionResult) -> Result<(), HttpError> {
    match completion {
        CompletionResult::WriteComplete { len: 0, .. } => Ok(()),
        CompletionResult::WriteComplete { len, .. } => Err(callback_protocol_error(format!(
            "WinHTTP reported {len} bytes for the final zero-length request write"
        ))),
        other => Err(other.into_failure("the final request write")),
    }
}

/// Identifies request framing metadata that cannot define safe upload framing.
///
/// The writer must know whether `WinHTTP` receives an exact `DWORD` total, an
/// explicit 64-bit length, or an unknown-length stream. Inconsistent or
/// malformed `Content-Length` values, and values that contradict a body which
/// knows its own length, make that choice ambiguous, and a caller-supplied
/// `Transfer-Encoding` contradicts the framing `WinHTTP` performs. Both are
/// rejected before any handle is opened, so a request never reaches the wire
/// carrying two framing directives.
///
/// One `ohno` source type per condition is rolled into this aggregate
/// (implementation.md section 1.1), so the framing code propagates a single
/// error type while the precise condition stays in the source chain.
#[ohno::error]
#[from(
    InconsistentContentLengthError,
    LargeContentLengthMismatchError,
    MismatchedContentLengthError,
    UnsupportedTransferEncodingError
)]
#[display("the request body framing metadata is unusable")]
pub(crate) struct RequestBodyFramingError;

/// Reports `Content-Length` headers that do not agree on one decimal `u64`.
#[ohno::error]
#[display("the request Content-Length header is not one consistent decimal u64 value")]
struct InconsistentContentLengthError;

/// Reports a `Content-Length` header that contradicts the true body length.
#[ohno::error]
#[display("the request Content-Length header must equal the exact request body length ({expected}) or be omitted")]
struct MismatchedContentLengthError {
    expected: u64,
}

/// Reports a `Content-Length` header that cannot frame an oversized body.
///
/// Bodies larger than `u32::MAX` cannot use `dwTotalLength`, so the header is
/// the only framing source. Where the body reports its own length, every header
/// value must equal that exact 64-bit length.
#[ohno::error]
#[display("a request body larger than u32::MAX requires Content-Length to equal its exact 64-bit length ({expected})")]
struct LargeContentLengthMismatchError {
    expected: u64,
}

/// Reports a caller-supplied `Transfer-Encoding` that `WinHTTP` cannot honor.
#[ohno::error]
#[display("WinHTTP frames the request body itself, so a request Transfer-Encoding header must be removed")]
struct UnsupportedTransferEncodingError;

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
                    // SAFETY: the writer borrows a RequestGuard, which exists
                    // only where ContextInstallation::install accepted the fully
                    // initialized context on a request opened under the
                    // session's WINHTTP_FLAG_ASYNC handle, whose status callback
                    // was registered with the full notification mask before any
                    // request handle existed. submit() armed the slot for Write
                    // and moved the live request handle into the returned
                    // future, so the handle stays open for this call, the armed
                    // kind matches it, and the emptied guard admits no second
                    // operation while this one is outstanding. `buffer`
                    // addresses the start of the view's first span and `len` is
                    // bounded by that span's length, so the span is readable and
                    // initialized for `len` bytes. The view moves into the
                    // operation buffer in this same call, and a view is
                    // immutable, so nothing can change or free those bytes until
                    // a completion, a request error, or HANDLE_CLOSING hands the
                    // view back; moving the view value does not move the pooled
                    // block its span lives in. A synchronous failure reclaims the
                    // view through submit()'s claim of the slot, which
                    // write_data's contract permits by starting no write and
                    // keeping no reference when it reports failure. No exclusive
                    // borrow of the context is outstanding across this call: the
                    // closure captures only a cloned facade and the span it
                    // lends, so a completion delivered inline reenters the
                    // callback on this thread with nothing but shared access in
                    // flight.
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
                // SAFETY: the guard, the slot armed for Write and the live
                // request handle moved into the returned future establish the
                // same preconditions as the span write above, and the emptied
                // guard again admits no overlapping operation. An absent buffer
                // is permitted only for the zero-length write that ends
                // automatic chunking, which is the only thing this method
                // performs: the request driver calls it once for a request
                // opened with WINHTTP_FLAG_AUTOMATIC_CHUNKING, after the body
                // reached end-of-stream, and awaits its completion before
                // response reception begins. No buffer is lent, and no exclusive
                // borrow of the context is outstanding across this call: the
                // closure captures only a cloned facade, so a completion
                // delivered inline reenters the callback on this thread with
                // nothing but shared access in flight.
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
                        .map_err(|_unsupported| invalid_request(RequestBodyError::from(UnsupportedBodyFrameError::new())))?;
                    return Err(invalid_request(RequestBodyError::from(UnsupportedTrailerFrameError::new())));
                }
            },
        }
    }
}

fn next_write_len(remaining: u64) -> Option<u32> {
    // `checked_sub`-style emptiness: a mutated `==`/`!=` on a bare comparison
    // against zero must not report a positive write length when nothing remains,
    // or the write loop submits empty buffers forever (AGENTS.md, "Code must not
    // hang even under mutation testing").
    let remaining = NonZeroU64::new(remaining)?;
    Some(u32::try_from(remaining.get().min(u64::from(u32::MAX))).expect("the write length is bounded by u32::MAX"))
}

/// Identifies body frame shapes that `WinHTTP` cannot submit.
///
/// One `ohno` source type per condition is rolled into this aggregate
/// (implementation.md section 1.1), so the send loop propagates a single error
/// type while the rejected frame shape stays in the source chain.
#[ohno::error]
#[from(UnsupportedBodyFrameError, UnsupportedTrailerFrameError)]
#[display("the request body produced a frame WinHTTP cannot submit")]
struct RequestBodyError;

/// Reports a request trailer frame, which `WinHTTP` has no API to submit.
#[ohno::error]
#[display("WinHTTP cannot submit request trailer frames")]
struct UnsupportedTrailerFrameError;

/// Reports a body frame that is neither data nor trailers.
#[ohno::error]
#[display("the request body produced an unsupported HTTP body frame")]
struct UnsupportedBodyFrameError;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::Arc;

    use bytesbuf::BytesView;
    use bytesbuf::mem::{GlobalPool, HasMemory as _, Memory as _};
    use http::HeaderValue;
    use ohno::Labeled as _;
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::{
        RequestBodyError, RequestBodyFraming, RequestBodyFramingError, WinHttpBodyWriter, end_chunked_completion, next_write_len,
        write_completion,
    };
    use crate::bindings::{BindingsFacade, MockBindings, WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH};
    use crate::context::{CompletionResult, OperationBuffer};
    use crate::mocks::{finish, installed};

    assert_impl_all!(RequestBodyFraming: UnwindSafe, RefUnwindSafe);
    // Every `ohno` error owns a boxed source without unwind-safety bounds.
    assert_not_impl_any!(RequestBodyFramingError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(RequestBodyError: UnwindSafe, RefUnwindSafe);
    // The writer mutably borrows a guard pointing at the callback context's UnsafeCell state.
    assert_not_impl_any!(WinHttpBodyWriter<'static>: UnwindSafe, RefUnwindSafe);

    #[test]
    fn request_body_framing_maps_known_and_unknown_lengths() {
        let mut headers = http::HeaderMap::new();
        let small = RequestBodyFraming::new(&mut headers, Some(42)).unwrap();
        assert_eq!(small.total_length(), 42);
        assert!(!small.automatic_chunking());

        // The largest length `dwTotalLength` can carry is still an exact length:
        // the sentinel is zero, so only a zero-length body collides with it.
        let exact_max = RequestBodyFraming::new(&mut headers, Some(u64::from(u32::MAX))).unwrap();
        assert_eq!(exact_max.total_length(), u32::MAX);
        assert_ne!(exact_max.total_length(), WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH);
        assert!(!exact_max.automatic_chunking());

        // Zero is that collision. WinHTTP reads the sentinel as "no total
        // supplied" and frames no body, which describes the same empty payload
        // as a declared length of zero, so no header is inserted to disambiguate.
        headers.remove(http::header::CONTENT_LENGTH);
        let empty = RequestBodyFraming::new(&mut headers, Some(0)).unwrap();
        assert_eq!(empty.total_length(), WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH);
        assert!(!empty.automatic_chunking());
        assert!(!headers.contains_key(http::header::CONTENT_LENGTH));

        // A caller-supplied zero still reconciles and survives, which is how an
        // empty POST keeps an explicit `Content-Length: 0` on the wire.
        headers.insert(http::header::CONTENT_LENGTH, HeaderValue::from_static("00"));
        let declared_empty = RequestBodyFraming::new(&mut headers, Some(0)).unwrap();
        assert_eq!(declared_empty.total_length(), WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH);
        assert_eq!(headers.get(http::header::CONTENT_LENGTH), Some(&HeaderValue::from_static("0")));

        headers.remove(http::header::CONTENT_LENGTH);
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

            let error = RequestBodyFraming::new(&mut headers, Some(5)).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("the request Content-Length header must equal the exact request body length (5) or be omitted"),
                "{error}"
            );
        }

        let mut headers = http::HeaderMap::new();
        headers.append(http::header::CONTENT_LENGTH, HeaderValue::from_static("5"));
        headers.append(http::header::CONTENT_LENGTH, HeaderValue::from_static("6"));
        let error = RequestBodyFraming::new(&mut headers, Some(5)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("the request Content-Length header must equal the exact request body length (5) or be omitted"),
            "{error}"
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

            let error = RequestBodyFraming::new(&mut headers, content_length).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("WinHTTP frames the request body itself, so a request Transfer-Encoding header must be removed"),
                "{error}"
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

            let error = RequestBodyFraming::new(&mut headers, Some(expected)).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("a request body larger than u32::MAX requires Content-Length to equal its exact 64-bit length (4294967296)"),
                "{error}"
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

        for invalid in ["", "invalid", "18446744073709551616"] {
            let mut headers = http::HeaderMap::new();
            headers.insert(http::header::CONTENT_LENGTH, HeaderValue::from_str(invalid).unwrap());
            let error = RequestBodyFraming::new(&mut headers, None).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("the request Content-Length header is not one consistent decimal u64 value"),
                "{error}"
            );
        }

        let mut headers = http::HeaderMap::new();
        headers.append(http::header::CONTENT_LENGTH, HeaderValue::from_static("10"));
        headers.append(http::header::CONTENT_LENGTH, HeaderValue::from_static("11"));
        let error = RequestBodyFraming::new(&mut headers, None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("the request Content-Length header is not one consistent decimal u64 value"),
            "{error}"
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

        assert_eq!(split(0), Vec::<u32>::new());
        assert_eq!(split(1), [1]);
        assert_eq!(split(u64::from(u32::MAX)), [u32::MAX]);
        assert_eq!(split(u64::from(u32::MAX) + 1), [u32::MAX, 1]);
        assert_eq!(split(u64::from(u32::MAX) * 2), [u32::MAX, u32::MAX]);
    }

    #[test]
    fn a_successful_write_completion_advances_the_view_past_the_written_bytes() {
        let buffer = BytesView::copied_from_slice(b"abcdef", &GlobalPool::new());

        let remaining = write_completion(CompletionResult::WriteComplete {
            buffer: buffer.clone(),
            len: 2,
        })
        .unwrap();

        assert_eq!(remaining.len(), buffer.len() - 2);
        assert_eq!(remaining, b"cdef");
    }

    #[test]
    fn the_writer_reserves_from_the_transport_memory_pool() {
        // `bytesbuf_io::Write` requires both memory traits so a caller can
        // serialize directly into transport-owned capacity, which must come
        // from the pool the transport was configured with.
        let (mut guard, context, contexts, session, closes) = installed();
        let pool = GlobalPool::new();
        let writer = WinHttpBodyWriter::new(&mut guard, BindingsFacade::mock(Arc::new(MockBindings::new())), pool);

        assert!(writer.reserve(64).capacity() >= 64);
        assert!(writer.memory().reserve(64).capacity() >= 64);

        // SAFETY: finish requires an installed context whose reclaiming
        // notification has not been delivered, no overlapping notification, and
        // no outstanding exclusive borrow. `installed` establishes all of them
        // and the writer above submitted no operation.
        unsafe { finish(guard, context, &contexts, session, &closes) };
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

    #[test]
    fn malformed_status_information_is_rejected_for_every_write_stage() {
        // A completion whose payload does not match its notification tells the
        // writer nothing about how many bytes reached the wire, so neither
        // stage may treat it as progress.
        let error = write_completion(CompletionResult::invalid_status_info(0x0010_0000, 3, OperationBuffer::none())).unwrap_err();

        assert_eq!(error.label(), "request_winhttp");
        assert!(error.to_string().contains("invalid status information"), "{error}");

        let error = end_chunked_completion(CompletionResult::invalid_status_info(0x0010_0000, 3, OperationBuffer::none())).unwrap_err();

        assert_eq!(error.label(), "request_winhttp");
        assert!(error.to_string().contains("invalid status information"), "{error}");

        let error = end_chunked_completion(CompletionResult::HeadersAvailable).unwrap_err();

        assert_eq!(error.label(), "request_winhttp");
        assert!(
            error.to_string().contains("unexpected completion for the final request write"),
            "{error}"
        );
    }
}
