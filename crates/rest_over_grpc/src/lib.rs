// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/rest_over_grpc/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/rest_over_grpc/favicon.ico")]

//! Automatically transcode gRPC services to REST/JSON endpoints.
//!
//! Services that expose a gRPC API often need to also expose a parallel REST API
//! either for compatibility or to enable REST-centric use cases. Implementing the same
//! API for both gRPC and REST is tedious, error-prone work, or requires additional machinery such
//! as a transcoding gateway.
//!
//! The `rest_over_grpc` crate makes it possible to start with a service that implements
//! a gRPC API and have it also expose a REST version of the same API with minimal work.
//! The resulting REST endpoint is located in the same process as the gRPC endpoint and is built on the
//! same set of handlers, so there isn't any extra network hop involved.
//!
//! The way this works is that you start with the `.proto` file that describes your
//! gRPC API and add annotations to individual gRPC methods to indicate how these methods
//! should be mapped into a REST API surface. The annotations are an industry standard
//! defined by [`google.api.http`](https://github.com/googleapis/googleapis/blob/master/google/api/http.proto).
//! Once the annotations are in place, you use this crate to generate a transcoder that
//! converts from an incoming HTTP request to a call to a gRPC method handler function.
//!
//! Given the generated transcoder, you need to wire it in to a web server that handles the
//! network communication. `rest_over_grpc` makes this easy by integrating with both the
//! [`tower`](https://crates.io/crates/tower) and [`layered`](https://crates.io/crates/layered)
//! crates, as well as having first class support for [`axum`](https://crates.io/crates/axum).
//!
//! # Architecture
//!
//! A `rest_over_grpc` endpoint is a stack of three layers. An HTTP request flows
//! down through them to a handler, and the response flows back up:
//!
//! ```text
//!                          HTTP request │   ▲ HTTP response
//!                                       ▼   │
//!   ┌────────────────────────────────────────────────────────────┐
//!   │  Serving      bytes on and off the wire                    │
//!   │               tower · layered · axum · raw `http` · none   │
//!   └────────────────────────────────────────────────────────────┘
//!                       (method, target, │   ▲ TranscodeResponse
//!                        headers, body)  ▼   │
//!   ┌────────────────────────────────────────────────────────────┐
//!   │  Transcoding  the generated `Transcoder`: route match +    │
//!   │               JSON/protobuf conversion                     │
//!   └────────────────────────────────────────────────────────────┘
//!                     request message +  │   ▲ response message
//!                            `Context`   ▼   │ or `Status`
//!   ┌────────────────────────────────────────────────────────────┐
//!   │  Handling     your gRPC method handlers                    │
//!   │               tonic bridge · direct impl · other stack     │
//!   └────────────────────────────────────────────────────────────┘
//! ```
//!
//! - **Serving** turns wire bytes into the neutral call the transcoder takes
//!   — `(method, target, headers, body)` — and turns the
//!   [`TranscodeResponse`](transcoding::TranscodeResponse) it returns back into a
//!   wire response. You choose how much of this the crate
//!   does for you, from a full [`tower_service::Service`] down to invoking the
//!   transcoder yourself.
//!
//! - **Transcoding** is done by the generated `Transcoder` — the piece this crate
//!   builds from your annotated `.proto`. It resolves the route, decodes the JSON
//!   request into the gRPC API's protobuf message, invokes the gRPC method
//!   handler, and encodes the reply (or a [`Status`](handling::Status)) back to JSON.
//!
//! - **Handling** is your service logic, reached through the generated per-API
//!   trait — implemented directly, or bridged from an existing gRPC service.
//!
//! The three layers are composed independently to form a fully cohesive REST/gRPC transcoding stack. Read
//! below for more details on each layer.
//!
//! # Usage
//!
//! Let's consider the three architectural layers mentioned above and the various options that exist
//! within each.
//!
//! ## Serving layer
//!
//! The serving layer is responsible for bridging from the network to/from the transcoder. You
//! have the following options for this layer:
//!
//! - **`tower`**: enable the `tower` feature and
//!   wrap the generated `Transcoder` with
//!   [`RestService::new`](serving::RestService::new), yielding a
//!   [`RestService`](serving::RestService) that implements
//!   [`tower_service::Service`]. Mount it like any other tower service; the
//!   service reads the body and headers, transcodes, and returns an
//!   [`http::Response`]. A single [`RestService`](serving::RestService) serves
//!   both unary (buffered) and server-streaming (frames forwarded to the wire)
//!   RPCs. See `examples/tower_service.rs`.
//!
//! - **`layered`**: enable the `layered` feature and use the same
//!   [`RestService::new`](serving::RestService::new) /
//!   [`RestService`](serving::RestService), which also implements
//!   [`layered::Service`]. See `examples/layered_service.rs`.
//!
//! - **`axum`**: because `axum` is built on `tower`, the `tower`-feature
//!   [`RestService`](serving::RestService) mounts directly — e.g.
//!   as a `fallback_service` or `nest_service` — with no `axum` feature needed
//!   (its response body is an [`http_body::Body`] type, so `axum` accepts it
//!   as-is). See `rest_over_grpc_examples`'s `examples/serving/axum_app.rs`. The
//!   separate `axum` feature covers the other direction: it implements
//!   [`IntoResponse`](https://docs.rs/axum-core/latest/axum_core/response/trait.IntoResponse.html)
//!   for the neutral [`HttpResponse`](transcoding::HttpResponse) /
//!   [`StreamingResponse`](transcoding::StreamingResponse) /
//!   [`TranscodeResponse`](transcoding::TranscodeResponse) value types, so a custom
//!   `async fn` handler that constructs those responses itself (mixing transcoded
//!   and hand-built replies) can `return` them directly rather than converting by
//!   hand. (The orphan rule means only this crate can add those impls.)
//!
//! - **Another `http` / `http-body` server directly**: skip the `Service` wrapper
//!   and call [`serve_http`](serving::serve_http)
//!   (the `serving` feature, on by default) with your
//!   [`http::Request`] and the
//!   generated `Transcoder`; it collects the body and transcodes — useful in a
//!   raw `hyper` `service_fn`, one arm of an ad-hoc router, or a test. Its
//!   closure-taking sibling [`serve_http_fn`](serving::serve_http_fn) suits a
//!   hand-written transcoder. See `examples/serve_http_fn.rs`.
//!
//! - **No web stack, or custom body handling:** with the `serving` feature
//!   disabled (`default-features = false`), call the generated `transcode` /
//!   `try_transcode` yourself with
//!   `(method, target, headers, body)`. Do this when your input isn't an
//!   [`http::Request`] (a `FaaS` event, a custom transport), or when you need to
//!   own the body step — size caps, header inspection, custom response headers —
//!   that the adapters bake in. See `rest_over_grpc_examples`'s
//!   `examples/serving/custom_body_handling.rs`.
//!
//! ## Transcoding layer
//!
//! The `Transcoder` itself is generated for you from the annotated `.proto`;
//! what you choose at this layer is how it delivers responses and how it composes
//! with routes it does not own.
//!
//! ### Server-streaming responses
//!
//! A streaming handler is two-phase — an `async fn` taking
//! the request and [`Context`](handling::Context) and returning a [`Result`] of a
//! [`ResponseStream`](handling::ResponseStream) or a [`Status`](handling::Status) — so initiation
//! can fail with a status before any item is produced, and the yielded stream is
//! `'static` (build it with [`Box::pin`]). The response encoding is negotiated
//! from the request's `Accept` header — a JSON array (the default for
//! `application/json`, `*/*`, or an absent header), newline-delimited JSON
//! (`application/x-ndjson`), or Server-Sent Events (`text/event-stream`).
//!
//! The generated `transcode` / `try_transcode` return a
//! [`TranscodeResponse`](transcoding::TranscodeResponse): a **unary** RPC yields a
//! buffered [`HttpResponse`](transcoding::HttpResponse), and a **server-streaming**
//! RPC yields a [`StreamingResponse`](transcoding::StreamingResponse) that the
//! serving adapters ([`serve_http`](serving::serve_http) /
//! [`RestService`](serving::RestService)) forward to the wire frame by frame as the
//! handler produces it. A failure observed after the headers are sent truncates
//! the body.
//!
//! ### Custom routes and fallbacks
//!
//! `transcode` defaults an unmatched route to a `404`. To compose the generated
//! routes with hand-written ones — a health check, a bespoke 404 page, a
//! catch-all — use `try_transcode` instead: it returns an [`Option`], where
//! [`None`] means *no generated route matched*, as opposed to a handler
//! returning [`Status::not_found`](handling::Status::not_found), which still yields `Some(404)`. Match on the
//! `None` case to fall through to your own handling; `transcode` is just the
//! convenience wrapper that turns `None` into a `404`. See
//! `rest_over_grpc_examples`'s `examples/transcoding/custom_fallback.rs`.
//!
//! ### Error responses with details
//!
//! A handler reports a domain failure by returning a [`Status`](handling::Status); the transcoding
//! layer renders it into the JSON error body. Beyond a [`Code`](handling::Code) and message, a
//! status can carry a `google.rpc.Status`-style `details` array (arbitrary JSON
//! values), which the transcoder renders as
//! `{"code": …, "message": …, "details": [ … ]}`, matching the shape the
//! reference gateways emit.
//!
//! ## Handling layer
//!
//! The generated `<Service>` trait has one method per RPC; how you implement it
//! depends on your gRPC stack:
//!
//! - **`tonic`:** the [`build`] module emits a blanket
//!   `impl <Service> for T where T: <tonic server trait>` by default, so a
//!   service written once against tonic's generated server trait also serves
//!   REST with no extra code — statuses, request/response metadata, and
//!   server-streaming responses are bridged for you. See the
//!   `rest_over_grpc_examples` crate's `tonic_bridge` module for this path end to
//!   end.
//!
//! - **Another framework (`volo`, `grpcio`, …) or none:** implement the
//!   generated trait directly, or — to reuse an existing framework service —
//!   write a small bridge that forwards each RPC and converts the framework's
//!   request/response/status types. See `rest_over_grpc_examples`'s
//!   `examples/handling/volo_bridge.rs`.
//!
//! Each generated method takes the decoded request message plus a mutable
//! [`Context`](handling::Context) reference: read the request headers (request-side metadata) from
//! it, and set custom response headers (`Location`, `ETag`, `Set-Cookie`, …) on
//! it. The transcoding layer merges those response headers into the returned
//! [`HttpResponse`](transcoding::HttpResponse) once the handler completes.
//!
//! # Generated code
//!
//! Let's assume you've annotated your `.proto` file with the following:
//!
//! ```protobuf
//! service Greeter {
//!   // Unary: bind the `{name}` path segment into the request message.
//!   rpc SayHello(HelloRequest) returns (HelloReply) {
//!     option (google.api.http) = { get: "/v1/greet/{name}" };
//!   }
//!
//!   // Server-streaming: a sequence of replies, encoded as a JSON array,
//!   // NDJSON, or Server-Sent Events per the request's `Accept` header.
//!   rpc StreamGreetings(HelloRequest) returns (stream HelloReply) {
//!     option (google.api.http) = { get: "/v1/greet/{name}:stream" };
//!   }
//! }
//! ```
//!
//! From that, `rest_over_grpc::build` (run from a `build.rs`; see the [`build`]
//! module) emits code shaped roughly like this — one service trait, a
//! `Transcoder`, and by default, a `tonic` bridge:
//!
//! ```ignore
//! // 1. Handling layer — a service trait with one `async fn` per gRPC method, taking the
//! //    decoded request and a `&mut Context` and returning `Result<Reply, Status>`
//! //    (a server-streaming RPC returns a `ResponseStream`). You implement this
//! //    (or reach it through the `tonic` bridge below).
//! pub trait Greeter: Send + Sync {
//!     async fn say_hello(&self, request: HelloRequest, cx: &mut Context)
//!         -> Result<HelloReply, Status>;
//!
//!     async fn stream_greetings(&self, request: HelloRequest, cx: &mut Context)
//!         -> Result<ResponseStream<HelloReply>, Status>;
//! }
//!
//! // 2. Transcoding layer — a `Transcoder` generic over your handler(s). It owns
//! //    the router built from the annotations and implements `Transcode`.
//! pub struct Transcoder<S> { /* ... */ }
//!
//! impl<S> Transcoder<S> {
//!     pub fn new(service: S) -> Self { /* ... */ }
//! }
//!
//! impl<S: Greeter> rest_over_grpc::transcoding::Transcode for Transcoder<S> {
//!     // Routes `GET /v1/greet/{name}` to `Greeter::say_hello`, decoding the JSON
//!     // request and encoding the reply. Returns a `TranscodeResponse` (a buffered
//!     // unary reply or a server-streaming frame stream); `None` when no route
//!     // matches.
//!     async fn try_transcode(&self, method: &str, target: &str,
//!         headers: HeaderMap, body: &[u8]) -> Option<TranscodeResponse> { /* ... */ }
//! }
//!
//! // 3. By default, a blanket `tonic` bridge: any `tonic`-generated server for
//! //    this service is a `Greeter` too, so one implementation serves gRPC and
//! //    REST with no extra code.
//! impl<T: greeter_server::Greeter> Greeter for T { /* forwards each RPC */ }
//! ```
//!
//! # Limitations
//!
//! The transcoder targets **unary and server-streaming** RPCs with JSON-shaped
//! payloads. Two limits follow from that, worth knowing before you design an
//! API around it:
//!
//! - **Client-streaming and bidirectional RPCs are unsupported.** They have no
//!   `google.api.http` mapping (an HTTP request is one message), so the
//!   [`build`] module rejects them at build time. Handle such an API —
//!   e.g. a chunked upload — with a dedicated HTTP handler that reads the body
//!   incrementally and bridges it to your native gRPC client-streaming call,
//!   composed alongside the transcoded JSON routes. See
//!   `rest_over_grpc_examples`'s `examples/handling/client_streaming_upload.rs`.
//!
//! - **The request body is buffered as one JSON `&[u8]`.** The generated
//!   `transcode` decodes the whole body with `serde_json`, so there is no
//!   incremental request path, and binary payloads must be base64-encoded in
//!   JSON. This is fine for ordinary request messages but unsuitable for large
//!   or binary uploads — route those around the transcoder (see the upload
//!   example above).
//!
//! # Crate features
//!
//! The core (always available) provides status mapping, the neutral message
//! types, and — in [`codegen_helpers`] — the generated-router transcode
//! primitives and JSON⇄protobuf message coding (via `pbjson`). Feature-gated
//! modules add the rest:
//!
//! - `serving` (default): the [`serve_http`](serving::serve_http) /
//!   [`serve_http_fn`](serving::serve_http_fn) one-shot helpers that bridge an
//!   [`http::Request`] to a transcoder and return an [`http::Response`] whose body
//!   ([`RestBody`](serving::RestBody)) serves both unary (buffered) and
//!   server-streaming (frames forwarded to the wire) replies. Implied by
//!   `tower` / `layered`.
//!
//! - `tower`: a [`tower_service::Service`] adapter
//!   ([`RestService`](serving::RestService)); implies `serving`.
//!
//! - `layered`: a [`layered::Service`] adapter (the repository's `async fn`-based
//!   service trait), on the same [`RestService`](serving::RestService); implies `serving`.
//!
//! - `axum`: an [`IntoResponse`](https://docs.rs/axum-core/latest/axum_core/response/trait.IntoResponse.html)
//!   impl for [`HttpResponse`](transcoding::HttpResponse), [`StreamingResponse`](transcoding::StreamingResponse), and
//!   [`TranscodeResponse`](transcoding::TranscodeResponse), so the neutral response types can be returned directly
//!   from an `axum` handler.
//!
//! - `build`: provides the build-time code generator as the [`build`]
//!   module, for use from a `build.rs`.
//!
//! Server-streaming response encodings — JSON array, NDJSON, and Server-Sent
//! Events — are always available (negotiated from the request's `Accept` header),
//! alongside the [`ResponseStream`](handling::ResponseStream) handler return type;
//! the [`StreamingResponse`](transcoding::StreamingResponse) /
//! [`TranscodeResponse`](transcoding::TranscodeResponse) path forwards each frame
//! to the wire as it is produced.
//!
//! # Examples
//!
//! Many runnable examples are provided in the source repository at the links below.
//!
//! This crate's own [`examples/`](https://github.com/microsoft/oxidizer/tree/main/crates/rest_over_grpc/examples)
//! show the adapters end to end, driving a stand-in transcoder:
//!
//! - [`tower_service.rs`](https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc/examples/tower_service.rs)
//!   — wrap a neutral transcoder as a [`RestService`](serving::RestService) for the `tower` ecosystem.
//! - [`layered_service.rs`](https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc/examples/layered_service.rs)
//!   — the same over the repository's `layered` `async fn` service trait.
//! - [`serve_http_fn.rs`](https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc/examples/serve_http_fn.rs)
//!   — call [`serve_http_fn`](serving::serve_http_fn) directly, without the [`RestService`](serving::RestService) wrapper.
//!
//! The [`rest_over_grpc_examples`](https://github.com/microsoft/oxidizer/tree/main/crates/rest_over_grpc_examples)
//! crate runs against a real generated transcoder, with examples grouped by the
//! architectural layer each exercises. The **serving** layer:
//!
//! - [`tower_service.rs`](https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/serving/tower_service.rs)
//!   — wiring the transcoder into a `tower` / `hyper` / `axum` stack.
//! - [`axum_app.rs`](https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/serving/axum_app.rs)
//!   — mounting the generated transcoder in an `axum::Router` as a
//!   `fallback_service`, no handler glue required.
//! - [`streaming_response.rs`](https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/serving/streaming_response.rs)
//!   — real server-streaming to the wire via `transcode` and
//!   [`serve_http`](serving::serve_http).
//! - [`custom_body_handling.rs`](https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/serving/custom_body_handling.rs)
//!   — owning the body step yourself (size caps, header inspection, custom
//!   response headers) around the generated `transcode`.
//!
//! The **transcoding** layer:
//!
//! - [`basic_transcode.rs`](https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/transcoding/basic_transcode.rs)
//!   — the core `transcode` loop over a `tonic`-bridged service.
//! - [`custom_fallback.rs`](https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/transcoding/custom_fallback.rs)
//!   — hand-written routes and a custom 404 via `try_transcode`.
//!
//! The **handling** layer:
//!
//! - [`direct_service.rs`](https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/handling/direct_service.rs)
//!   — implementing the generated service trait directly, using [`Context`](handling::Context)
//!   for request/response metadata and returning [`Status`](handling::Status) errors with details.
//! - [`volo_bridge.rs`](https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/handling/volo_bridge.rs)
//!   — a hand-written bridge for a non-`tonic` gRPC stack.
//! - [`client_streaming_upload.rs`](https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/handling/client_streaming_upload.rs)
//!   — handling a client-streaming upload outside the transcoder.
//!
//! Generating the service code from `google.api.http` rules is shown by this
//! crate's [`build`] example:
//!
//! - [`generate_service.rs`](https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc/examples/generate_service.rs)
//!   — build the HTTP rules and emit the service trait + transcoder as a
//!   `TokenStream`.

