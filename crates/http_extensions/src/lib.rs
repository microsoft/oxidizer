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
//! This crate provides common HTTP features that build on the [`http`] crate.
//! It includes body handling, error types, and extension traits for HTTP requests
//! and responses.
//!
//! # Core Types
//!
//! - [`HttpRequest`] and [`HttpResponse`] are aliases for requests and responses with [`HttpBody`].
//! - [`HttpRequestBuilder`] constructs HTTP requests with chained method calls.
//! - [`HttpResponseBuilder`] constructs HTTP responses with chained method calls.
//! - [`HttpBody`] supports text, binary, JSON, and streaming content.
//! - [`HttpBodyBuilder`] creates HTTP bodies with a configurable memory provider.
//! - [`HttpError`] provides backtraces and recovery information.
//! - [`RequestHandler`] defines HTTP middleware and request pipelines.
//!
//! # Extension Traits
//!
//! The crate provides extension traits for standard HTTP types:
//!
//! - [`StatusExt`] classifies status codes and their recovery behavior.
//! - [`RequestExt`] extends HTTP requests.
//! - [`ResponseExt`] classifies response recovery behavior and supports `Retry-After`.
//! - [`HttpRequestExt`] clones requests and their bodies.
//! - [`HeaderMapExt`] extracts and parses header values.
//! - [`HeaderValueExt`] constructs [`HeaderValue`][http::HeaderValue] from [`Bytes`][bytes::Bytes].
//! - [`ExtensionsExt`] extracts URI template labels from [`Extensions`][http::Extensions].
//!
//! # Quick Start
//!
//! The following example creates an HTTP client, builds a request, and requires
//! a successful response:
//!
//! ```rust
//! # #[cfg(not(feature = "test-util"))] fn main() {}
//! # #[cfg(feature = "test-util")]
//! # use http_extensions::{
//! #     HttpRequestBuilder, HttpResponseBuilder, HttpBodyBuilder, HttpRequestBuilderExt,
//! #     FakeHandler, StatusExt, Result,
//! # };
//! # #[cfg(feature = "test-util")]
//! # #[tokio::main]
//! # async fn main() -> Result<()> {
//! // Create a builder for request and response bodies.
//! let body_builder = HttpBodyBuilder::new_fake();
//!
//! // Create a fake handler that returns a successful response.
//! // The `test-util` feature provides this test handler.
//! let handler = FakeHandler::from(
//!     HttpResponseBuilder::new(&body_builder)
//!         .status(200)
//!         .header("Content-Type", "application/json")
//!         .text(r#"{"message": "Success"}"#)
//!         .build()?,
//! );
//!
//! // Build and send an HTTP request through the handler.
//! let response = handler
//!     .request_builder()
//!     .get("https://api.example.com/data")
//!     .header("Authorization", "Bearer token")
//!     .fetch()
//!     .await?;
//!
//! // Return an error if the response does not have a `2xx` status code.
//! let validated_response = response.ensure_success()?;
//!
//! println!("response status: {}", validated_response.status());
//! # Ok(())
//! # }
//! ```
//!
//! This example uses the `test-util` feature to create a [`FakeHandler`].
//! Production code uses an HTTP client that implements [`RequestHandler`].
//! In both cases, [`HttpRequestBuilder`] builds the request and the handler sends it.
//! [`StatusExt::ensure_success`] then requires a successful response.
//!
//! # Integration with the HTTP Ecosystem
//!
//! This crate builds on the [`http`] crate:
//!
//! - It uses [`http::Request`] and [`http::Response`] as base types.
//! - It reuses [`http::Method`], [`http::StatusCode`], and [`http::HeaderMap`].
//! - It implements standard traits such as [`http_body::Body`].
//! - It works with other Rust HTTP libraries.
//!
//! # Examples
//!
//! ## Requiring a Successful Response
//! ```rust
//! # fn main() {
//! # #[cfg(feature = "test-util")] {
//! # (|| {
//! # use http_extensions::{StatusExt, HttpResponse, HttpResponseBuilder, HttpError};
//! # let response: HttpResponse = HttpResponseBuilder::new_fake().status(200).build().unwrap();
//! // Return an error if the response was not successful.
//! let validated_response = response.ensure_success()?;
//! # Ok::<(), HttpError>(())
//! # })().unwrap();
//! # }
//! # }
//! ```
//!
//! ## Creating Request Bodies
//! ```rust
//! # fn main() {
//! # #[cfg(feature = "test-util")] {
//! # use http_extensions::HttpBodyBuilder;
//! # let builder = HttpBodyBuilder::new_fake();
//! // Create different body types.
//! let text_body = builder.text("Hello, world!");
//! let binary_body = builder.slice(&[1, 2, 3, 4]);
//! let empty_body = builder.empty();
//! # }
//! # }
//! ```
//!
//! ## Building HTTP Requests
//! ```rust
//! # fn main() {
//! # #[cfg(feature = "test-util")] {
//! # use http_extensions::{HttpRequestBuilder, HttpBodyBuilder};
//! # let body_builder = HttpBodyBuilder::new_fake();
//! let request = HttpRequestBuilder::new(&body_builder)
//!     .get("https://api.example.com/data")
//!     .text("Hello World")
//!     .build()
//!     .unwrap();
//! # }
//! # }
//! ```
//!
//! ## Building HTTP Responses
//! ```rust
//! # fn main() {
//! # #[cfg(feature = "test-util")] {
//! # use http_extensions::{HttpResponseBuilder, HttpBodyBuilder};
//! # let body_builder = HttpBodyBuilder::new_fake();
//! let response = HttpResponseBuilder::new(&body_builder)
//!     .status(200)
//!     .header("Content-Type", "text/plain")
//!     .body(body_builder.text("Success"))
//!     .build()
//!     .unwrap();
//! # }
//! # }
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
//! The `test-util` feature provides [`FakeHandler`].
//! It returns test responses without network requests.
//! Use it to test code that depends on HTTP clients.
//!
//! # Features
//!
//! - `json` enables JSON serialization and deserialization through [`Json`].
//! - `test-util` enables test implementations.
//!
//! # Memory Management
//!
//! Methods that allocate body data use the [`bytesbuf`] memory provider configured on
//! [`HttpBodyBuilder`].
//! Methods that wrap existing body data keep the storage supplied by the caller.
//! A pooled provider can reuse memory after the body data is consumed.

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

mod error_labels;

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

mod http_request_builder_ext;
pub use http_request_builder_ext::HttpRequestBuilderExt;

mod extensions;
pub use extensions::{ExtensionsExt, HeaderMapExt, HeaderValueExt, HttpRequestExt, RequestExt, ResponseExt, StatusExt};

mod uri_template_label;
pub use uri_template_label::UriTemplateLabel;

pub mod timeout;

pub mod routing;

mod http_response_builder;
pub use http_response_builder::HttpResponseBuilder;

mod http_request_builder;
pub(crate) mod http_utils;

pub use http_request_builder::HttpRequestBuilder;

#[cfg(any(feature = "test-util", test))]
mod fake_handler;
#[cfg(any(feature = "test-util", test))]
pub use fake_handler::FakeHandler;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod testing;

#[cfg(any(doc, test))]
pub mod documentation;
