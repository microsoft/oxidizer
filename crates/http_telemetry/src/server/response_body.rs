// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The response body wrapper, which emits the event of a request when its response body ends.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::Pin;
use std::task::{Context, Poll};

use bytesbuf::BytesView;
use http::{Method, StatusCode, Version};
use http_body::{Body, Frame, SizeHint};
use http_extensions::{HttpBody, HttpBodyBuilder, HttpError, HttpResponse};
use http_headers::headers::{ContentLength, ContentLengthOwned};

use super::drop_guard::{RequestEnd, TelemetryDropGuard};

pub(crate) fn emit_when_body_ends(mut guard: TelemetryDropGuard, response: HttpResponse, body_builder: &HttpBodyBuilder) -> HttpResponse {
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

    // A body with a declared length ends at that length over HTTP/1.0 and HTTP/1.1 only. Over HTTP/2
    // and HTTP/3, trailers can follow the content (RFC 9112, sections 6.2 and 7.1, RFC 9113, section
    // 8.1, and RFC 9114, section 4.1).
    let content_length = matches!(guard.version, Version::HTTP_10 | Version::HTTP_11)
        .then(|| {
            ContentLength::view(response.headers())
                .ok()
                .flatten()
                .map(ContentLengthOwned::get)
                .or_else(|| response.body().size_hint().exact())
        })
        .flatten();
    response.map(|body| {
        body_builder.rewrap(body, |inner| TelemetryBody {
            inner,
            guard,
            status_code,
            content_length,
            bytes_read: 0,
        })
    })
}

struct TelemetryBody {
    inner: HttpBody,
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
            let poll = Pin::new(&mut this.inner).poll_frame(cx);
            let ended = matches!(&poll, Poll::Ready(Some(Ok(frame))) if frame.is_data()) && this.inner.is_end_stream();
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
        self.inner.size_hint()
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }
}

impl Drop for TelemetryBody {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        if self.inner.is_end_stream() || self.content_length == Some(self.bytes_read) {
            self.guard.emit(RequestEnd::Complete(self.status_code));
        } else {
            self.guard.emit(RequestEnd::Abandoned(Some(self.status_code)));
        }
    }
}