mod code;
mod context;
mod http_response;
mod path;
mod status;
mod transcode;
mod transcode_api;
mod transcode_response;

#[cfg(feature = "axum")]
mod axum_support;
mod stream;

#[cfg(feature = "serving")]
#[cfg_attr(docsrs, doc(cfg(feature = "serving")))]
pub mod serving;

#[cfg(feature = "build")]
#[cfg_attr(docsrs, doc(cfg(feature = "build")))]
pub mod build;

pub mod transcoding {
    //! Transcoding layer: the [`Transcode`] contract and the values it produces.
    //!
    //! The generated `Transcoder` implements [`Transcode`]; calling it yields a
    //! [`TranscodeResponse`] — a unary RPC's buffered [`HttpResponse`], or a
    //! server-streaming RPC's [`StreamingResponse`] whose frames reach the wire
    //! incrementally.
    #[doc(inline)]
    pub use crate::http_response::HttpResponse;
    #[doc(inline)]
    pub use crate::transcode_api::Transcode;
    #[doc(inline)]
    pub use crate::transcode_response::{FrameStream, StreamingResponse, TranscodeResponse};
}

pub mod handling {
    //! Handling layer: the value types your service handlers deal in.
    //!
    //! The generated `<Service>` trait's methods take the decoded request plus a
    //! [`&mut Context`](Context) and return `Result<Reply, Status>` (or, for a
    //! server-streaming RPC, `Result<ResponseStream<Reply>, Status>`). A [`Status`]
    //! carries a [`Code`].
    #[doc(inline)]
    pub use crate::code::{Code, UnknownCode};
    #[doc(inline)]
    pub use crate::context::Context;
    #[doc(inline)]
    pub use crate::status::Status;
    #[doc(inline)]
    pub use crate::transcode_response::ResponseStream;
}

