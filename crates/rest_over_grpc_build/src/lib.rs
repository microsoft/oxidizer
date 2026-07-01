// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/rest_over_grpc_build/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/rest_over_grpc_build/favicon.ico"
)]

//! Build-time code generation that lowers `google.api.http`-annotated gRPC
//! services into a framework-neutral REST router.
//!
//! This crate is intended to be called from a consumer's `build.rs`. It is the
//! codegen half of the gRPC→REST transcoding system; the runtime half lives in
//! `rest_over_grpc`. It is deliberately *not* a proc-macro: codegen is driven from
//! external descriptors (`.proto` / `FileDescriptorSet` + HTTP annotations),
//! which a `build.rs` is far better suited to read than a macro.
//!
//! # Pipeline
//!
//! 1. Describe each RPC's HTTP binding with an [`HttpRule`] (mirroring
//!    [`google.api.HttpRule`](https://github.com/googleapis/googleapis/blob/master/google/api/http.proto)).
//! 2. [`HttpRule::lower`] turns a rule (plus its `additional_bindings`) into one
//!    or more [`Route`]s, each pairing an HTTP method + parsed
//!    [`PathTemplate`](http_path_template::PathTemplate) with its body / response-body
//!    configuration.
//! 3. [`Router::new`] collects the routes for a service and
//!    [`Router::generate`] emits the static dispatch code as a
//!    [`TokenStream`](proc_macro2::TokenStream).
//!
//! The emitted router performs no runtime trie/regex construction: it is
//! straight-line generated Rust that matches the HTTP method and path segments
//! and reports the resolved RPC plus its captured path-variable bindings.
//!
//! # Scope
//!
//! Codegen handles unary RPCs: the HTTP-rule model, lowering (with
//! `additional_bindings`, `response_body`, and custom verbs), the static
//! router, the async service trait, and the request/response dispatcher, with
//! path/query/body binding wired through `rest_over_grpc::transcode`. Streaming
//! RPCs are rejected at codegen time; the streaming response encodings live in
//! `rest_over_grpc::stream`.
//!
//! # Examples
//!
//! Build an HTTP rule, lower it into routes, and generate a static router:
//!
//! ```
//! use rest_over_grpc_build::{HttpMethod, HttpRule, Router};
//!
//! let routes = HttpRule::new("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}")
//!     .lower()
//!     .expect("the path template is valid");
//! let tokens = Router::new(routes).generate();
//!
//! assert!(tokens.to_string().contains("pub fn resolve"));
//! ```
//!
//! To inspect a larger generated service,
//! `examples/generate_service.rs` builds [`HttpRule`]s, lowers them, and
//! pretty-prints the generated service trait + dispatcher:
//!
//! ```text
//! cargo run -p rest_over_grpc_build --example generate_service
//! ```

mod annotations;
mod body;
mod codegen;
#[cfg(feature = "descriptor")]
mod descriptor;
mod http_method;
mod http_rule;
mod message_types;
mod response_body;
mod route;
mod rule_error;
mod service;
mod service_method;

#[doc(inline)]
pub use annotations::{ANNOTATIONS_PROTO, HTTP_PROTO, write_annotation_protos};
#[doc(inline)]
pub use body::Body;
#[doc(inline)]
pub use codegen::Router;
#[cfg(feature = "descriptor")]
#[doc(inline)]
pub use descriptor::{DescriptorError, services_from_descriptor};
#[doc(inline)]
pub use http_method::HttpMethod;
#[doc(inline)]
pub use http_rule::HttpRule;
#[doc(inline)]
pub use message_types::MessageTypes;
#[doc(inline)]
pub use response_body::ResponseBody;
#[doc(inline)]
pub use route::Route;
#[doc(inline)]
pub use rule_error::RuleError;
#[doc(inline)]
pub use service::Service;
#[doc(inline)]
pub use service_method::ServiceMethod;
