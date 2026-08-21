// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Scripting and observation vocabulary shared by both localhost fixtures.
//!
//! `server.rs` (TCP; HTTP/1.1 and HTTP/2) and `http3_server.rs` (QUIC; HTTP/3) are two
//! implementations of a single scripted-response concept: a test hands either of them a sequence
//! of [`ResponsePlan`] values and reads back a [`ServerSnapshot`] of [`RecordedRequest`] values.
//! Defining that vocabulary here rather than inside one of the fixtures keeps the two
//! implementations peers, instead of making one reach into the other for the shared types.

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, Version};

/// One frame of a scripted response body.
///
/// Mirrors the `http_body::Frame` shape the transport is expected to surface, so a plan can
/// script a multi-frame body and a trailer section without either fixture inventing its own
/// representation.
#[derive(Clone, Debug)]
pub(crate) enum ResponseFrame {
    Data(Bytes),
    Trailers(HeaderMap),
}

/// A scripted response that either fixture can serve.
///
/// Tests describe what the peer should send - status, headers, body frames, trailers, and whether
/// the response stalls once the scripted frames are exhausted - and the fixture translates that
/// into whatever its protocol implementation requires. The fields are visible to the whole
/// `common` module so that both fixtures can drive their own protocol stack from one plan.
#[derive(Clone, Debug)]
pub(crate) struct ResponsePlan {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    pub(crate) frames: Vec<ResponseFrame>,
    /// When set, the response never completes after the scripted frames are sent. This is how a
    /// test observes a download that is still in flight; the fixture aborts the stalled task on
    /// shutdown, so no timer and no wall-clock deadline is involved.
    pub(crate) stall_after_frames: bool,
}

impl ResponsePlan {
    pub(crate) fn ok(body: impl Into<Bytes>) -> Self {
        Self {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            frames: vec![ResponseFrame::Data(body.into())],
            stall_after_frames: false,
        }
    }

    pub(crate) fn status(status: StatusCode) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            frames: vec![ResponseFrame::Data(Bytes::new())],
            stall_after_frames: false,
        }
    }

    pub(crate) fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.headers.append(name, value);
        self
    }

    pub(crate) fn chunks(chunks: impl IntoIterator<Item = Bytes>) -> Self {
        Self {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            frames: chunks.into_iter().map(ResponseFrame::Data).collect(),
            stall_after_frames: false,
        }
    }

    pub(crate) fn trailers(mut self, trailers: HeaderMap) -> Self {
        self.frames.push(ResponseFrame::Trailers(trailers));
        self
    }

    pub(crate) fn stall_after_frames(mut self) -> Self {
        self.stall_after_frames = true;
        self
    }

    /// Total length of the scripted data frames.
    ///
    /// A fixture whose protocol implementation does not derive `Content-Length` for it needs this
    /// to declare the body length the way a real origin server would.
    pub(crate) fn body_length(&self) -> usize {
        self.frames
            .iter()
            .map(|frame| match frame {
                ResponseFrame::Data(data) => data.len(),
                ResponseFrame::Trailers(_) => 0,
            })
            .sum()
    }
}

/// What a fixture observed on the wire for one request.
///
/// This is the only channel through which a test can assert on what the transport actually sent,
/// as opposed to what it was asked to send.
#[derive(Clone, Debug)]
pub(crate) struct RecordedRequest {
    pub(crate) method: Method,
    pub(crate) uri: Uri,
    pub(crate) version: Version,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Bytes,
    pub(crate) trailers: Option<HeaderMap>,
}

/// Everything a fixture observed over its lifetime, read once it has been shut down.
///
/// `connections` counts accepted transport connections rather than requests, which is how the
/// connection-pooling and cancellation tests distinguish reuse from re-establishment.
#[derive(Debug)]
pub(crate) struct ServerSnapshot {
    pub(crate) requests: Vec<RecordedRequest>,
    pub(crate) connections: usize,
}