/// Runtime primitives that generated transcoders reference by absolute path.
///
/// These back the generated static router (the path scanners) and the
/// JSON⇄message transcoder (`decode_request`, `parse_path_field`,
/// `encode_response`, and friends), and re-export the [`http`] header types (and
/// the [`Stream`](futures_core::Stream) trait) the generated service traits name
/// so a consumer need not add a direct `http` / `futures-core` dependency. They
/// are an implementation detail of the generated code, not a human-facing API,
/// and are hidden from the rendered documentation. Application and adapter
/// authors deal in the [`Status`](crate::handling::Status), [`Code`](crate::handling::Code),
/// and [`HttpResponse`](crate::transcoding::HttpResponse) types instead.
#[doc(hidden)]
pub mod codegen_helpers {
    pub use http::{HeaderMap, HeaderName, HeaderValue};
    pub use routerama::Route;
    pub use routerama::codegen_helpers::{scan_segments, split_verb};

    pub use crate::path::{QueryPairs, parse_query, split_query};
    pub use crate::stream::{Stream, StreamEncoding, encode_frames, map_stream_status};
    pub use crate::transcode::{
        RequestBodyKind, ResponseBodyKind, RestParse, TranscodeError, decode_request, encode_response, parse_path_enum_value,
        parse_path_field,
    };
}
