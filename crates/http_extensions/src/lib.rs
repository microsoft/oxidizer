// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/http_extensions/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/http_extensions/favicon.ico"
)]

//! Shared HTTP types and extension traits for clients and servers.
//!
//! This crate provides common HTTP functionality built on the popular [`http`] crate,
//! including flexible body handling, unified error types, and ergonomic extension traits
//! for working with HTTP requests and responses.
//!
//! # Core Types
//!
//! - [`HttpRequest`] and [`HttpResponse`] - Type aliases for requests and responses with [`HttpBody`]
//! - [`HttpRequestBuilder`] - Builder for constructing HTTP requests with a fluent API
//! - [`HttpResponseBuilder`] - Builder for constructing HTTP responses with a fluent API
//! - [`HttpBody`] - Flexible body type supporting text, binary, JSON, and streaming content
//! - [`HttpBodyBuilder`] - Builder for creating HTTP bodies with memory pool optimization
//! - [`HttpError`] - Unified error type with automatic backtraces and recovery classification
//! - [`RequestHandler`] - Trait for HTTP middleware and request processing pipelines
//!
//! # Extension Traits
//!
//! The crate provides extension traits that add convenience methods to standard HTTP types:
//!
//! - [`StatusExt`] - Status code validation and recovery classification
//! - [`RequestExt`] - Extensions for HTTP requests
//! - [`ResponseExt`] - Response recovery classification with `Retry-After` support
//! - [`HttpRequestExt`] - Request cloning with body support
//! - [`HeaderMapExt`] - Header value extraction and parsing
//! - [`HeaderValueExt`] - Construction of [`HeaderValue`][http::HeaderValue] from [`Bytes`][bytes::Bytes]
//!
//! # Quick Start
//!
//! Here's a complete example showing how to create an HTTP client, build a request,
//! and validate the response:
//!
//! ```rust
//! # use http_extensions::{
//! #     HttpRequestBuilder, HttpResponseBuilder, HttpBodyBuilder, HttpRequestBuilderExt,
//! #     FakeHandler, StatusExt, Result,
//! # };
//! # #[tokio::main]
//! # async fn main() -> Result<()> {
//! // Create a body builder for constructing request/response bodies
//! let body_builder = HttpBodyBuilder::new_fake();
//!
//! // Create a fake handler that returns a successful response
//! // (This uses the `test-util` feature for testing; similar workflow applies to real clients)
//! let handler = FakeHandler::from(
//!     HttpResponseBuilder::new(&body_builder)
//!         .status(200)
//!         .header("Content-Type", "application/json")
//!         .text(r#"{"message": "Success"}"#)
//!         .build()?,
//! );
//!
//! // Build and send an HTTP request using the handler
//! let response = handler
//!     .request_builder()
//!     .get("https://api.example.com/data")
//!     .header("Authorization", "Bearer token")
//!     .fetch()
//!     .await?;
//!
//! // Validate that the response succeeded (returns error for `4xx/5xx` status codes)
//! let validated_response = response.ensure_success()?;
//!
//! println!("response status: {}", validated_response.status());
//! # Ok(())
//! # }
//! ```
//!
//! **Note**: This example uses the `test-util` feature to create a `FakeHandler` for testing.
//! In production code, you would use a real HTTP client that implements the
//! [`RequestHandler`] trait, but the workflow remains the same: build requests with
//! [`HttpRequestBuilder`], send them through a handler, and validate responses with
//! [`StatusExt::ensure_success`].
//!
//! # Integration with the HTTP Ecosystem
//!
//! This crate builds on the popular [`http`] crate rather than inventing new types:
//!
//! - Uses [`http::Request`] and [`http::Response`] as base types
//! - Reuses [`http::Method`], [`http::StatusCode`], and [`http::HeaderMap`]
//! - Implements standard traits like [`http_body::Body`] for ecosystem compatibility
//! - Works seamlessly with other Rust HTTP libraries
//!
//! # Examples
//!
//! ## Validating Response Status
//! ```rust
//! # use http_extensions::{StatusExt, HttpResponse, HttpResponseBuilder, HttpError};
//! # let response: HttpResponse = HttpResponseBuilder::new_fake().status(200).build().unwrap();
//! // Check if the response succeeded and return an error if not
//! let validated_response = response.ensure_success()?;
//! # Ok::<(), HttpError>(())
//! ```
//!
//! ## Creating Request Bodies
//! ```rust
//! # use http_extensions::HttpBodyBuilder;
//! # let builder = HttpBodyBuilder::new_fake();
//! // Create different body types
//! let text_body = builder.text("Hello, world!");
//! let binary_body = builder.slice(&[1, 2, 3, 4]);
//! let empty_body = builder.empty();
//! ```
//!
//! ## Building HTTP Requests
//! ```rust
//! # use http_extensions::{HttpRequestBuilder, HttpBodyBuilder};
//! # let body_builder = HttpBodyBuilder::new_fake();
//! let request = HttpRequestBuilder::new(&body_builder)
//!     .get("https://api.example.com/data")
//!     .text("Hello World")
//!     .build()
//!     .unwrap();
//! ```
//!
//! ## Building HTTP Responses
//! ```rust
//! # use http_extensions::{HttpResponseBuilder, HttpBodyBuilder};
//! # let body_builder = HttpBodyBuilder::new_fake();
//! let response = HttpResponseBuilder::new(&body_builder)
//!     .status(200)
//!     .header("Content-Type", "text/plain")
//!     .body(body_builder.text("Success"))
//!     .build()
//!     .unwrap();
//! ```
//!
//! ## Building Middleware with `RequestHandler`
//! ```rust
//! # use http_extensions::{HttpRequest, HttpResponse, RequestHandler, Result};
//! # use layered::Service;
//! struct LoggingMiddleware<S> {
//!     inner: S,
//! }
//!
//! impl<S: RequestHandler> Service<HttpRequest> for LoggingMiddleware<S> {
//!     type Out = Result<HttpResponse>;
//!
//!     async fn execute(&self, request: HttpRequest) -> Self::Out {
//!         println!("Processing request to: {}", request.uri());
//!         let response = self.inner.execute(request).await?;
//!         println!("Response status: {}", response.status());
//!         Ok(response)
//!     }
//! }
//! ```
//!
//! ## Testing with `FakeHandler`
//!
//! The `FakeHandler` type (available with the `test-util` feature) lets you mock HTTP responses
//! for testing without making actual network requests. This is useful for unit testing code
//! that depends on HTTP clients.
//!
//! # Features
//!
//! - `json` - Enables JSON serialization/deserialization support via `Json` type
//! - `test-util` - Enables fake implementations for testing
//!
//! # Memory Management
//!
//! Bodies created through [`HttpBodyBuilder`] use memory pools from [`bytesbuf`] to
//! reduce allocation overhead. When body data is consumed, memory is automatically recycled
//! for future requests. This makes the crate particularly efficient for high-throughput scenarios.

