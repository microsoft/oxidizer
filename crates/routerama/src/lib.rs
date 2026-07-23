// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![no_std]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/routerama/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/routerama/favicon.ico")]

//! HTTP routing, response composition, and query/form processing.
//!
//! Routerama exposes independent, feature-gated modules:
//!
//! - [`resolve`] provides typed static and runtime-configured path resolution.
//! - [`response`] provides HTTP body types and typed response composition.
//! - [`route`] provides generated handler dispatch and request extraction.
//! - [`query`] provides bounded query-string decoding and encoding.
//!
//! `route` enables `response`. The additive `json`, `form`, `mount`, and
//! `tower` features add bounded JSON/form extraction, erased runtime services,
//! and a `tower_service::Service` adapter. `bytesbuf` preserves fragmented
//! `BytesView` request and response data and supports caller-provided-memory
//! templates under `no_std + alloc`; `bytesbuf-std` adds `GlobalPool` and
//! standard-I/O JSON decoding. No features are enabled by default, and the
//! crate root re-exports no feature-specific API.
//!
//! # Example
//!
//! ```
//! # #[cfg(feature = "route")]
//! # async fn example() {
//! use routerama::response::Body;
//! use routerama::route::{Request, State, StatusCode, router};
//!
//! #[derive(Clone)]
//! struct AppState(&'static str);
//!
//! struct Api;
//!
//! #[router(state = AppState)]
//! impl Api {
//!     #[route(GET, "/books/{id}")]
//!     async fn book(&self, id: u32, state: State<AppState>) -> String {
//!         format!("{}:{id}", state.0.0)
//!     }
//!
//!     #[fallback]
//!     async fn fallback(&self, failure: routerama::route::RouteFailure<'_>) -> StatusCode {
//!         failure.status()
//!     }
//! }
//!
//! let request = Request::get("/books/42")
//!     .body(Body::empty())
//!     .expect("valid request");
//! let response = Api.route(request, &AppState("main")).await;
//! assert_eq!(response.status(), StatusCode::OK);
//! # }
//! ```
//!
//! See each module and the crate's runnable examples for extraction,
//! predicates, dynamic routes, interceptors, mounted services, and transport
//! integration.
//!
//! # `no_std`
//!
//! Routerama is `#![no_std]` and uses `alloc` where owned storage is required.
//! Procedural macros execute on the host. Features that depend on HTTP response
//! types enable their required `std` support.

extern crate alloc;
extern crate self as routerama;
#[cfg(test)]
extern crate std;
#[cfg(all(not(test), feature = "bytesbuf-std"))]
extern crate std;

#[cfg(any(feature = "resolve", feature = "route"))]
mod affix_edge;
#[cfg(any(feature = "resolve", feature = "route"))]
mod build_error_entry;
#[cfg(any(feature = "resolve", feature = "route"))]
mod captures;
#[cfg(any(feature = "resolve", feature = "route"))]
mod codegen_helpers;
#[cfg(any(feature = "resolve", feature = "route"))]
mod configuration_error;
#[cfg(any(feature = "resolve", feature = "route"))]
mod decode;
#[cfg(any(feature = "resolve", feature = "route"))]
mod dyn_builder;
#[cfg(any(feature = "resolve", feature = "route"))]
mod dyn_route;
#[cfg(any(feature = "resolve", feature = "route"))]
#[path = "extract_helpers.rs"]
mod extract_helpers;
#[cfg(any(feature = "resolve", feature = "route"))]
mod http_method;
#[cfg(any(feature = "resolve", feature = "route"))]
mod literal_edge;
#[cfg(feature = "query")]
mod primitive;
#[cfg(any(feature = "resolve", feature = "route"))]
mod raw_match;
#[cfg(any(feature = "resolve", feature = "route"))]
mod raw_resolver;
#[cfg(any(feature = "resolve", feature = "route"))]
mod resolve_error;
#[cfg(any(feature = "resolve", feature = "route"))]
mod resolver;
#[cfg(any(feature = "resolve", feature = "route"))]
mod route_match;
#[cfg(any(feature = "resolve", feature = "route"))]
mod rt_node;
#[cfg(any(feature = "resolve", feature = "route"))]
mod walk;

#[cfg(any(feature = "resolve", feature = "route"))]
pub mod path;
#[cfg(feature = "query")]
pub mod query;
#[cfg(feature = "resolve")]
pub mod resolve;
#[cfg(feature = "response")]
pub mod response;
#[cfg(feature = "route")]
pub mod route;

#[cfg(any(feature = "resolve", feature = "route"))]
use configuration_error::ConfigurationError;
#[cfg(any(feature = "resolve", feature = "route"))]
use http_method::HttpMethod;
#[cfg(any(feature = "resolve", feature = "route"))]
use resolve_error::ResolveError;
