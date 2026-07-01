// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/rest_over_grpc/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/rest_over_grpc/favicon.ico")]

//! Framework-neutral runtime primitives for transcoding gRPC services into
//! REST/JSON endpoints.
//!
//! This crate is the runtime half of a gRPC→REST transcoding system. It is
//! deliberately decoupled from any particular web stack (hyper, axum, tonic,
//! tower, …): it knows nothing about sockets, bodies, or executors. Instead it
//! provides the small, allocation-conscious building blocks that generated code
//! (emitted by the companion `rest_over_grpc_build` crate from `google.api.http`
//! annotations) plugs into:
//!
//! - [`Code`] and [`map_code_to_http`] / [`map_http_to_code`] translate between
//!   gRPC status codes and HTTP status codes following the conventions used by
//!   the reference gRPC-Gateway and Google API gateways.
//! - [`Status`] and [`HttpResponse`] are the neutral request/response value
//!   types that generated dispatchers return.
//! - The [`transcode`] module provides serde-based JSON⇄message request/response
//!   transcoding, and the dispatch primitives ([`scan_segments`],
//!   [`RouteMatch`], [`Binding`], …) back the generated static router.
//!
//! Path-template parsing (the `google.api.http` pattern grammar such as
//! `shelves/{shelf}/books/{book=**}`) lives in the separate `http_path_template`
//! crate. A generated router needs it only at build time: `rest_over_grpc_build`
//! lowers the parsed templates into a static match tree, so no template parsing
//! or matching happens at runtime.
//!
//! # Features
//!
//! The core (always available) provides status mapping, the neutral message
//! types, the generated-router dispatch primitives, and JSON⇄protobuf message
//! coding (via `pbjson`) in [`transcode`]. Feature-gated modules add the rest:
//!
//! - `tower`: a [`tower_service::Service`] adapter ([`adapter::RestService`]).
//! - `layered`: a [`layered::Service`] adapter (the repository's `async fn`-based
//!   service trait), on the same [`adapter::RestService`].
//! - `streaming`: server-streaming response encodings — JSON array, NDJSON, and
//!   Server-Sent Events ([`stream`]).
//!
//! # Examples
//!
//! Decode captured path/query values into a request type, then encode the
//! response as JSON:
//!
//! ```
//! use rest_over_grpc::Binding;
//! use rest_over_grpc::transcode::{BodyKind, ResponseBodyKind, decode_request, encode_response};
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Debug, Deserialize, Serialize)]
//! struct GetShelf {
//!     shelf: String,
//!     theme: String,
//! }
//!
//! let bindings = [Binding::new(&["shelf"], "7")];
//! let request: GetShelf =
//!     decode_request(&bindings, &[("theme", "history")], b"", BodyKind::None)?;
//!
//! assert_eq!(request.shelf, "7");
//! assert_eq!(request.theme, "history");
//!
//! let body = encode_response(&request, ResponseBodyKind::Whole)?;
//! let value: serde_json::Value = serde_json::from_slice(&body)?;
//! assert_eq!(value["shelf"], "7");
//! assert_eq!(value["theme"], "history");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! See `examples/tower_service.rs` and `examples/layered_service.rs` for
//! feature-gated programs that wrap a neutral dispatcher as an
//! [`adapter::RestService`] and serve `http::Request`s through the `tower` and
//! `layered` ecosystems.

mod binding;
mod code;
mod http_response;
mod path;
mod route_match;
mod segments;
mod status;

#[cfg(any(feature = "tower", feature = "layered"))]
pub mod adapter;
#[cfg(feature = "streaming")]
pub mod stream;
pub mod transcode;

#[doc(inline)]
pub use binding::Binding;
#[doc(inline)]
pub use code::{Code, UnknownCode, map_code_to_http, map_http_to_code};
#[doc(inline)]
pub use http_response::HttpResponse;
#[doc(inline)]
pub use path::{parse_query, scan_segments, split_path, split_query, split_verb};
#[doc(inline)]
pub use route_match::RouteMatch;
#[doc(inline)]
pub use segments::{Segments, SegmentsIter};
#[doc(inline)]
pub use status::Status;
