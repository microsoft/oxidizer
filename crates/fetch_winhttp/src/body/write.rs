// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytesbuf::mem::{GlobalPool, HasMemory, Memory, MemoryShared};
use bytesbuf::{BytesBuf, BytesView};
use bytesbuf_io::Write;
use fetch::{HttpBody, HttpError, RecoveryInfo};
use http::header::CONTENT_LENGTH;
use http::{HeaderMap, HeaderValue};
use http_body::Body as _;
use std::fmt;
use std::future::poll_fn;
use std::pin::Pin;
use std::ptr::NonNull;

use crate::bindings::{Bindings as _, Facade};
use crate::context::{CompletionResult, OperationBuffer, OperationKind};
use crate::error_labels;
use crate::options::WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH;
use crate::request::RequestGuard;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RequestBodyPlan {
    total_length: u32,
    automatic_chunking: bool,
}

impl RequestBodyPlan {
    pub(crate) fn new(headers: &mut HeaderMap, content_length: Option<u64>) -> Result<Self, RequestBodyPlanError> {
        let content_length = match content_length {
            Some(length) => Some(length),
            None => Self::declared_content_length(headers)?,
        };

        match content_length {
            Some(length) => {
                let total_length = match u32::try_from(length) {
                    Ok(length) => length,
                    Err(_too_large) => {
                        validate_large_content_length(headers, length)?;
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

    fn declared_content_length(headers: &mut HeaderMap) -> Result<Option<u64>, RequestBodyPlanError> {
        let mut values = headers.get_all(CONTENT_LENGTH).iter();
        let Some(first) = values.next() else {
            return Ok(None);
        };
        let Some(length) = parse_content_length(first) else {
            return Err(RequestBodyPlanError::InvalidContentLength);
        };

        if values.any(|value| parse_content_length(value) != Some(length)) {
            return Err(RequestBodyPlanError::InvalidContentLength);
        }

        let value = HeaderValue::from_str(&length.to_string())
            .expect("a decimal u64 contains only ASCII digits and is always a valid HTTP header value");
        headers.insert(CONTENT_LENGTH, value);

        Ok(Some(length))
    }

    pub(crate) const fn total_length(self) -> u32 {
        self.total_length
    }

    pub(crate) const fn automatic_chunking(self) -> bool {
        self.automatic_chunking
    }
}

fn validate_large_content_length(headers: &mut HeaderMap, expected: u64) -> Result<(), RequestBodyPlanError> {
    if headers
        .get_all(CONTENT_LENGTH)
        .iter()
        .any(|value| parse_content_length(value) != Some(expected))
    {
        return Err(RequestBodyPlanError::InvalidLargeContentLength { expected });
    }

    let value = HeaderValue::from_str(&expected.to_string())
        .expect("a decimal u64 contains only ASCII digits and is always a valid HTTP header value");
    headers.insert(CONTENT_LENGTH, value);

    Ok(())
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RequestBodyPlanError {
    InvalidContentLength,
    InvalidLargeContentLength { expected: u64 },
}

impl fmt::Display for RequestBodyPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidContentLength => f.write_str("the request Content-Length header is not one consistent decimal u64 value"),
            Self::InvalidLargeContentLength { expected } => write!(
                f,
                "a request body larger than u32::MAX requires Content-Length to equal its exact 64-bit length ({expected})"
            ),
        }
    }
}

impl std::error::Error for RequestBodyPlanError {}

#[derive(Debug)]
pub(crate) struct WinHttpBodyWriter<'guard> {
    guard: &'guard mut RequestGuard,
    bindings: Facade,
    memory: GlobalPool,
}

impl<'guard> WinHttpBodyWriter<'guard> {
    pub(crate) const fn new(guard: &'guard mut RequestGuard, bindings: Facade, memory: GlobalPool) -> Self {
        Self { guard, bindings, memory }
    }

    async fn write_view(&mut self, mut data: BytesView) -> fetch::Result<()> {
        while !data.is_empty() {
            let span = data.first_slice();
            debug_assert!(!span.is_empty(), "a nonempty BytesView must have a nonempty first span");

            let len = next_write_len(span.len() as u64).expect("a nonempty span always produces a write length");
            let buffer = NonNull::from(span).cast::<u8>();
            let address = buffer.as_ptr().addr();
            let bindings = self.bindings.clone();
            let write = self
                .guard
                .submit(
                    OperationKind::Write,
                    OperationBuffer::write(data, address, len),
                    move |request, _context| {
                        // SAFETY: the exact BytesView and contiguous span
                        // identified by buffer/len are retained in the active
                        // operation until its completion.
                        unsafe { bindings.write_data(request, buffer, len) }
                    },
                )
                .map_err(|_active| callback_protocol_error("the write operation slot was already active"))?;
            let completion = write
                .await
                .map_err(|_disconnected| callback_protocol_error("the write completion channel disconnected"))?;

            data = write_completion(completion)?;
        }

        Ok(())
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
    use bytesbuf::BytesView;
    use bytesbuf::mem::GlobalPool;
    use ohno::Labeled as _;

    use http::HeaderValue;

    use super::{RequestBodyPlan, RequestBodyPlanError, next_write_len, write_completion};
    use crate::context::CompletionResult;
    use crate::options::WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH;

    #[test]
    fn request_body_plan_maps_known_and_unknown_lengths() {
        let mut headers = http::HeaderMap::new();
        let small = RequestBodyPlan::new(&mut headers, Some(42)).expect("small known length is supported");
        assert_eq!(small.total_length(), 42);
        assert!(!small.automatic_chunking());

        let large_length = u64::from(u32::MAX) + 1;
        let large = RequestBodyPlan::new(&mut headers, Some(large_length)).expect("large known length is supported");
        assert_eq!(large.total_length(), WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH);
        assert!(!large.automatic_chunking());
        assert_eq!(
            headers.get(http::header::CONTENT_LENGTH),
            Some(&HeaderValue::from_static("4294967296"))
        );

        headers.remove(http::header::CONTENT_LENGTH);
        let unknown = RequestBodyPlan::new(&mut headers, None).expect("unknown length is supported");
        assert_eq!(unknown.total_length(), WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH);
        assert!(unknown.automatic_chunking());
    }

    #[test]
    fn large_content_length_is_validated_and_canonicalized() {
        let expected = u64::from(u32::MAX) + 1;
        let mut headers = http::HeaderMap::new();
        headers.append(http::header::CONTENT_LENGTH, HeaderValue::from_static("04294967296"));
        headers.append(http::header::CONTENT_LENGTH, HeaderValue::from_static("4294967296"));

        RequestBodyPlan::new(&mut headers, Some(expected)).expect("equivalent values are valid");

        assert_eq!(
            headers.get_all(http::header::CONTENT_LENGTH).iter().collect::<Vec<_>>(),
            [&HeaderValue::from_static("4294967296")]
        );

        for invalid in ["4294967295", "not-a-number", "18446744073709551616"] {
            let mut headers = http::HeaderMap::new();
            headers.insert(
                http::header::CONTENT_LENGTH,
                HeaderValue::from_str(invalid).expect("test header value is syntactically valid"),
            );

            assert_eq!(
                RequestBodyPlan::new(&mut headers, Some(expected)),
                Err(RequestBodyPlanError::InvalidLargeContentLength { expected })
            );
        }
    }

    #[test]
    fn content_length_header_declares_an_otherwise_unknown_body_length() {
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::CONTENT_LENGTH, HeaderValue::from_static("10"));

        let plan = RequestBodyPlan::new(&mut headers, None).expect("the header declares the body length");
        assert_eq!(plan.total_length(), 10);
        assert!(!plan.automatic_chunking());

        for invalid in ["invalid", "18446744073709551616"] {
            let mut headers = http::HeaderMap::new();
            headers.insert(
                http::header::CONTENT_LENGTH,
                HeaderValue::from_str(invalid).expect("test header value is syntactically valid"),
            );
            assert_eq!(
                RequestBodyPlan::new(&mut headers, None),
                Err(RequestBodyPlanError::InvalidContentLength)
            );
        }

        let mut headers = http::HeaderMap::new();
        headers.append(http::header::CONTENT_LENGTH, HeaderValue::from_static("10"));
        headers.append(http::header::CONTENT_LENGTH, HeaderValue::from_static("11"));
        assert_eq!(
            RequestBodyPlan::new(&mut headers, None),
            Err(RequestBodyPlanError::InvalidContentLength)
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
        let error = write_completion(CompletionResult::HeadersAvailable).expect_err("wrong completion kind is rejected");

        assert_eq!(error.label(), "request_winhttp");
        assert!(error.to_string().contains("unexpected completion for Write"));

        let error = write_completion(CompletionResult::WriteComplete {
            buffer: BytesView::copied_from_slice(b"x", &GlobalPool::new()),
            len: 0,
        })
        .expect_err("zero-byte completion is rejected");

        assert_eq!(error.label(), "request_winhttp");
        assert!(error.to_string().contains("without writing any bytes"));
    }
}
