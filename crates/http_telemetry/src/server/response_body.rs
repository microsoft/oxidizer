// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The response body wrapper, which emits the event of a request when its response body ends.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::Pin;
use std::task::{Context, Poll};

use bytesbuf::BytesView;
use http::header::{CONTENT_LENGTH, TRANSFER_ENCODING};
use http::{HeaderMap, Method, StatusCode, Version};
use http_body::{Body, Frame, SizeHint};
use http_extensions::body::{Content, StreamingBody};
use http_extensions::{HttpError, HttpResponse};

use super::drop_guard::{RequestEnd, TelemetryDropGuard};

pub(crate) fn emit_when_body_ends(mut guard: TelemetryDropGuard, response: HttpResponse) -> HttpResponse {
    let status_code = response.status();
    // A server sends no content for a body at its end, nor for these responses (RFC 9110, sections
    // 9.3.2, 9.3.6, 15.2, 15.3.5 and 15.4.5), and can drop their body without a poll. So their
    // request ends when the handler returns.
    //
    // A panic in `is_end_stream` or `size_hint` of the body is not caught, here or in
    // `TelemetryBody::drop`, so it emits no event. This is deliberate: such a panic is rare, and
    // catching it would slow down every request.
    let no_content = response.body().is_end_stream()
        || guard.method == Method::HEAD
        || (guard.method == Method::CONNECT && status_code.is_success())
        || status_code.is_informational()
        || status_code == StatusCode::NO_CONTENT
        || status_code == StatusCode::NOT_MODIFIED;
    if no_content {
        guard.emit(RequestEnd::Complete(status_code));
        return response;
    }

    // A body with a declared length ends at that length over HTTP/1.0 and HTTP/1.1 only, and only when
    // no `Transfer-Encoding` frames the body. Chunked framing, HTTP/2 and HTTP/3 let trailers follow
    // the content (RFC 9112, sections 6.3 and 7.1, RFC 9113, section 8.1, and RFC 9114, section 4.1).
    let content_length = (matches!(guard.version, Version::HTTP_10 | Version::HTTP_11)
        && !response.headers().contains_key(TRANSFER_ENCODING))
    .then(|| decode_content_length(response.headers()).or_else(|| response.body().size_hint().exact()))
    .flatten();
    response.map(|body| {
        // TODO: Decide whether a buffered body can complete telemetry here instead of becoming a stream.
        body.map(|content| {
            Ok(Content::Streaming(StreamingBody::new(TelemetryBody {
                inner: content,
                guard,
                status_code,
                content_length,
                bytes_read: 0,
            })))
        })
        .expect("the response body was confirmed above to not be at its end")
    })
}

/// All repeated and comma-separated lengths must agree (RFC 9110, section 8.6).
fn decode_content_length(headers: &HeaderMap) -> Option<u64> {
    let mut length = None;
    for value in headers.get_all(CONTENT_LENGTH) {
        for item in value.to_str().ok()?.split(',') {
            let item = item.trim_matches([' ', '\t']);
            if item.is_empty() || !item.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let number = item.parse().ok()?;
            if length.is_some_and(|previous| previous != number) {
                return None;
            }
            length = Some(number);
        }
    }
    length
}

struct TelemetryBody {
    inner: Content,
    guard: TelemetryDropGuard,
    status_code: StatusCode,
    content_length: Option<u64>,
    bytes_read: u64,
}

impl Body for TelemetryBody {
    type Data = BytesView;
    type Error = HttpError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<BytesView>, HttpError>>> {
        let this = &mut *self;
        let polled = catch_unwind(AssertUnwindSafe(|| {
            let poll = match &mut this.inner {
                Content::Bytes(bytes) => {
                    let bytes = std::mem::take(bytes);
                    Poll::Ready((!bytes.is_empty()).then(|| Ok(Frame::data(bytes))))
                }
                Content::Streaming(body) => Pin::new(body).poll_frame(cx),
            };
            let ended = matches!(&poll, Poll::Ready(Some(Ok(frame))) if frame.is_data()) && this.is_end_stream();
            (poll, ended)
        }));
        let (poll, ended) = match polled {
            Ok(polled) => polled,
            Err(payload) => {
                this.guard.emit(RequestEnd::Panic(Some(this.status_code)));
                resume_unwind(payload)
            }
        };
        match &poll {
            Poll::Ready(None) => this.guard.emit(RequestEnd::Complete(this.status_code)),
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    this.bytes_read += data.len() as u64;
                }
                if ended || frame.is_trailers() {
                    this.guard.emit(RequestEnd::Complete(this.status_code));
                }
            }
            Poll::Ready(Some(Err(error))) => this.guard.emit(RequestEnd::Error(error, Some(this.status_code))),
            Poll::Pending => {}
        }
        poll
    }

    fn size_hint(&self) -> SizeHint {
        match &self.inner {
            Content::Bytes(bytes) => SizeHint::with_exact(bytes.len() as u64),
            Content::Streaming(body) => body.size_hint(),
        }
    }

    fn is_end_stream(&self) -> bool {
        match &self.inner {
            Content::Bytes(bytes) => bytes.is_empty(),
            Content::Streaming(body) => body.is_end_stream(),
        }
    }
}

impl Drop for TelemetryBody {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        if self.is_end_stream() || self.content_length == Some(self.bytes_read) {
            self.guard.emit(RequestEnd::Complete(self.status_code));
        } else {
            self.guard.emit(RequestEnd::Abandoned(Some(self.status_code)));
        }
    }
}
