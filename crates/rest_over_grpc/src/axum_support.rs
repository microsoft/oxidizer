// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `axum` integration (feature `axum`): [`IntoResponse`] for the neutral
//! response value types.
//!
//! The neutral [`HttpResponse`] and
//! [`StreamingResponse`](crate::transcoding::StreamingResponse) already convert into
//! [`http::Response`], and [`RestService`](crate::serving::RestService) implements
//! [`tower_service::Service`](tower_service::Service), which `axum` mounts
//! directly. This module adds the last piece of ergonomics: returning an
//! [`HttpResponse`] or [`StreamingResponse`] straight from an `axum` handler
//! function.

use axum_core::response::{IntoResponse, Response};
use bytes::Bytes;

use crate::transcode_response::{StreamingError, StreamingResponse, TranscodeResponse, apply_stream_headers};
use crate::transcoding::HttpResponse;

impl IntoResponse for HttpResponse {
    /// Converts the buffered response into an `axum` response, preserving the
    /// status, `Content-Type`, and any custom headers.
    fn into_response(self) -> Response {
        let (parts, body) = self.into_http().into_parts();
        Response::from_parts(parts, axum_core::body::Body::from(body))
    }
}

impl IntoResponse for StreamingResponse {
    /// Converts the streaming response into an `axum` response whose body streams
    /// each encoded frame incrementally.
    ///
    /// The status is `200 OK` with the encoding's `Content-Type` (authoritative
    /// over any custom header), and a mid-stream failure terminates the body,
    /// truncating the response.
    fn into_response(self) -> Response {
        use futures_util::StreamExt as _;

        let (content_type, headers, frames) = self.into_parts();
        let body = axum_core::body::Body::from_stream(frames.map(|item| item.map(Bytes::from).map_err(StreamingError)));

        // `Response::new` never fails, so a malformed caller-supplied
        // `content_type` cannot panic here; the negotiated `Content-Type` stays
        // authoritative over any custom header a handler set.
        let mut response = http::Response::new(body);
        apply_stream_headers(response.headers_mut(), content_type, headers);
        response.into_response()
    }
}

impl IntoResponse for TranscodeResponse {
    /// Converts either variant into an `axum` response: a
    /// [`Unary`](crate::transcoding::TranscodeResponse::Unary) buffers, a
    /// [`Streaming`](crate::transcoding::TranscodeResponse::Streaming) streams.
    fn into_response(self) -> Response {
        match self {
            Self::Unary(response) => response.into_response(),
            Self::Streaming(response) => response.into_response(),
        }
    }
}
