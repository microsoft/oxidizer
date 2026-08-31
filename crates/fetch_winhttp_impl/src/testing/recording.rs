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
pub enum ResponseFrame {
    /// A chunk of response body bytes.
    Data(Bytes),
    /// The response's trailer section, which terminates the body.
    Trailers(HeaderMap),
}

/// A scripted response that either fixture can serve.
///
/// Tests describe what the peer should send - status, headers, body frames, trailers, and whether
/// the response stalls once the scripted frames are exhausted - and the fixture translates that
/// into whatever its protocol implementation requires. The fields are public so that both fixtures
/// can drive their own protocol stack from one plan.
#[derive(Clone, Debug)]
pub struct ResponsePlan {
    /// Status the fixture responds with.
    pub status: StatusCode,
    /// Headers the fixture sends, beyond whatever its protocol stack adds.
    pub headers: HeaderMap,
    /// Body frames the fixture emits, in order.
    pub frames: Vec<ResponseFrame>,
    /// When set, the response never completes after the scripted frames are sent. This is how a
    /// test observes a download that is still in flight; the fixture aborts the stalled task on
    /// shutdown, so no timer and no wall-clock deadline is involved.
    pub stall_after_frames: bool,
}

impl ResponsePlan {
    /// A `200 OK` carrying `body` as a single data frame.
    #[must_use]
    pub fn ok(body: impl Into<Bytes>) -> Self {
        Self {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            frames: vec![ResponseFrame::Data(body.into())],
            stall_after_frames: false,
        }
    }

    /// A response carrying `status` and an empty body.
    #[must_use]
    pub fn status(status: StatusCode) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            frames: vec![ResponseFrame::Data(Bytes::new())],
            stall_after_frames: false,
        }
    }

    /// Appends a response header.
    #[must_use]
    pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.headers.append(name, value);
        self
    }

    /// A `200 OK` whose body arrives as several data frames.
    #[must_use]
    pub fn chunks(chunks: impl IntoIterator<Item = Bytes>) -> Self {
        Self {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            frames: chunks.into_iter().map(ResponseFrame::Data).collect(),
            stall_after_frames: false,
        }
    }

    /// Terminates the body with a trailer section.
    #[must_use]
    pub fn trailers(mut self, trailers: HeaderMap) -> Self {
        self.frames.push(ResponseFrame::Trailers(trailers));
        self
    }

    /// Leaves the response in flight once the scripted frames are sent.
    #[must_use]
    pub fn stall_after_frames(mut self) -> Self {
        self.stall_after_frames = true;
        self
    }

    /// Total length of the scripted data frames.
    ///
    /// A fixture whose protocol implementation does not derive `Content-Length` for it needs this
    /// to declare the body length the way a real origin server would.
    pub fn body_length(&self) -> usize {
        self.frames
            .iter()
            .map(|frame| match frame {
                ResponseFrame::Data(data) => data.len(),
                ResponseFrame::Trailers(_) => 0,
            })
            .sum()
    }
}

/// How a fixture chooses the response plan for each request it serves.
#[derive(Clone, Debug)]
pub enum ResponseScript {
    /// One plan per request, in order. A request past the end of the sequence is answered with
    /// `500`, and every request is recorded into the [`ServerSnapshot`].
    Sequence(Vec<ResponsePlan>),
    /// The same plan for every request, with no per-request recording. Benchmarks drive an
    /// unbounded number of requests this way, so fixture memory does not grow with the iteration
    /// count.
    Repeat(ResponsePlan),
}

impl ResponseScript {
    /// The plan to serve for the request at `index`.
    pub(crate) fn plan(&self, index: usize) -> ResponsePlan {
        match self {
            Self::Sequence(plans) => plans
                .get(index)
                .cloned()
                .unwrap_or_else(|| ResponsePlan::status(StatusCode::INTERNAL_SERVER_ERROR)),
            Self::Repeat(plan) => plan.clone(),
        }
    }

    /// Whether the fixture should retain a [`RecordedRequest`] for each request it serves.
    pub(crate) fn records(&self) -> bool {
        matches!(self, Self::Sequence(_))
    }
}

/// What a fixture observed on the wire for one request.
///
/// This is the only channel through which a test can assert on what the transport actually sent,
/// as opposed to what it was asked to send.
#[derive(Clone, Debug)]
pub struct RecordedRequest {
    /// Method the transport sent.
    pub method: Method,
    /// Request target as it arrived on the wire.
    pub uri: Uri,
    /// Protocol version the connection negotiated.
    pub version: Version,
    /// Headers the transport sent, including any it added itself.
    pub headers: HeaderMap,
    /// Request body, collected in full.
    pub body: Bytes,
    /// Request trailer section, if the protocol carried one.
    pub trailers: Option<HeaderMap>,
}

/// Everything a fixture observed over its lifetime, read once it has been shut down.
///
/// `connections` counts accepted transport connections rather than requests, which is how the
/// connection-pooling and cancellation tests distinguish reuse from re-establishment.
#[derive(Debug)]
pub struct ServerSnapshot {
    /// Requests the fixture served, in the order it assigned response plans.
    pub requests: Vec<RecordedRequest>,
    /// Transport connections the fixture accepted.
    pub connections: usize,
}