use http::{Request, Response};

/// Specialized HTTP request that uses the [`HttpBody`] type for the body.
///
/// This is a type alias for [`Request<HttpBody>`].
pub type HttpRequest = Request<HttpBody>;

/// Specialized HTTP response that uses the [`HttpBody`] type for the body.
///
/// This is a type alias for [`Response<HttpBody>`].
pub type HttpResponse = Response<HttpBody>;

mod error;
pub use error::{HttpError, Result};

mod body;
pub use body::{HttpBody, HttpBodyBuilder, HttpBodyOptions};

#[cfg(any(feature = "json", test))]
mod json;
#[doc(inline)]
#[cfg(any(feature = "json", test))]
pub use json::{Json, JsonError};

mod constants;

mod request_handler;
pub use request_handler::RequestHandler;

mod request_handler_ext;
pub use request_handler_ext::RequestHandlerExt;

mod http_request_builder_ext;
pub use http_request_builder_ext::HttpRequestBuilderExt;

mod extensions;
pub use extensions::{HeaderMapExt, HeaderValueExt, HttpRequestExt, RequestExt, ResponseExt, StatusExt};

mod url_template_label;
pub use url_template_label::UrlTemplateLabel;

pub mod timeout;

mod http_response_builder;
pub use http_response_builder::HttpResponseBuilder;

mod http_request_builder;
pub(crate) mod http_utils;

pub(crate) mod resilience;

pub use http_request_builder::HttpRequestBuilder;

#[cfg(any(feature = "test-util", test))]
mod fake_handler;
#[cfg(any(feature = "test-util", test))]
pub use fake_handler::FakeHandler;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod testing;

pub mod _documentation;
