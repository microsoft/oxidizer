// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![forbid(unsafe_code)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/rest_over_grpc/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/rest_over_grpc/favicon.ico")]

//! Automatically transcode gRPC services to REST/JSON endpoints.
//!
//! `rest_over_grpc` generates REST routes from `google.api.http` annotations in
//! your `.proto` files. The generated REST surface runs in the same process as
//! the gRPC service, so you can reuse the same handlers without a separate
//! gateway hop.
//!
//! # Three layers
//!
//! The crate is organized around three layers:
//!
//! - **Serving**: adapt network I/O to the transcoder and back.
//! - **Transcoding**: match routes, decode JSON into protobuf messages, and
//!   encode replies or [`Status`](handling::Status) errors.
//! - **Handling**: implement the generated service trait directly or bridge an
//!   existing gRPC stack into it.
//!
//! ## Serving
//!
//! Pick the integration that fits your stack:
//!
//! - `tower`: wrap a generated transcoder with [`RestService::new`](serving::RestService::new) to get a [`tower_service::Service`].
//! - `layered`: the same [`RestService`](serving::RestService) also implements [`layered::Service`].
//! - `axum`: the `tower`-based [`RestService`](serving::RestService) mounts directly in `axum`; with the `axum` feature, the neutral response types also implement [`IntoResponse`](https://docs.rs/axum-core/latest/axum_core/response/trait.IntoResponse.html).
//! - direct HTTP: call [`serve_http`](serving::serve_http) or [`serve_http_fn`](serving::serve_http_fn) yourself.
//! - custom transport: disable `serving` and call [`transcode`](transcoding::Transcode::transcode) / [`try_transcode`](transcoding::Transcode::try_transcode) with `(method, target, headers, body)`.
//!
//! ## Transcoding
//!
//! Generated transcoders support unary and server-streaming RPCs. Unary calls
//! return a buffered [`HttpResponse`](transcoding::HttpResponse); server-streaming
//! calls return a [`StreamingResponse`](transcoding::StreamingResponse) whose
//! frames are forwarded as they are produced.
//!
//! Server-streaming response encoding is negotiated from `Accept`: JSON array
//! (`application/json`, `*/*`, or absent), NDJSON (`application/x-ndjson`), or
//! Server-Sent Events (`text/event-stream`).
//!
//! Use [`transcode`](transcoding::Transcode::transcode) when unmatched routes
//! should become `404`; use [`try_transcode`](transcoding::Transcode::try_transcode)
//! when you want to fall back to custom routing.
//!
//! ## Handling
//!
//! The generated `<Service>` trait has one method per RPC, each taking the
//! decoded request plus a mutable [`Context`](handling::Context).
//!
//! - `tonic`: the [`build`] module emits a guarded bridge so a `tonic`
//!   implementation can serve REST with an explicit authorization policy.
//! - direct implementation: implement the generated trait yourself.
//! - other gRPC stacks: write a small bridge that forwards into the generated
//!   trait.
//!
//! Server-streaming methods return a [`ResponseStream`](handling::ResponseStream),
//! and handlers report failures with [`Status`](handling::Status). Use
//! `Context` for request metadata and to set response headers.
//!
//! # Quick start: bridge an existing `tonic` service
//!
//! The normal setup generates protobuf messages, proto3-JSON serde
//! implementations, and the REST layer from the same descriptor set.
//!
//! 1. Annotate the service:
//!
//! ```text
//! syntax = "proto3";
//! package library;
//!
//! import "google/api/annotations.proto";
//!
//! service Library {
//!   rpc GetShelf(GetShelfRequest) returns (Shelf) {
//!     option (google.api.http) = {
//!       get: "/v1/shelves/{shelf}"
//!     };
//!   }
//! }
//!
//! message GetShelfRequest {
//!   string shelf = 1;
//! }
//!
//! message Shelf {
//!   string name = 1;
//! }
//! ```
//!
//! 2. In `build.rs`, compile one descriptor set through `tonic-prost-build`,
//!    `pbjson-build`, and `rest_over_grpc`. The REST generator does not generate
//!    message types or serde implementations itself:
//!
//! ```text
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let mut compiler = protox::Compiler::new(["proto"])?;
//!     compiler.include_imports(true);
//!     compiler.include_source_info(true);
//!     compiler.open_file("library.proto")?;
//!     let descriptors = compiler.encode_file_descriptor_set();
//!
//!     tonic_prost_build::configure()
//!         .build_client(false)
//!         .build_server(true)
//!         .build_transport(false)
//!         .compile_fds(compiler.file_descriptor_set())?;
//!     pbjson_build::Builder::new()
//!         .register_descriptors(&descriptors)?
//!         .build(&[".library"])?;
//!     rest_over_grpc::build::compile_fds(
//!         &descriptors,
//!         std::env::var("OUT_DIR")?,
//!     )?;
//!     Ok(())
//! }
//! ```
//!
//! Add `rest_over_grpc` with features `build,tower` plus `protox`,
//! `tonic-prost-build`, and `pbjson-build` as build dependencies. Generated
//! pbjson code also requires the corresponding serde/runtime dependencies.
//! The [worked example manifest] lists a complete set.
//!
//! 3. Include the generated files. Message, serde, and service-trait output
//!    belong in the proto package module; the top-level transcoder is included
//!    beside that module:
//!
//! ```text
//! pub mod library {
//!     include!(concat!(env!("OUT_DIR"), "/library.rs"));
//!     include!(concat!(env!("OUT_DIR"), "/library.serde.rs"));
//!     include!(concat!(env!("OUT_DIR"), "/library.rest.rs"));
//! }
//!
//! mod rest {
//!     use super::library;
//!     include!(concat!(env!("OUT_DIR"), "/transcoder.rest.rs"));
//! }
//! ```
//!
//! 4. Implement the generated `tonic` server trait as usual, then wrap that
//!    implementation in the generated transcoder:
//!
//! ```ignore
//! #[derive(Clone)]
//! struct LibraryService;
//!
//! #[tonic::async_trait]
//! impl library::library_server::Library for LibraryService {
//!     async fn get_shelf(
//!         &self,
//!         request: tonic::Request<library::GetShelfRequest>,
//!     ) -> Result<tonic::Response<library::Shelf>, tonic::Status> {
//!         let shelf = request.into_inner().shelf;
//!         Ok(tonic::Response::new(library::Shelf {
//!             name: format!("shelves/{shelf}"),
//!         }))
//!     }
//! }
//!
//! let bridge = library::__rest_over_grpc_bridge_Library::LibraryRestBridge::with_guard(LibraryService, authorize_rest_metadata);
//! let transcoder = rest::Transcoder::new(bridge);
//! let service = rest_over_grpc::serving::RestService::new(transcoder)
//!     .with_max_body_bytes(1 << 20);
//! ```
//!
//! `authorize_rest_metadata` is an application-provided guard that checks the
//! REST request's metadata before the tonic handler runs. Tonic transport
//! interceptors do not execute on REST calls. If an upstream HTTP layer
//! guarantees the same authorization, use the deliberately named
//! `LibraryRestBridge::externally_authenticated` constructor on the namespaced bridge instead.
//! The tonic bridge is emitted by default; call
//! [`Generator::builder`](build::Generator::builder) with
//! [`emit_tonic_bridge(false)`](build::GeneratorBuilder::emit_tonic_bridge) when
//! implementing the generated REST trait directly. See the [complete build
//! script], [generated includes], and [tonic handler] for versions that compile.
//!
//! [worked example manifest]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/Cargo.toml
//! [complete build script]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/build.rs
//! [generated includes]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/src/tonic_bridge.rs
//! [tonic handler]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/src/tonic_bridge.rs
//!
//! # Examples
//!
//! The [example index] maps common tasks to runnable examples. It covers
//! end-to-end generation, serving, direct transcoding, custom fallback,
//! streaming, OpenAPI, direct handlers, `tonic` bridging, and non-`tonic`
//! bridges. [`generate_service.rs`] demonstrates the lower-level manual
//! `HttpRule` API; annotation-driven generation is shown in the [complete build
//! script].
//!
//! # Limitations
//!
//! The crate supports unary and server-streaming RPCs only. Client-streaming
//! and bidirectional RPCs have no `google.api.http` mapping and are rejected by
//! [`build`].
//!
//! Requests are buffered and parsed as JSON, so there is no incremental request
//! body path and binary payloads must fit JSON-friendly encoding.
//!
//! Query parameter field paths are limited to 64 levels of nesting. A dotted
//! key such as `?a.b.c=1` builds one level per segment, so a path deeper than
//! the limit is rejected as an invalid request rather than decoded, truncated,
//! or allowed to exhaust the stack. The limit is fixed and not configurable: it
//! bounds the recursion an untrusted request can drive, and a bound a caller
//! could raise would not bound anything. It stays far above the nesting any
//! real proto message uses.
//! Query inputs also have a fixed aggregate pair and byte budget.
//!
//! # Cargo features
//!
//! - `serving` (default): [`serve_http`](serving::serve_http), [`serve_http_fn`](serving::serve_http_fn), and [`RestBody`](serving::RestBody).
//! - `tower`: [`RestService`](serving::RestService) as a [`tower_service::Service`].
//! - `layered`: [`RestService`](serving::RestService) as a [`layered::Service`].
//! - `axum`: `IntoResponse` for [`HttpResponse`](transcoding::HttpResponse), [`StreamingResponse`](transcoding::StreamingResponse), and [`TranscodeResponse`](transcoding::TranscodeResponse).
//! - `build`: the build-time code generator module.
//! - `build-openapi`: `build` plus OpenAPI 3.1 document generation.
//!
//! `tower` and `layered` imply `serving`. The `axum` feature only adds
//! `IntoResponse`; enable `tower` as well to mount [`RestService`](serving::RestService)
//! as an Axum fallback service.
//!
//! [example index]: https://github.com/microsoft/oxidizer/tree/main/crates/rest_over_grpc_examples#examples
//! [`generate_service.rs`]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc/examples/generate_service.rs

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
    //!
    //! # Request mapping
    //!
    //! The generated route's `google.api.http` rule determines where each field
    //! comes from:
    //!
    //! 1. JSON body fields are decoded according to `body`.
    //! 2. Query parameters overlay body fields.
    //! 3. Captured path variables are applied last and have highest precedence.
    //!
    //! Fields bound by the path or body are omitted from the generated OpenAPI
    //! query parameters. Other query parameters use proto3 JSON field names and
    //! may be dotted for nested messages. Repeated keys decode into repeated
    //! fields; supplying a repeated key for a scalar is an invalid request.
    //!
    //! Query names and values are strictly percent-decoded, with `+` interpreted
    //! as a space. Normal path captures decode all valid escapes; reserved
    //! multi-segment captures preserve escaped slashes. Malformed escapes,
    //! invalid UTF-8, invalid JSON, and values that do not match their protobuf
    //! field type become [`Code::InvalidArgument`](crate::handling::Code::InvalidArgument)
    //! responses (`400 Bad Request`).
    //!
    //! The transcoder expects proto3-JSON-compatible [`serde`] implementations
    //! on generated messages; [`pbjson`](https://docs.rs/pbjson) is the usual
    //! choice. It does not inspect the request `Content-Type`; serving code that
    //! requires `application/json` must enforce that policy before transcoding.
    //!
    //! # Routing and responses
    //!
    //! Primary and additional bindings, custom verbs, `body`, and
    //! `response_body` are binding-specific. Unary responses are buffered.
    //! Server-streaming responses negotiate JSON arrays, NDJSON, or SSE from
    //! `Accept`; unsupported values fall back to JSON.
    //!
    //! Handler [`Status`](crate::handling::Status) values use
    //! [`Code::to_http_status`](crate::handling::Code::to_http_status) and a
    //! `google.rpc.Status`-shaped JSON body. Response serialization failures
    //! become `Internal` (`500`). Once streaming headers have been sent, a
    //! handler or serialization failure terminates the body; it cannot change
    //! the already-sent `200 OK`.
    //!
    //! # Request-size policy
    //!
    //! Request JSON is buffered. HTTP adapters default to a 1 MiB incremental
    //! limit and reject oversized bodies with `413 Payload Too Large`. Use
    //! [`RestService::with_max_body_bytes`](crate::serving::RestService::with_max_body_bytes)
    //! or the direct helpers' `*_with_max_body_bytes` variants to override it.
    #[doc(inline)]
    pub use crate::http_response::HttpResponse;
    #[doc(inline)]
    pub use crate::stream::StreamEncoding;
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
    use base64::Engine as _;
    pub use http::{HeaderMap, HeaderName, HeaderValue};
    pub use routerama::codegen_helpers::{InvalidPath, RouteMatch, scan_segments, split_verb, with_scanned_path};
    use serde_json::{Value, json};

    pub use crate::path::{QueryPairs, parse_query, split_query};
    pub use crate::stream::{Stream, StreamEncoding, encode_frames, map_stream_status};
    pub use crate::transcode::{
        RequestBodyKind, ResponseBodyKind, RestParse, TranscodeError, decode_request, encode_response, parse_path_enum_value,
        parse_path_field, parse_reserved_path_enum_value, parse_reserved_path_field,
    };

    /// Wraps opaque `grpc-status-details-bin` bytes in a labeled JSON value.
    #[must_use]
    pub fn grpc_status_details(bytes: &[u8]) -> Value {
        json!({
            "kind": "grpc-status-details-bin",
            "encoding": "base64",
            "value": base64::engine::general_purpose::STANDARD.encode(bytes),
        })
    }

    /// Wraps ordinary gRPC metadata in a labeled JSON value.
    #[must_use]
    pub fn grpc_ascii_metadata(name: &str, value: &str) -> Value {
        json!({
            "kind": "grpc-metadata",
            "name": name,
            "value": value,
        })
    }

    /// Wraps binary gRPC metadata in a labeled, base64-encoded JSON value.
    #[must_use]
    pub fn grpc_binary_metadata(name: &str, value: &[u8]) -> Value {
        json!({
            "kind": "grpc-metadata",
            "name": name,
            "encoding": "base64",
            "value": base64::engine::general_purpose::STANDARD.encode(value),
        })
    }

    /// Wraps metadata whose text or binary encoding is invalid without losing
    /// its encoded header bytes.
    #[must_use]
    pub fn grpc_opaque_metadata(name: &str, encoded_value: &[u8]) -> Value {
        json!({
            "kind": "grpc-metadata",
            "name": name,
            "encoding": "base64",
            "representation": "encoded-header",
            "value": base64::engine::general_purpose::STANDARD.encode(encoded_value),
        })
    }

    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    mod tests {
        use super::*;

        #[test]
        fn grpc_status_details_are_labeled_and_base64_encoded() {
            assert_eq!(
                grpc_status_details(b"\x00\xff"),
                json!({
                    "kind": "grpc-status-details-bin",
                    "encoding": "base64",
                    "value": "AP8=",
                })
            );
        }

        #[test]
        fn grpc_ascii_metadata_is_labeled_text() {
            assert_eq!(
                grpc_ascii_metadata("x-reason", "invalid"),
                json!({
                    "kind": "grpc-metadata",
                    "name": "x-reason",
                    "value": "invalid",
                })
            );
        }

        #[test]
        fn grpc_binary_metadata_is_labeled_and_base64_encoded() {
            assert_eq!(
                grpc_binary_metadata("trace-bin", b"\x01\xfe"),
                json!({
                    "kind": "grpc-metadata",
                    "name": "trace-bin",
                    "encoding": "base64",
                    "value": "Af4=",
                })
            );
        }

        #[test]
        fn grpc_opaque_metadata_preserves_encoded_header_bytes() {
            assert_eq!(
                grpc_opaque_metadata("broken-bin", b"{}.."),
                json!({
                    "kind": "grpc-metadata",
                    "name": "broken-bin",
                    "encoding": "base64",
                    "representation": "encoded-header",
                    "value": "e30uLg==",
                })
            );
        }
    }
}
