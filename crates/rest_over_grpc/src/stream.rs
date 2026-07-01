// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Server-streaming response transcoding.
//!
//! A gRPC *server-streaming* RPC yields a sequence of response messages. There
//! is no single canonical way to render that sequence over HTTP/JSON, so this
//! module supports the three encodings the reference gateways offer, selected
//! by [`StreamEncoding`]:
//!
//! - [`StreamEncoding::JsonArray`] — the whole sequence as one JSON array
//!   (`[msg, msg, …]`), `application/json`.
//! - [`StreamEncoding::NdJson`] — newline-delimited JSON, one compact object per
//!   line, `application/x-ndjson`.
//! - [`StreamEncoding::Sse`] — [Server-Sent Events](https://developer.mozilla.org/docs/Web/API/Server-sent_events),
//!   one `data:` frame per message, `text/event-stream`.
//!
//! [`encode_frames`] adapts a stream of response messages into a stream of
//! encoded byte frames, so a streaming transport can forward each frame as it is
//! produced without buffering the whole response. [`collect_stream`] and
//! [`stream_response`] buffer the frames for transports (and adapters) that want
//! a single body.
//!
//! Client-streaming and bidirectional RPCs cannot be transcoded to REST and are
//! rejected at code-generation time, so only the server-streaming direction is
//! modeled here.

use futures_core::Stream;
use futures_util::StreamExt as _;
use serde::Serialize;

use crate::{HttpResponse, Status};

/// How a sequence of server-streamed response messages is rendered onto the
/// HTTP response body.
///
/// # Examples
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "streaming")] {
/// use rest_over_grpc::stream::StreamEncoding;
///
/// assert_eq!(StreamEncoding::JsonArray.content_type(), "application/json");
/// assert_eq!(
///     StreamEncoding::NdJson.content_type(),
///     "application/x-ndjson"
/// );
/// assert_eq!(StreamEncoding::Sse.content_type(), "text/event-stream");
/// # }
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEncoding {
    /// The whole sequence as a single JSON array (`[a, b, …]`); an empty stream
    /// renders as `[]`. Content type `application/json`.
    JsonArray,
    /// Newline-delimited JSON: each message is a compact JSON value followed by
    /// a `\n`. Content type `application/x-ndjson`.
    NdJson,
    /// [Server-Sent Events]: each message is emitted as a `data: <json>\n\n`
    /// frame. Content type `text/event-stream`.
    ///
    /// [Server-Sent Events]: https://developer.mozilla.org/docs/Web/API/Server-sent_events
    Sse,
}

impl StreamEncoding {
    /// The `Content-Type` header value for responses in this encoding.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "streaming")] {
    /// use rest_over_grpc::stream::StreamEncoding;
    ///
    /// assert_eq!(
    ///     StreamEncoding::NdJson.content_type(),
    ///     "application/x-ndjson"
    /// );
    /// # }
    /// # }
    /// ```
    #[must_use]
    pub const fn content_type(self) -> &'static str {
        match self {
            Self::JsonArray => "application/json",
            Self::NdJson => "application/x-ndjson",
            Self::Sse => "text/event-stream",
        }
    }
}

/// Serializes a single message to its compact JSON bytes, mapping a failure to a
/// [`Code::Internal`](crate::Code::Internal) [`Status`].
fn item_json<T: Serialize>(message: &T) -> Result<Vec<u8>, Status> {
    serde_json::to_vec(message).map_err(|e| Status::internal(format!("failed to serialize a streamed message: {e}")))
}

/// The driver state threaded through [`encode_frames`]'s unfold.
struct FrameState<S> {
    stream: core::pin::Pin<Box<S>>,
    encoding: StreamEncoding,
    /// Whether the next successfully encoded item is the first one. Only
    /// meaningful for [`StreamEncoding::JsonArray`] (chooses `[` vs `,`).
    first: bool,
    /// Set once the terminal frame has been emitted so the stream fuses.
    done: bool,
}

