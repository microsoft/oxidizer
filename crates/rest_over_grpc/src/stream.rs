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
//!   line, `application/x-ndjson` (also selected by the `application/jsonl`
//!   alias).
//! - [`StreamEncoding::Sse`] — [Server-Sent Events](https://developer.mozilla.org/docs/Web/API/Server-sent_events),
//!   one `data:` frame per message, `text/event-stream`.
//!
//! [`encode_frames`] adapts a stream of response messages into a stream of
//! encoded byte frames, so a streaming transport can forward each frame as it is
//! produced without buffering the whole response.
//!
//! Client-streaming and bidirectional RPCs cannot be transcoded to REST and are
//! rejected at code-generation time, so only the server-streaming direction is
//! modeled here.

use core::pin::Pin;

/// Re-export of [`futures_core::Stream`], so generated server-streaming service
/// traits can name the stream trait through `rest_over_grpc` without the
/// consumer taking a direct `futures-core` dependency.
pub use futures_core::Stream;
use futures_util::StreamExt as _;
use serde::Serialize;

use crate::handling::Status;

/// How a sequence of server-streamed response messages is rendered onto the
/// HTTP response body.
///
/// # Examples
///
/// ```
/// # fn main() {
/// # {
/// use rest_over_grpc::codegen_helpers::StreamEncoding;
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
    /// # {
    /// use rest_over_grpc::codegen_helpers::StreamEncoding;
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

    /// Negotiates a streaming encoding from a request's `Accept` header.
    ///
    /// The first recognized media type wins: `text/event-stream` selects
    /// [`Sse`](Self::Sse) and `application/x-ndjson` (or `application/jsonl`)
    /// selects [`NdJson`](Self::NdJson). Anything else — including
    /// `application/json`, `*/*`, or an absent header — falls back to
    /// [`JsonArray`](Self::JsonArray). Media-type parameters (e.g. `;q=0.9`) are
    /// ignored.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() {
    /// # {
    /// use rest_over_grpc::codegen_helpers::StreamEncoding;
    ///
    /// assert_eq!(
    ///     StreamEncoding::from_accept("text/event-stream"),
    ///     StreamEncoding::Sse
    /// );
    /// assert_eq!(
    ///     StreamEncoding::from_accept("application/x-ndjson"),
    ///     StreamEncoding::NdJson
    /// );
    /// assert_eq!(
    ///     StreamEncoding::from_accept("application/json"),
    ///     StreamEncoding::JsonArray
    /// );
    /// assert_eq!(StreamEncoding::from_accept(""), StreamEncoding::JsonArray);
    /// # }
    /// # }
    /// ```
    #[must_use]
    pub fn from_accept(accept: &str) -> Self {
        for media in accept.split(',') {
            // Drop any parameters (`;q=…`, `;charset=…`) and surrounding space.
            // HTTP media types are case-insensitive, so match ASCII-fold.
            let media_type = media.split(';').next().unwrap_or("").trim();
            if media_type.eq_ignore_ascii_case("text/event-stream") {
                return Self::Sse;
            }
            if media_type.eq_ignore_ascii_case("application/x-ndjson") || media_type.eq_ignore_ascii_case("application/jsonl") {
                return Self::NdJson;
            }
        }
        Self::JsonArray
    }
}

/// Serializes `message` to compact JSON directly into a freshly-allocated frame
/// buffer already carrying its per-item [`StreamEncoding`] framing.
///
/// Serializing straight into the framed buffer avoids the second allocation and
/// full copy a serialize-to-`Vec`-then-reframe step would incur — one buffer per
/// streamed item instead of two. A serialization failure maps to a
/// [`Code::Internal`](crate::handling::Code::Internal) [`Status`] (the partially
/// written frame is discarded by the caller).
fn serialize_framed_item<T: Serialize>(encoding: StreamEncoding, first: bool, message: &T) -> Result<Vec<u8>, Status> {
    // Seed with the capacity `serde_json::to_vec` would pick, plus room for the
    // largest per-item envelope (`SSE`'s `data: ` + `\n\n`), so a typical message
    // frames without reallocating.
    let mut frame = Vec::with_capacity(128 + 8);
    match encoding {
        StreamEncoding::JsonArray => frame.push(if first { b'[' } else { b',' }),
        StreamEncoding::NdJson => {}
        StreamEncoding::Sse => frame.extend_from_slice(b"data: "),
    }
    serde_json::to_writer(&mut frame, message).map_err(|e| Status::internal(format!("failed to serialize a streamed message: {e}")))?;
    match encoding {
        StreamEncoding::JsonArray => {}
        StreamEncoding::NdJson => frame.push(b'\n'),
        StreamEncoding::Sse => frame.extend_from_slice(b"\n\n"),
    }
    Ok(frame)
}

