// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! An HTTP client for resilient and observable services.
//!
//! ```
//! # async fn example(client: &fetch::HttpClient) -> fetch::Result<()> {
//! let response = client.get("https://example.com").fetch().await?;
//! assert!(response.status().is_success());
//! # Ok(())
//! # }
//! ```

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(
    not(feature = "json"),
    allow(
        rustdoc::broken_intra_doc_links,
        reason = "JSON links are unavailable when the json feature is disabled"
    )
)]

//! [`HttpClient`] supports different async runtimes and transports. It includes secure defaults,
//! resilience, telemetry, and test utilities.
//!
//! # Why a new HTTP client?
//!
//! `fetch` includes the features that services commonly need:
//!
//! - Secure TLS validation, retries, circuit breaking, hedging, and OpenTelemetry telemetry.
//! - Fake responses through the `test-util` feature.
//! - A request pipeline that supports custom logging, metrics, retries, and other handlers.
//! - Memory pooling and zero-copy operations for large responses.
//!
//! `fetch` does not require one runtime, I/O implementation, or HTTP transport. A transport handler
//! performs I/O at the end of the request pipeline. Resilience, telemetry, routing, logging, and
//! retries run above that handler.
//!
//! - Tokio works with the bundled transport. A custom transport can use another runtime and I/O
//!   implementation.
//! - A transport implements [`RequestHandler`]. It can use the bundled hyper transport, a custom
//!   client, or another client such as [`reqwest`](https://docs.rs/reqwest/).
//!
//! A library can use `fetch` while the application selects the runtime and transport. See the
//! [`custom`] module and [`custom::create_builder`] for an example.
//!
//! ## How does it compare to `reqwest`?
//!
//! Both `fetch` and [`reqwest`](https://docs.rs/reqwest/) use
//! [`hyper`](https://docs.rs/hyper/) by default. `fetch` also includes resilience and supports
//! custom runtimes and transports. A custom transport can use `reqwest` while the rest of the
//! `fetch` pipeline stays unchanged.
//!
//! | Feature | `fetch` | `reqwest` |
//! | ------- | -------------- | --------- |
//! | Runtime support | Tokio and custom runtimes | Tokio on native targets; browser implementation on `wasm32` |
//! | Custom transport or I/O | Built in | Not supported |
//! | TLS/HTTPS | rustls or native-tls | rustls or native-tls |
//! | Resilience | Retries and circuit breaking are enabled by default; hedging is built in and optional | Built-in retries; circuit breaking and hedging require external middleware |
//! | JSON support | Built in | Built in |
//! | Testing tools | Built in | Requires custom or external tools |
//! | OpenTelemetry metrics | Built in | Requires custom integration |
//! | Structured request logging | Built-in tracing events | Requires custom middleware |
//! | Advanced HTTP client features [^1] | Not supported yet [^2] | Available through optional features |
//! | Request pipeline | Built in | Requires custom or external tools |
//! | Zero-copy buffers | Built in | Partial support through `Bytes` |
//! | Linux support | Full support | Full support |
//!
//! [^1]: Advanced HTTP client features include file uploads, cookies, proxies, and redirects.
//! [^2]: Future versions can add the missing cookies, redirects, and forms features.
//!
//! > **Note**: `fetch` uses methods such as `get()`, `post()`, and `fetch()`, which are similar to
//! > `reqwest` methods.
//!
//! # Getting Started
//!
//! The bundled transport runs on Tokio. Use a custom transport for another runtime. See the
//! [`custom`] module.
//!
//! ```rust,no_run
//! # #[cfg(all(feature = "tokio", any(feature = "rustls", feature = "native-tls")))]
//! # {
//! use fetch::{HttpClient, HttpError};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), HttpError> {
//!     // Create a client with the bundled Tokio transport.
//!     let client: HttpClient = HttpClient::new_tokio();
//!
//!     // Return an error for a `4xx` or `5xx` status, then read the body as text.
//!     let body: String = client.get("https://example.com").fetch_text_body().await?;
//!
//!     println!("response: {body}");
//!
//!     Ok(())
//! }
//! # }
//! ```
//!
//! > **Note**: Use [`HttpClient::builder_tokio`] to add handlers or change timeouts and other
//! > configuration.
//!
//! # Making Requests
//!
//! Use methods such as [`HttpClient::get`] and [`HttpClient::post`] for common requests. Each method
//! returns a builder for request-specific configuration.
//!
//! ## GET Requests
//!
//! ```rust
//! # use fetch::{HttpClient, HttpError, HttpResponse};
//! # async fn example(client: &HttpClient) -> Result<(), HttpError> {
//! // Send a GET request.
//! let response: HttpResponse = client.get("https://www.example.com").fetch().await?;
//!
//! # Ok(())
//! # }
//! ```
//!
//! ## POST Requests
//!
//! ```rust
//! # use fetch::HttpClient;
//! # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
//! // Send a POST request with a text body.
//! let response = client
//!     .post("https://httpbin.org/post")
//!     .text("the exact body that is sent")
//!     .fetch()
//!     .await?;
//!
//! # Ok(())
//! # }
//! ```
//!
//! ## Other request methods
//!
//! The client has methods such as [`HttpClient::put`] and [`HttpClient::delete`]. Use
//! [`HttpClient::request`] for another HTTP method:
//!
//! ```rust
//! # use fetch::{HttpClient, HttpError};
//! # use http::Method;
//! # async fn example(client: &HttpClient) -> Result<(), HttpError> {
//! // Send a PATCH request.
//! let response = client
//!     .request(Method::PATCH, "https://api.example.com/items/42")
//!     .fetch()
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! A request can include headers, an HTTP version, and a body:
//!
//! ```rust
//! # use fetch::{HttpClient, HttpError};
//! # use http::{header, Version};
//! # async fn example(client: &HttpClient) -> Result<(), HttpError> {
//! let response = client
//!     .post("https://api.example.com/upload")
//!     // Add HTTP headers.
//!     .header(header::AUTHORIZATION, "Bearer token123")
//!     .header(header::CONTENT_TYPE, "application/json")
//!     // Select the HTTP version.
//!     .version(Version::HTTP_2)
//!     .text("{\"name\": \"document.pdf\"}")
//!     .fetch()
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! These methods return [`HttpRequestBuilder`], which configures and sends the request.
//!
//! ## Reuse a base URI
//!
//! Use [`HttpClientBuilder::base_uri`] to set one [`BaseUri`] for all requests.
//!
//! The client base URI overrides a base URI in a request.
//!
//! ```rust
//! # #[cfg(feature = "test-util")]
//! # {
//! # use http::StatusCode;
//! # use fetch::fake::FakeHandler;
//! # use fetch::HttpClientBuilder;
//! # use fetch::HttpResponseBuilder;
//! # use fetch::fake::FakeDeps;
//! # use templated_uri::BaseUri;
//! # async fn example(builder: HttpClientBuilder) -> Result<(), Box<dyn std::error::Error>> {
//! let client = builder
//!     .base_uri(BaseUri::from_static("https://example.com/api/v1/")) // The trailing slash is required.
//!     .build();
//!
//! // This request uses `https://example.com/api/v1/foo/bar`.
//! let response = client.get("/foo/bar").fetch().await?;
//! # Ok(())
//! # }
//! # }
//! ```
//!
//! # Handling Responses
//!
//! [`HttpRequestBuilder::fetch`] returns an [`HttpResponse`] with the body, status code, headers,
//! and extensions. `HttpResponse` is a type alias for [`Response<HttpBody>`].
//!
//! A response supports these operations:
//!
//! - [`HttpResponse::ensure_success`] returns an error if the status is not `2xx`.
//! - [`HttpResponse::status`] returns the HTTP status.
//! - [`HttpResponse::headers`] returns the response headers.
//! - [`HttpResponse::into_body`] returns the response body.
//! - [`HttpBody::into_text`] and [`HttpBody::into_bytes`] convert the body.
//! - [`HttpBody::into_json`] converts the body when the `json` feature is enabled.
//!
//! ```rust
//! # use fetch::{HttpBody, HttpClient, HttpResponse, StatusExt};
//! # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
//! // Send a GET request.
//! let mut response: HttpResponse = client.get("https://www.example.com").fetch().await?;
//!
//! // Return an error if the status is not successful.
//! response = response.ensure_success()?;
//!
//! // Read the headers.
//! println!("Headers: {}", response.headers().len());
//!
//! // Consume the response and return its body.
//! let body: HttpBody = response.into_body();
//!
//! // Read the body as text.
//! let text: String = body.into_text().await?;
//!
//! println!("Response body: {}", text);
//! # Ok(())
//! # }
//! ```
//!
//! ## Specialized Fetch Methods
//!
//! These methods fetch a response and convert its body:
//!
//! - [`fetch_text`][crate::HttpRequestBuilder::fetch_text] returns a string.
//! - [`fetch_bytes`][crate::HttpRequestBuilder::fetch_bytes] returns a `BytesView`.
//! - [`fetch_json`][crate::HttpRequestBuilder::fetch_json] returns owned JSON and requires `json`.
//! - [`fetch_json_ref`][crate::HttpRequestBuilder::fetch_json_ref] returns zero-copy JSON and
//!   requires `json`.
//!
//! Each method returns [`Response<T>`]. The response retains its status, headers, and extensions.
//!
//! ```rust
//! # use http::Response;
//! # use fetch::{HttpClient, StatusExt};
//! # use serde_json::json;
//! # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
//! // Return the response with a text body.
//! let response = client
//!     .get("https://api.example.com/users")
//!     .fetch_text()
//!     .await?;
//!
//! // Read response metadata before the body.
//! println!("Status: {}", response.status());
//! println!("Content-Type: {:?}", response.headers().get("content-type"));
//!
//! // Return an error for an unsuccessful status, then extract the body.
//! let text: String = response.ensure_success()?.into_body();
//!
//! # Ok(())
//! # }
//! ```
//!
//! ## Body-Only Shortcuts
//!
//! The `_body` variants call [`ensure_success`][crate::HttpResponse::ensure_success], discard the
//! response metadata, and return the converted body.
//!
//! - [`fetch_text_body`][crate::HttpRequestBuilder::fetch_text_body] returns a `String`.
//! - [`fetch_bytes_body`][crate::HttpRequestBuilder::fetch_bytes_body] returns a `BytesView`.
//! - [`fetch_json_body`][crate::HttpRequestBuilder::fetch_json_body] returns an owned value and
//!   requires `json`.
//!
//! ```rust
//! # use fetch::HttpClient;
//! # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
//! // Fetch the response, require a successful status, and return the text body.
//! let text: String = client
//!     .get("https://api.example.com/users")
//!     .fetch_text_body()
//!     .await?;
//! println!("body: {text}");
//! # Ok(())
//! # }
//! ```
//!
//! # URL Handling
//!
//! The HTTP client uses [`templated_uri`] for URLs. [`Uri`] supports dynamic path segments and
//! query parameters through templates.
//!
//! Templates follow [RFC 6570](https://datatracker.ietf.org/doc/html/rfc6570) level 3. [`Uri`] and
//! string types also support URLs without typed templates. In that case, the full `PathAndQuery`
//! string becomes the template.
//!
//! [`handlers::Logging`] records the URL template as `url.path.template`.
//!
//! This example creates a [`Uri`] with a templated path:
//!
//! ```rust
//! # use fetch::HttpClient;
//! use templated_uri::{BaseUri, EscapedString, PathAndQueryTemplate, Uri, templated};
//!
//! #[templated(template = "/users/{user_id}/", unredacted)]
//! #[derive(Clone)]
//! struct UserPath {
//!     user_id: EscapedString, // EscapedString escapes the value for use in URIs.
//! }
//!
//! # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
//! let user_path = UserPath {
//!     user_id: EscapedString::from_static("12345"),
//! };
//!
//! client
//!     .get(
//!         Uri::default()
//!             .with_base(BaseUri::from_static("https://api.example.com"))
//!             .with_path_and_query(user_path),
//!     )
//!     .fetch_text()
//!     .await?;
//!
//! # Ok(())
//! # }
//! ```
//!
//! ## Classification in URLs
//!
//! `templated_uri` uses `data_privacy` to classify URL paths and queries. The `classified` attribute
//! lets a [`PathAndQueryTemplate`](templated_uri::PathAndQueryTemplate) use classified
//! `data_privacy` types.
//!
//! ```
//! use data_privacy::Sensitive;
//! use templated_uri::{EscapedString, PathAndQueryTemplate, templated};
//!
//! #[templated(template = "/{org_id}/user/{user_id}")]
//! struct UserPath {
//!     #[unredacted]
//!     org_id: u32,
//!     user_id: Sensitive<EscapedString>,
//! }
//! ```
//!
//! # JSON Support
//!
//! The `json` feature supports these operations:
//!
//! - [`HttpRequestBuilder::json`][crate::HttpRequestBuilder::json] serializes a Rust type as JSON.
//! - [`HttpRequestBuilder::fetch_json`][crate::HttpRequestBuilder::fetch_json] deserializes into an
//!   owned Rust type. The value can outlive the response. It can cross thread boundaries if its
//!   type implements `Send`.
//! - [`HttpRequestBuilder::fetch_json_ref`][crate::HttpRequestBuilder::fetch_json_ref] returns
//!   [`Json<T>`][crate::Json]. It can borrow strings and byte arrays from the response buffer.
//! - [`HttpBody::into_json`][crate::HttpBody::into_json] converts a response body into JSON.
//!
//! ## Owned JSON with `fetch_json`
//!
//! Use `fetch_json` for owned data. It deserializes JSON directly into the target type:
//!
//! ```rust
//! # use fetch::{HttpClient, Response, StatusExt};
//! # use serde::{Deserialize, Serialize};
//! # #[cfg(feature = "json")]
//! # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
//! // Define a type with owned data.
//! #[derive(Serialize, Deserialize)]
//! struct Person {
//!     id: u32,
//!     name: String,
//! }
//!
//! let person = Person {
//!     id: 1,
//!     name: "Alice Johnson".to_owned(),
//! };
//!
//! // Send and receive owned JSON.
//! let response: Response<Person> = client
//!     .put("https://api.company.com/people")
//!     .json(&person)
//!     .fetch_json::<Person>()
//!     .await?;
//!
//! // Read the response metadata.
//! let response = response.ensure_success()?;
//!
//! // Extract the deserialized value.
//! let person: Person = response.into_body();
//!
//! println!("Person retrieved, name: {}", person.name);
//!
//! # Ok(())
//! # }
//! ```
//!
//! [`fetch_json_body`][crate::HttpRequestBuilder::fetch_json_body] fetches the response, requires a
//! successful status, and returns the owned value.
//!
//! ## Zero-Copy JSON with `fetch_json_ref`
//!
//! Pair `fetch_json_ref` with borrowed string fields such as `Cow<'a, str>`. [`Json<T>`][crate::Json]
//! borrows from the response buffer. Prefer `Cow<'a, str>` over `&'a str`. `Cow` uses an owned value
//! when an escaped JSON string cannot be borrowed.
//!
//! ```rust
//! # use fetch::{HttpClient, Response, StatusExt};
//! # #[cfg(feature = "json")]
//! # use fetch::Json;
//! # use serde::{Deserialize, Serialize};
//! # use std::borrow::Cow;
//! # #[cfg(feature = "json")]
//! # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
//! // Define a type that can borrow string data.
//! #[derive(Serialize, Deserialize)]
//! struct Person<'a> {
//!     id: u32,
//!     #[serde(borrow)]
//!     name: Cow<'a, str>,
//! }
//!
//! let person = Person {
//!     id: 1,
//!     name: "Alice Johnson".into(),
//! };
//!
//! // Send and receive zero-copy JSON.
//! let response: Response<Json<Person>> = client
//!     .put("https://api.company.com/people")
//!     .json(&person)
//!     .fetch_json_ref::<Person>()
//!     .await?;
//!
//! // Read the response metadata.
//! let response = response.ensure_success()?;
//!
//! // Extract the JSON wrapper.
//! let mut json_body: Json<Person> = response.into_body();
//!
//! // Parse the JSON data using zero-copy deserialization.
//! // The parsed Person borrows string data from the underlying buffer.
//! let person: Person = json_body.read()?;
//!
//! println!("Person retrieved, name: {}", person.name);
//!
//! # Ok(())
//! # }
//! ```
//!
//! Borrowed fields reduce heap allocations and copies. The parsed value cannot outlive the response
//! buffer.
//!
//! # Request Pipeline
//!
//! Each request passes through a sequence of [`RequestHandler`] implementations.
//!
//! A handler can:
//!
//! - Modify the request before the next handler receives it.
//! - Return a response without calling the next handler, for example to serve a cached response.
//! - Process the response from the next handler.
//! - Add logging, metrics, or other shared behavior.
//!
//! The transport handler is the final [`RequestHandler`]. It performs I/O and returns a response.
//! `fetch` supplies the resilience, telemetry, routing, and logging layers above it. The transport
//! can use the bundled hyper implementation, custom runtime and I/O code, or another client such
//! as [`reqwest`](https://docs.rs/reqwest/). See [`custom`] and [`custom::create_builder`] for a
//! custom transport.
//!
//! ## Built-in Pipeline Types
//!
//! The client supports three pipeline types:
//!
//! ### Standard Pipeline
//!
//! The standard pipeline is the default. The outermost handler processes the request first and the
//! response last.
//!
//! See [`StandardRequestPipeline`][crate::pipeline::StandardRequestPipeline] and
//! [`HttpClientBuilder::standard_pipeline`].
//!
//! ### Custom Pipeline
//!
//! A custom pipeline contains the handlers that its factory returns. See
//! [`HttpClientBuilder::custom_pipeline`].
//!
//! ### Minimal Pipeline
//!
//! The minimal pipeline contains only [`Dispatch`][crate::handlers::Dispatch]. `Dispatch` passes
//! each request to the transport.
//!
//! ```rust
//! # use fetch::HttpClientBuilder;
//! # fn example(mut builder: HttpClientBuilder) -> Result<(), Box<dyn std::error::Error>> {
//! // Create a client with only the dispatch handler.
//! let minimal_client = builder.minimal_pipeline().build();
//!
//! // Wrap the client with custom processing.
//! let wrapped_client = MyHttpWrapper::new(minimal_client);
//! # Ok(())
//! # }
//! # struct MyHttpWrapper;
//! # impl MyHttpWrapper { fn new<T>(_: T) -> Self { Self } }
//! ```
//!
//! Use this pipeline when another component supplies all middleware.
//!
//! ## Creating Custom Handlers
//!
//! See [`RequestHandler`] to implement request or response processing.
//!
//! # Testing with the HTTP Client
//!
//! Enable `test-util` to return fake responses without network I/O.
//!
//! A fake client can:
//!
//! - Return different HTTP responses.
//! - Exercise retry and error handling.
//! - Keep tests deterministic by avoiding network calls.
//! - Reproduce cases that are difficult to produce with a service.
//!
//! `HttpClient::new_fake` creates a client from predefined responses or a
//! [`FakeHandler`](crate::fake::FakeHandler):
//!
//! ```rust
//! # #[cfg(feature = "test-util")]
//! # fn example() {
//! # use fetch::{HttpClient, HttpResponseBuilder};
//! # use fetch::fake::FakeHandler;
//! # use http::StatusCode;
//!
//! // Always return a 200 response.
//! let client = HttpClient::new_fake(StatusCode::OK);
//!
//! // Return a sequence of responses without bodies.
//! let client = HttpClient::new_fake(vec![StatusCode::OK, StatusCode::INTERNAL_SERVER_ERROR]);
//!
//! // Create a fake response.
//! let response = HttpResponseBuilder::new_fake()
//!     .status(StatusCode::INTERNAL_SERVER_ERROR)
//!     .text("fake text")
//!     .build()
//!     .expect("always succeeds");
//!
//! // Always return the same response.
//! let client = HttpClient::new_fake(response);
//!
//! // Use a synchronous or asynchronous custom handler.
//! let fake_handler = FakeHandler::from_fn(|req| {
//!     println!("Fake handler called for request, url: {}", req.uri());
//!
//!     HttpResponseBuilder::new_fake()
//!         .status(StatusCode::INTERNAL_SERVER_ERROR)
//!         .text("fake text")
//!         .build()
//! });
//!
//! // Create a client that uses the custom handler.
//! let client = HttpClient::new_fake(fake_handler);
//! # }
//! ```
//!
//! # Memory management with `BytesView`
//!
//! `fetch` uses [`BytesView`] for request and response bodies. It provides:
//!
//! - Memory pooling.
//! - Reduced copying.
//! - A single view over multiple chunks.
//! - Zero-copy operations when the data layout permits them.
//! - Compatibility with the [`bytes`] crate.
//!
//! [`BytesView`] implements [`Buf`]. Use [`BytesBuf`], which implements [`BufMut`], to build a
//! mutable body.
//!
//! ```rust
//! # use fetch::HttpClient;
//! # use bytesbuf::{BytesView};
//! # use bytes::Buf;
//! #
//! # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
//! // Require a successful status and return the body as BytesView.
//! let mut body_bytes = client.get("https://example.com").fetch_bytes_body().await?;
//!
//! // Use methods from bytes::Buf.
//! let length = body_bytes.remaining();
//!
//! // Copy part of the body when required.
//! let mut buffer = vec![0; 10.min(length)];
//! if !body_bytes.is_empty() {
//!     body_bytes.copy_to_slice(&mut buffer);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! This representation reduces memory use and copies for large bodies.
//!
//! [`BytesView`]: bytesbuf::BytesView
//! [`BytesBuf`]: bytesbuf::BytesBuf
//! [`Buf`]: bytes::Buf
//! [`BufMut`]: bytes::BufMut
//! [`bytes`]: https://docs.rs/bytes
//!
//! # Performance
//!
//! Reuse clients, parse repeated URIs once, and keep binary data in [`BytesView`]:
//!
//! ```rust,no_run
//! # use fetch::{HttpClient, Uri};
//! # #[cfg(all(feature = "tokio", any(feature = "rustls", feature = "native-tls")))]
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create one client and reuse it.
//! let client = HttpClient::new_tokio();
//!
//! // Parse repeated URIs once.
//! let users_uri: Uri = "https://api.example.com/users".parse()?;
//! let items_uri: Uri = "https://api.example.com/items".parse()?;
//!
//! // Keep binary data in BytesView.
//! let bytes = client.get(users_uri.clone()).fetch_bytes_body().await?;
//! process_binary_data(bytes);
//! # Ok(())
//! # }
//! # fn process_binary_data(bytes: bytesbuf::BytesView) {}
//! ```
//!
//! Creating an [`HttpClient`] sets up connection pools and security configuration. Create one client
//! and share it across tasks. Parse repeated [`Uri`] values once. Converting [`BytesView`] to
//! `String` allocates and copies data, so keep binary or large bodies in [`BytesView`] when possible.
//!
//! # Integration with the HTTP Ecosystem
//!
//! `fetch` uses types from [`http`] and extensions from [`http_extensions`]. It re-exports common
//! types from both crates.
//!
//! # Resilience
//!
//! The [`seatbelt`] crate provides the built-in resilience layers.
//!
//! Client handlers and seatbelt middleware implement [`Service`][`layered::Service`], so the
//! pipeline can combine them without an adapter.
//!
//! The available patterns include:
//!
//! - Retries with configurable backoff.
//! - Timeouts.
//! - Circuit breakers.
//!
//! The [standard pipeline][crate::pipeline::StandardRequestPipeline] configures these patterns by
//! default.
//!
//! # TLS Support
//!
//! The client supports two TLS backends:
//!
//! - `rustls` is the default. It uses [`rustls`](https://docs.rs/rustls) with the
//!   [`aws-lc-rs`](https://docs.rs/aws-lc-rs) crypto provider. The `tls` feature enables it.
//! - `native-tls` uses `SChannel` on Windows, Security Framework on `macOS`, and `OpenSSL` on
//!   Linux. It is selected when `native-tls` is enabled without `rustls`.
//!
//! With `rustls`, [`rustls-platform-verifier`](https://docs.rs/rustls-platform-verifier) uses the
//! platform trust store. It:
//!
//! - Validates certificates against the operating system root `CAs`.
//! - Validates host names and certificate expiration.
//! - Applies TLS security policies.
//!
//! The default client applies the selected TLS backend:
//!
//! ```rust,no_run
//! # #[cfg(all(feature = "tokio", any(feature = "rustls", feature = "native-tls")))]
//! # {
//! use fetch::HttpClient;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = HttpClient::new_tokio();
//!
//!     // Send an HTTPS request.
//!     let response = client.get("https://www.example.com").fetch_text().await?;
//!
//!     Ok(())
//! }
//! # }
//! ```
//!
//! Enable `tls` and `tokio` to use the default rustls transport:
//!
//! ```toml
//! fetch = { version = "*", features = ["tls", "tokio"] }
//! ```
//!
//! Enable `native-tls` and `tokio` to use native TLS:
//!
//! ```toml
//! fetch = { version = "*", features = ["native-tls", "tokio"] }
//! ```
//!
//! If both backends are enabled, use
//! [`TlsOptions::builder_rustls()`](tls::TlsOptions::builder_rustls) or
//! `TlsOptions::builder_native_tls()` to select a backend for each client.
//!
//! # Automatic Response Decompression
//!
//! Automatic response decompression is off by default. Enable the required `compression-*`
//! features, or enable `compression-all`. Then select the formats through
//! [`DecompressionOptions`][options::DecompressionOptions]:
//!
//! ```
//! # #[cfg(all(feature = "test-util", feature = "compression-gzip"))]
//! # {
//! # use fetch::HttpClient;
//! # use fetch::fake::FakeDeps;
//! # use fetch::options::DecompressionFormat;
//! # use http::StatusCode;
//! # let builder = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default());
//! let client = builder.decompression(&[DecompressionFormat::Gzip]).build();
//! # }
//! ```
//!
//! Requests list the formats in `Accept-Encoding`, with the most preferred format first. Before the
//! client returns a response that uses a selected format, it removes `Content-Encoding` and
//! `Content-Length`. It decodes the body lazily as the caller reads it. If the response uses a
//! format that was not selected, the client returns it without changes.
//!
//! [`DecompressionFormat::ALL`](options::DecompressionFormat::ALL) asks for everything the build can
//! decompress.
//!
//! A malformed body fails when code reads it, not when the response arrives. This failure does not
//! trigger a retry. The client uses the `http_compression` defaults. The decompressed output limit
//! is 64 MiB. Each decompression stage accepts at most 1,024 concatenated streams. `fetch` does not
//! override these defaults.
//!
//! [`DecompressionOptions`][options::DecompressionOptions] sets limits for decompressed output and
//! compressed streams. Pass `None` to
//! [`max_output_len`][options::DecompressionOptions::max_output_len] to remove the output limit.
//! The limits also apply to streamed bodies. Exceeding a limit returns the
//! `compression_limit_exceeded` error label.
//!
//! [`response_body_options`](HttpClientBuilder::response_body_options) separately limits memory
//! used to buffer a body. Its default limit is 2 GiB.
//!
//! For server-side compression and decompression, use the
//! [`http_compression`](https://docs.rs/http_compression) crate directly.
//!
//! # Features
//!
//! Enable optional features in `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! fetch = { version = "*", features = ["json", "tokio", "tls", "compression-gzip"] }
//! ```
//!
//! - `tokio` enables the Tokio runtime integration. Combine it with `rustls`, `native-tls`, or the
//!   `tls` alias to provide `HttpClient::builder_tokio`,
//!   `HttpClient::builder_tokio_with_options`, and related APIs. The bundled transport requires a
//!   TLS feature even when the client permits HTTP.
//!
//! - `json` enables JSON serialization and deserialization. It provides methods such as
//!   `HttpRequestBuilder::json` and `HttpRequestBuilder::fetch_json`.
//!
//! - `tls` enables HTTPS through `rustls` and the `aws-lc-rs` crypto provider. It is an alias for
//!   `rustls`.
//!
//! - `rustls` enables the default TLS backend with `aws-lc-rs`.
//!
//! - `native-tls` enables `SChannel` on Windows, Security Framework on `macOS`, and `OpenSSL` on
//!   Linux. If both TLS backends are enabled, `rustls` is the default. Use
//!   `TlsOptions::builder_native_tls()` to select native TLS.
//!
//! - `test-util` provides fake responses and client behavior for tests without network I/O.
//!
//! The `compression-*` features select formats for
//! [automatic response decompression](#automatic-response-decompression). They are off by default.
//! A build without them links no compression implementation. Enabling a feature makes its format
//! available. The client does not decompress a response until
//! [`DecompressionOptions::formats`](options::DecompressionOptions::formats)
//! selects it and the client applies the options with
//! [`decompression`](HttpClientBuilder::decompression).
//!
//! - `compression-gzip` links `gzip` compression (RFC 1952) and adds
//!   [`DecompressionFormat::Gzip`](options::DecompressionFormat). The legacy `x-gzip` token also
//!   selects it.
//!
//! - `compression-deflate` links `deflate` compression and adds
//!   [`DecompressionFormat::Deflate`](options::DecompressionFormat). Despite the token, the HTTP
//!   `deflate` format is zlib-wrapped DEFLATE (RFC 1950), not raw DEFLATE (RFC 1951).
//!
//! - `compression-brotli` links Brotli compression (`br`, RFC 7932) and adds
//!   [`DecompressionFormat::Brotli`](options::DecompressionFormat).
//!
//! - `compression-zstd` links Zstandard compression (`zstd`, RFC 8878) and adds
//!   [`DecompressionFormat::Zstd`](options::DecompressionFormat).
//!
//! - `compression-all` enables all four compression features. Use
//!   [`DecompressionFormat::ALL`](options::DecompressionFormat::ALL) to request decompression with all of them.
//!
//! > **Note**: Enable `tokio` and `tls` for HTTPS with the bundled transport. Enable `json` for JSON APIs.
pub use ::http::{Extensions, HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode, Version};
pub use http_extensions::routing;
pub use seatbelt::{Recovery, RecoveryInfo};
pub use templated_uri::{BasePath, BaseUri, Origin, PathAndQuery, Uri};