/// Adapts a stream of server-streamed response messages into a stream of encoded
/// byte frames in the chosen [`StreamEncoding`].
///
/// Framing (JSON-array brackets and separators, NDJSON newlines, SSE `data:`
/// envelopes) is applied incrementally, so a streaming transport can forward
/// each yielded frame immediately. The frames concatenated together are exactly
/// the body [`collect_stream`] would produce.
///
/// If the input stream yields an `Err`, that error is forwarded as the final
/// item and the stream ends (for [`StreamEncoding::JsonArray`] the array's
/// closing `]` is intentionally omitted — callers surface the error as a
/// status rather than a partial body).
///
/// # Examples
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "streaming")] {
/// use futures::StreamExt as _;
/// use futures_util::stream;
/// use rest_over_grpc::Status;
/// use rest_over_grpc::stream::{StreamEncoding, encode_frames};
/// use serde::Serialize;
///
/// #[derive(Debug, Serialize)]
/// struct Message {
///     n: u32,
/// }
///
/// let items = stream::iter(vec![
///     Ok::<_, Status>(Message { n: 1 }),
///     Ok(Message { n: 2 }),
/// ]);
/// let frames: Vec<Vec<u8>> = futures::executor::block_on(
///     encode_frames(items, StreamEncoding::JsonArray)
///         .map(|frame| frame.expect("frame"))
///         .collect(),
/// );
///
/// assert_eq!(
///     String::from_utf8(frames.concat()).expect("utf8"),
///     r#"[{"n":1},{"n":2}]"#
/// );
/// # }
/// # }
/// ```
pub fn encode_frames<S, T>(items: S, encoding: StreamEncoding) -> impl Stream<Item = Result<Vec<u8>, Status>>
where
    S: Stream<Item = Result<T, Status>>,
    T: Serialize,
{
    let state = FrameState {
        stream: Box::pin(items),
        encoding,
        first: true,
        done: false,
    };

    futures_util::stream::unfold(state, |mut state| async move {
        if state.done {
            return None;
        }

        match state.stream.next().await {
            Some(Ok(message)) => {
                let json = match item_json(&message) {
                    Ok(json) => json,
                    Err(status) => {
                        state.done = true;
                        return Some((Err(status), state));
                    }
                };
                let frame = frame_item(state.encoding, state.first, &json);
                state.first = false;
                Some((Ok(frame), state))
            }
            Some(Err(status)) => {
                state.done = true;
                Some((Err(status), state))
            }
            None => {
                state.done = true;
                match state.encoding {
                    // Close the array; an empty stream yields the whole `[]`.
                    StreamEncoding::JsonArray => {
                        let closing = if state.first { b"[]".to_vec() } else { b"]".to_vec() };
                        Some((Ok(closing), state))
                    }
                    // NDJSON and SSE need no terminal frame.
                    StreamEncoding::NdJson | StreamEncoding::Sse => None,
                }
            }
        }
    })
}

/// Wraps one already-serialized message `json` in its per-item framing.
fn frame_item(encoding: StreamEncoding, first: bool, json: &[u8]) -> Vec<u8> {
    match encoding {
        StreamEncoding::JsonArray => {
            let mut frame = Vec::with_capacity(json.len() + 1);
            frame.push(if first { b'[' } else { b',' });
            frame.extend_from_slice(json);
            frame
        }
        StreamEncoding::NdJson => {
            let mut frame = Vec::with_capacity(json.len() + 1);
            frame.extend_from_slice(json);
            frame.push(b'\n');
            frame
        }
        StreamEncoding::Sse => {
            let mut frame = Vec::with_capacity(json.len() + 8);
            frame.extend_from_slice(b"data: ");
            frame.extend_from_slice(json);
            frame.extend_from_slice(b"\n\n");
            frame
        }
    }
}

/// Buffers a server-streamed response into a single body in the chosen
/// [`StreamEncoding`].
///
/// # Errors
///
/// Returns the first [`Status`] the stream (or serialization) produces; no
/// partial body is returned so the caller can render a clean error response.
///
/// # Examples
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "streaming")] {
/// use futures_util::stream;
/// use rest_over_grpc::Status;
/// use rest_over_grpc::stream::{StreamEncoding, collect_stream};
/// use serde::Serialize;
///
/// #[derive(Debug, Serialize)]
/// struct Message {
///     n: u32,
/// }
///
/// let items = stream::iter(vec![Ok::<_, Status>(Message { n: 7 })]);
/// let body = futures::executor::block_on(collect_stream(items, StreamEncoding::NdJson))
///     .expect("stream collects");
/// assert_eq!(body, b"{\"n\":7}\n");
/// # }
/// # }
/// ```
pub async fn collect_stream<S, T>(items: S, encoding: StreamEncoding) -> Result<Vec<u8>, Status>
where
    S: Stream<Item = Result<T, Status>>,
    T: Serialize,
{
    let mut frames = core::pin::pin!(encode_frames(items, encoding));
    let mut body = Vec::new();
    while let Some(frame) = frames.next().await {
        body.extend_from_slice(&frame?);
    }
    Ok(body)
}

/// Renders a server-streamed response as a buffered [`HttpResponse`].
///
/// On success the response is `200 OK` with the encoding's content type; if the
/// stream yields an error the response is the mapped status (via
/// [`status_response`](crate::transcode::status_response)), matching the unary
/// path.
///
/// # Examples
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "streaming")] {
/// use futures_util::stream;
/// use rest_over_grpc::Status;
/// use rest_over_grpc::stream::{StreamEncoding, stream_response};
/// use serde::Serialize;
///
/// #[derive(Debug, Serialize)]
/// struct Message {
///     n: u32,
/// }
///
/// let items = stream::iter(vec![Ok::<_, Status>(Message { n: 1 })]);
/// let response = futures::executor::block_on(stream_response(items, StreamEncoding::Sse));
/// assert_eq!(response.status(), http::StatusCode::OK);
/// assert_eq!(response.content_type(), "text/event-stream");
/// assert_eq!(response.body(), b"data: {\"n\":1}\n\n");
/// # }
/// # }
/// ```
pub async fn stream_response<S, T>(items: S, encoding: StreamEncoding) -> HttpResponse
where
    S: Stream<Item = Result<T, Status>>,
    T: Serialize,
{
    match collect_stream(items, encoding).await {
        Ok(body) => HttpResponse::new(http::StatusCode::OK, encoding.content_type(), body),
        Err(status) => crate::transcode::status_response(&status),
    }
}

#[cfg(test)]
mod tests {
    use futures_util::stream;

    use super::*;

    #[derive(Debug, Serialize)]
    struct Msg {
        n: u32,
    }

    // A type whose `Serialize` impl always fails, exercising the mid-stream
    // serialization-error path.
    struct BadMsg;

    impl Serialize for BadMsg {
        fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("intentional serialize failure"))
        }
    }

    fn ok_stream(ns: &[u32]) -> impl Stream<Item = Result<Msg, Status>> {
        let items: Vec<Result<Msg, Status>> = ns.iter().map(|&n| Ok(Msg { n })).collect();
        stream::iter(items)
    }

    async fn collect(ns: &[u32], encoding: StreamEncoding) -> String {
        let body = collect_stream(ok_stream(ns), encoding).await.expect("collects");
        String::from_utf8(body).expect("utf8")
    }

    #[tokio::test]
    async fn json_array_encodes_all_messages() {
        assert_eq!(collect(&[1, 2, 3], StreamEncoding::JsonArray).await, r#"[{"n":1},{"n":2},{"n":3}]"#);
    }

    #[tokio::test]
    async fn json_array_empty_stream_is_empty_array() {
        assert_eq!(collect(&[], StreamEncoding::JsonArray).await, "[]");
    }

    #[tokio::test]
    async fn ndjson_encodes_one_object_per_line() {
        assert_eq!(collect(&[1, 2], StreamEncoding::NdJson).await, "{\"n\":1}\n{\"n\":2}\n");
    }

    #[tokio::test]
    async fn ndjson_empty_stream_is_empty() {
        assert_eq!(collect(&[], StreamEncoding::NdJson).await, "");
    }

    #[tokio::test]
    async fn sse_wraps_each_message_in_a_data_frame() {
        assert_eq!(collect(&[7], StreamEncoding::Sse).await, "data: {\"n\":7}\n\n");
    }

    #[tokio::test]
    async fn frames_concatenate_to_the_collected_body() {
        let frames: Vec<Vec<u8>> = encode_frames(ok_stream(&[1, 2]), StreamEncoding::JsonArray)
            .map(|f| f.expect("frame"))
            .collect()
            .await;
        let joined: Vec<u8> = frames.concat();
        assert_eq!(String::from_utf8(joined).expect("utf8"), r#"[{"n":1},{"n":2}]"#);
    }

    #[tokio::test]
    async fn error_in_stream_is_propagated() {
        let items = stream::iter(vec![Ok(Msg { n: 1 }), Err(Status::internal("boom"))]);
        let err = collect_stream(items, StreamEncoding::JsonArray).await.expect_err("propagates");
        assert_eq!(err.message(), "boom");
    }

    #[tokio::test]
    async fn stream_response_maps_error_to_status() {
        let items = stream::iter(vec![Err::<Msg, _>(Status::not_found("gone"))]);
        let response = stream_response(items, StreamEncoding::NdJson).await;
        assert_eq!(response.status().as_u16(), 404);
    }

    #[tokio::test]
    async fn stream_response_sets_encoding_content_type() {
        let response = stream_response(ok_stream(&[1]), StreamEncoding::Sse).await;
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(response.content_type(), "text/event-stream");
    }

    #[test]
    fn content_types_match_encodings() {
        assert_eq!(StreamEncoding::JsonArray.content_type(), "application/json");
        assert_eq!(StreamEncoding::NdJson.content_type(), "application/x-ndjson");
        assert_eq!(StreamEncoding::Sse.content_type(), "text/event-stream");
    }

    #[tokio::test]
    async fn serialization_failure_is_reported_as_internal() {
        let items = stream::iter(vec![Ok(BadMsg)]);
        let err = collect_stream(items, StreamEncoding::JsonArray).await.expect_err("nan fails");
        assert_eq!(err.code(), crate::Code::Internal);
    }
}