/// The driver state threaded through [`encode_frames`]'s unfold.
struct FrameState<S> {
    stream: Pin<Box<S>>,
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
/// each yielded frame immediately.
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
/// # {
/// use futures::StreamExt as _;
/// use futures_util::stream;
/// use rest_over_grpc::codegen_helpers::{StreamEncoding, encode_frames};
/// use rest_over_grpc::handling::Status;
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
                let frame = match serialize_framed_item(state.encoding, state.first, &message) {
                    Ok(frame) => frame,
                    Err(status) => {
                        state.done = true;
                        return Some((Err(status), state));
                    }
                };
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

/// Maps a server-streaming response stream's foreign error type to [`Status`],
/// leaving the item type untouched.
///
/// Backs the `tonic` bridge for server-streaming RPCs: it converts each streamed
/// item's foreign error `E` (e.g. `tonic::Status`) to a [`Status`] via
/// `convert_status`, so this crate never names the foreign type.
///
/// # Examples
///
/// ```
/// # fn main() {
/// # {
/// use futures::StreamExt as _;
/// use futures_util::stream;
/// use rest_over_grpc::codegen_helpers::map_stream_status;
/// use rest_over_grpc::handling::{Code, Status};
///
/// // Stand in for a response stream whose items carry a foreign error type
/// // (here, a plain `&str`).
/// let source = stream::iter(vec![Ok::<u32, &str>(1), Err("boom")]);
/// let convert = |error: &str| Status::new(Code::Internal, error);
///
/// let items: Vec<_> = futures::executor::block_on(map_stream_status(source, convert).collect());
/// assert_eq!(items[0].as_ref().unwrap(), &1);
/// assert_eq!(items[1].as_ref().unwrap_err().code(), Code::Internal);
/// # }
/// # }
/// ```
#[doc(hidden)]
pub fn map_stream_status<S, T, E, C>(stream: S, convert_status: C) -> impl Stream<Item = Result<T, Status>> + Send
where
    S: Stream<Item = Result<T, E>> + Send,
    C: Fn(E) -> Status + Send,
    T: Send,
    E: Send,
{
    stream.map(move |item| item.map_err(&convert_status))
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt as _;

    use super::*;
    use crate::handling::Code;

    /// A message whose serialization always fails, to exercise the streamed
    /// item-serialization error path.
    struct Unserializable;

    impl Serialize for Unserializable {
        fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("intentional serialize failure"))
        }
    }

    #[test]
    fn encode_frames_forwards_a_streamed_item_serialization_failure() {
        let items = futures_util::stream::iter(vec![Ok::<_, Status>(Unserializable)]);
        let frames: Vec<_> = futures::executor::block_on(encode_frames(items, StreamEncoding::JsonArray).collect());
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].as_ref().expect_err("serialize fails").code(), Code::Internal);
    }

    #[test]
    fn encode_frames_wraps_each_item_in_its_sse_envelope() {
        // Exercises the incremental `serialize_framed_item` SSE arm: each item
        // becomes a `data: …\n\n` frame.
        #[derive(serde::Serialize)]
        struct Tick {
            n: u32,
        }

        let items = futures_util::stream::iter(vec![Ok::<_, Status>(Tick { n: 1 }), Ok(Tick { n: 2 })]);
        let frames: Vec<_> = futures::executor::block_on(encode_frames(items, StreamEncoding::Sse).collect());
        let frames: Vec<Vec<u8>> = frames.into_iter().map(|f| f.expect("frame encodes")).collect();
        assert_eq!(frames[0], b"data: {\"n\":1}\n\n");
        assert_eq!(frames[1], b"data: {\"n\":2}\n\n");
    }

    #[test]
    fn from_accept_is_case_insensitive() {
        // HTTP media types are case-insensitive, so mixed/upper case must still
        // negotiate the streaming encoding.
        assert_eq!(StreamEncoding::from_accept("Text/Event-Stream"), StreamEncoding::Sse);
        assert_eq!(StreamEncoding::from_accept("APPLICATION/X-NDJSON"), StreamEncoding::NdJson);
        assert_eq!(StreamEncoding::from_accept("Application/JSONL"), StreamEncoding::NdJson);
        // Parameters and surrounding whitespace are still ignored.
        assert_eq!(
            StreamEncoding::from_accept("text/plain, Text/Event-Stream;q=0.9"),
            StreamEncoding::Sse
        );
    }
}
