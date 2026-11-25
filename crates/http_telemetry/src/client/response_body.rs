// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The complete event of a client request with its logged response body.

use http::{StatusCode, Version};
use http_body::Body;
use http_body_util::BodyExt;
use http_extensions::HttpResponse;
use http_extensions::body::{Content, StreamingBody};

use super::request::{ClientRequest, RequestEnd};
use crate::body_buffer::BodyBuffer;
use crate::drop_guard::TelemetryDropGuard;

/// Emits the event of the request whose handler returned `response`, whose body the layer logs. If the body streams, the
/// event comes when the caller drops the body, and logs the bytes that the caller read.
pub(crate) fn emit_with_response_body(mut guard: TelemetryDropGuard<ClientRequest>, response: HttpResponse) -> HttpResponse {
    let (status_code, version) = (response.status(), response.version());
    let mut buffer = BodyBuffer::new(guard.request.client.body_size_limit);
    if response.body().is_end_stream() {
        guard.emit(RequestEnd::Complete(status_code, version, Some(&buffer)));
        return response;
    }
    response.map(|body| {
        body.map(|content| {
            Ok(match content {
                Content::Bytes(bytes) => {
                    buffer.push_last(&bytes);
                    guard.emit(RequestEnd::Complete(status_code, version, Some(&buffer)));
                    Content::Bytes(bytes)
                }
                Content::Streaming(inner) => {
                    let duration_secs = guard.elapsed_secs();
                    let mut pending_event = PendingEvent {
                        guard,
                        status_code,
                        version,
                        duration_secs,
                        buffer,
                    };
                    Content::Streaming(StreamingBody::new(inner.inspect_frame(move |frame| {
                        if let Some(data) = frame.data_ref() {
                            pending_event.buffer.push(data);
                        }
                    })))
                }
            })
        })
        .expect("map fails only for a consumed body, which is at its end, so the check above rules it out")
    })
}

/// The complete event of a request whose logged response body streams. The body owns it, so the event comes when the
/// caller drops the body, with the bytes that the caller read. The duration ends when the handler returns.
struct PendingEvent {
    guard: TelemetryDropGuard<ClientRequest>,
    status_code: StatusCode,
    version: Version,
    duration_secs: f64,
    buffer: BodyBuffer,
}

impl Drop for PendingEvent {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        self.guard.emit_with_duration(
            RequestEnd::Complete(self.status_code, self.version, Some(&self.buffer)),
            self.duration_secs,
        );
    }
}