pub mod http {
    //! Re-exports header, method, request, response, status, and version modules
    //! from the underlying `http` ecosystem crate.
    //!
    //! The most commonly used HTTP types remain available directly at the
    //! `fetch` crate root.

    pub use ::http::{header, method, request, response, status, version};
}

pub(crate) mod constants;

mod error_labels;

/// TLS backend configuration and client identity types.
pub mod tls;

mod client_builder;
mod dispatch_builder;
pub use client_builder::HttpClientBuilder;

/// Client transport, body, routing, and decompression options.
pub mod options;

mod client;
pub use client::HttpClient;

#[cfg(any(feature = "test-util", test))]
pub mod fake;

/// Custom runtime and transport integration.
pub mod custom;

#[cfg(all(feature = "tokio", any(feature = "rustls", feature = "native-tls")))]
/// Tokio transport integration.
pub mod tokio;

/// Built-in request pipeline handlers.
pub mod handlers;

/// Telemetry attributes and connection diagnostics.
pub mod telemetry;

pub use http_extensions::{
    HeaderMapExt, HeaderValueExt, HttpBody, HttpBodyBuilder, HttpError, HttpRequest, HttpRequestBuilder, HttpRequestExt, HttpResponse,
    HttpResponseBuilder, RequestExt, RequestHandler, ResponseExt, Result, StatusExt,
};
#[cfg(any(feature = "json", test))]
pub use http_extensions::{Json, JsonError};

/// HTTP-specific resilience layers and configuration.
pub mod resilience;

/// Standard, custom, and minimal request pipeline configuration.
pub mod pipeline;

/// Longer-form documentation for [`fetch`](crate).
#[cfg(any(doc, test))]
pub mod documentation;

// Installs a silent, always-interested global `tracing` subscriber before any
// unit test in this crate runs. This keeps `tracing` emission paths executing
// deterministically (never poisoned into the "disabled" state) and lets per-test
// thread-local subscribers compose safely. See `docs/tracing-tests.md`.
#[cfg(test)]
testing_aids::init_tracing!();
