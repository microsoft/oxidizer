// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(
    not(feature = "json"),
    allow(
        rustdoc::broken_intra_doc_links,
        reason = "json feature disabled, intra-doc links to json types will be broken"
    )
)]

//! A fast, safe HTTP client that just works.
//!
//! This crate provides a powerful HTTP client that works with different async runtimes, handles
//! security properly by default, and makes testing easy. The [`HttpClient`] provides a clean API
//! for making HTTP requests without worrying about the complex details of modern HTTP.
//!
//! # Why a new HTTP client?
//!
//! `fetch` bundles the capabilities real-world services need into a single client, ready to use
//! out of the box:
//!
//! - **Secure, resilient and observable by default**: Strong TLS validation, built-in resilience
//!   (retries, circuit breaking, hedging), and OpenTelemetry-compatible observability are
//!   pre-configured for real-world use.
//! - **Built-in testability**: The `test-util` feature lets you mock HTTP responses without complex
//!   setup, making tests fast and deterministic.
//! - **Composable pipeline**: Modular request handlers make it easy to add or customize behaviors
//!   like logging, metrics, or retries.
//! - **Memory efficient**: Uses smart pooling and zero-copy techniques to handle large responses
//!   with minimal overhead.
//!
//! Crucially, `fetch` delivers these features **without forcing a runtime, an I/O implementation, or
//! a particular HTTP transport on you**. The request pipeline is built around a *transport handler*
//! at its leaf that you can swap out, with everything above it — resilience, observability, routing,
//! logging, retries — layered on top. This makes `fetch`:
//!
//! - **runtime-agnostic**: Tokio works out of the box, or plug in any async runtime and I/O by
//!   supplying your own transport handler; and
//! - **transport-agnostic**: the transport handler is just a [`RequestHandler`] that turns a request
//!   into a response, so you can keep the bundled hyper transport, wrap a hand-rolled client, or even
//!   reuse an existing one like [`reqwest`](https://docs.rs/reqwest/).
//!
//! That makes `fetch` an excellent fit for **libraries that want to stay runtime- and
//! transport-agnostic**: they depend on `fetch` for its features while leaving the runtime and
//! transport choice to the consuming application, which plugs in whatever it already uses. See the
//! [`custom`] module and [`custom::create_builder`] for a worked example.
//!
//! ## How does it compare to `reqwest`?
//!
//! By default both `fetch` and [`reqwest`](https://docs.rs/reqwest/) are built on top of the powerful
//! [`hyper`](https://docs.rs/hyper/) HTTP implementation. While `reqwest` has been the go-to HTTP
//! client for many Rust applications, `fetch` offers a different set of trade-offs that may
//! better suit your needs, especially for crates that require resilience and multi-runtime support.
//! Unlike `reqwest`, `fetch` is not tied to its default transport at all: you can swap hyper out for
//! any transport — including `reqwest` itself — and keep all of `fetch`'s surrounding features.
//!
//! | Feature | `fetch` | `reqwest` |
//! | ------- | -------------- | --------- |
//! | **Runtime Support** | ✅ Tokio **and custom runtimes** | ✅ Tokio only |
//! | **Custom Transport / IO** | ✅ **Built-in** — plug in your own runtime, I/O, or even another HTTP client (e.g. `reqwest`) as the transport | ❌ Not supported |
//! | **TLS/HTTPS** | ✅ Via rustls or native-tls | ✅ Via rustls or native-tls |
//! | **Resilience** | ✅ Built-in and default | ❌ Optional, external crates required |
//! | **JSON support** | ✅ Built-in | ✅ Built-in |
//! | **Testing tools** | ✅ Built-in | ❌ Custom, external crates required |
//! | **`OTel` Metrics/Logging** | ✅ Built-in | ❌ Custom implementation needed |
//! | **Advanced HTTP Client Features [^1]** | ❌ Not yet supported [^2] | ✅ Via optional features |
//! | **Request Pipeline** | ✅ Built-in | ❌ Custom, external crates required |
//! | **Zero-copy Buffers** | ✅ Built-in | ❌ Partial, uses `Bytes` |
//! | **Linux support** | ✅ Full support | ✅ Full support |
//!
//! [^1]: Advanced HTTP client features include things like multipart uploads, cookies, proxies, and redirects.
//! [^2]: The features currently missing (cookies, redirects, forms) may be added in future versions as the
//! client matures.
//!
//! > **Note**: If you're already familiar with `reqwest`, you'll feel right at home with `fetch`.
//! > The APIs are intentionally similar, with familiar methods like `get()`, `post()`, and `fetch()`. Most
//! > basic HTTP operations follow the same patterns, making it easy to switch between the two libraries.
//!
//! # Getting Started
//!
//! This client runs on the Tokio runtime by default. (Other runtimes can be plugged in via a
//! custom transport — see the [`custom`] module.)
//!
//! ```rust,no_run
//! # #[cfg(all(feature = "tokio", any(feature = "rustls", feature = "native-tls")))]
//! # {
//! use fetch::{HttpClient, HttpError, Response, StatusExt};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), HttpError> {
//!     // Create a client using the builder
//!     let client: HttpClient = HttpClient::new_tokio();
//!
//!     // Retrieve the response as text
//!     let response: Response<String> = client
//!         .get("https://example.com")
//!         .fetch_text()
//!         .await?
//!         .ensure_success()?; // Verifies that the response was successful
//!
//!     println!("response: {}", response.body());
//!
//!     Ok(())
//! }
//! # }
//! ```
//!
//! > **Customization**: If you need to customize the HTTP client (e.g., add custom handlers, modify timeouts,
//! > or configure other options), use [`HttpClient::builder_tokio`] instead of `new_tokio` to access
//! > the full builder API.
//!
//! # Making Requests
//!
//! The HTTP client makes it easy to send different types of requests. Use convenient methods like
//! [`HttpClient::get`] and [`HttpClient::post`] for common operations, and the builder pattern to customize
//! your requests.
//!
//! ## GET Requests
//!
//! ```rust
//! # use fetch::{HttpClient, HttpError, HttpResponse};
//! # async fn example(client: &HttpClient) -> Result<(), HttpError> {
//! // Simple GET request
//! let response: HttpResponse = client
//!     .get("https://www.example.com")
//!     .fetch() // Fetch executes the request and returns a response
//!     .await?;
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
//! // POST request with text body
//! let response = client
//!     .post("https://httpbin.org/post")
//!     .text("the exact body that is sent") // Attaches a text body to the request
//!     .fetch()
//!     .await?;
//!
//! # Ok(())
//! # }
//! ```
//!
//! ## Handling Complex Requests
//!
//! The client supports all standard HTTP methods through dedicated methods like [`HttpClient::put`],
//! [`HttpClient::delete`], and more. For anything else, use [`HttpClient::request`] with any HTTP method:
//!
//! ```rust
//! # use fetch::{HttpClient, HttpError};
//! # use http::Method;
//! # async fn example(client: &HttpClient) -> Result<(), HttpError> {
//! // Using a custom method
//! let response = client
//!     .request(Method::PATCH, "https://api.example.com/items/42")
//!     .fetch()
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! You can customize requests with headers, specific HTTP versions, or by attaching bodies:
//!
//! ```rust
//! # use fetch::{HttpClient, HttpError};
//! # use http::{header, Version};
//! # async fn example(client: &HttpClient) -> Result<(), HttpError> {
//! let response = client
//!     .post("https://api.example.com/upload")
//!     // Add HTTP headers
//!     .header(header::AUTHORIZATION, "Bearer token123")
//!     .header(header::CONTENT_TYPE, "application/json")
//!     // Set HTTP version
//!     .version(Version::HTTP_2)
//!     .text("{\"name\": \"document.pdf\"}")
//!     .fetch()
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! All these methods return a [`HttpRequestBuilder`] object that lets you customize and then execute your request.
//!
//! ## Handling Multiple Requests to the Same Base URI
//!
//! If you need to make multiple requests to the same base URI efficiently, use the [`HttpClientBuilder::base_uri`] builder method.
//! This allows you to set a [`BaseUri`] for all requests, so you don't have to repeat the base URI each time.
//!
//! This setting overrides any base URI set in the URI you pass to the request methods.
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
//!     .base_uri(BaseUri::from_static("https://example.com/api/v1/")) // Trailing slash is mandatory
//!     .build();
//!
//! let response = client.get("/foo/bar").fetch().await?; // Full URL called by this is `https://example.com/api/v1/foo/bar`
//! # Ok(())
//! # }
//! # }
//! ```
//!
//! # Handling Responses
//!
//! When you call [`HttpRequestBuilder::fetch`], you get an [`HttpResponse`] with everything about the response -
//! the body, status code, headers, and more. Under the hood, `HttpResponse` is just a type alias for
//! [`Response<HttpBody>`].
//!
//! Here's what you can do with a response:
//!
//! - Check if it worked: [`HttpResponse::ensure_success`] returns an error if the status isn't `2xx`.
//! - Look at status codes: [`HttpResponse::status`] gives you the HTTP status.
//! - Read headers: [`HttpResponse::headers`] lets you access the response headers.
//! - Get the body: [`HttpResponse::into_body`] gives you just the response body.
//! - Process the data: Convert the body to different formats using methods like [`HttpBody::into_text`],
//!   [`HttpBody::into_bytes`], or when the `json` feature is enabled, [`HttpBody::into_json`].
//!
//! ```rust
//! # use fetch::{HttpBody, HttpClient, HttpResponse, StatusExt};
//! # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
//! // Make a GET request
//! let mut response: HttpResponse = client.get("https://www.example.com").fetch().await?;
//!
//! // Check if the response was successful
//! response = response.ensure_success()?;
//!
//! // Check the headers
//! println!("Headers: {}", response.headers().len());
//!
//! // Consume the response and extract the body
//! let body: HttpBody = response.into_body();
//!
//! // Process the body as text
//! let text: String = body.into_text().await?;
//!
//! println!("Response body: {}", text);
//! # Ok(())
//! # }
//! ```
//!
//! ## Specialized Fetch Methods
//!
//! Instead of calling [`HttpRequestBuilder::fetch`] and then converting the response body separately, use these
//! convenient shortcut methods:
//!
//! - [`fetch_text`][crate::HttpRequestBuilder::fetch_text]: Gets the response body as a string in one step.
//! - [`fetch_bytes`][crate::HttpRequestBuilder::fetch_bytes]: Gets the body as a memory-efficient `BytesView`.
//! - [`fetch_json`][crate::HttpRequestBuilder::fetch_json]: Gets the response body as zero-copy JSON (requires `json` feature).
//! - [`fetch_json_owned`][crate::HttpRequestBuilder::fetch_json_owned]: Gets the response body as owned JSON (requires `json` feature).
//!
//! These methods automatically convert the response body to the format you want (string, JSON, etc.),
//! saving you from handling the raw [`HttpBody`] type directly. They return a [`Response<T>`] where `T`
//! is your desired format, so you still get all response details and can check the status and headers
//! before using the body.
//!
//! ```rust
//! # use http::Response;
//! # use fetch::{HttpClient, StatusExt};
//! # use serde_json::json;
//! # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
//! // Retrieve the response as text
//! let response = client
//!     .get("https://api.example.com/users")
//!     .fetch_text()
//!     .await?;
//!
//! // We can examine response metadata before handling the body
//! println!("Status: {}", response.status());
//! println!("Content-Type: {:?}", response.headers().get("content-type"));
//!
//! // Then ensure success and extract the body
//! let text: String = response
//!     .ensure_success()? // Ensure the response was successful
//!     .into_body(); // Discard the response metadata and get the body as a string
//!
//! # Ok(())
//! # }
//! ```
//!
//! # URL Handling
//! The HTTP client uses the [`templated_uri`] crate for
//! URL handling, which provides a powerful and flexible way to work with URIs.
//!
//! You can use the [`Uri`] type to build URIs with templated paths and queries, allowing you to
//! create URLs with dynamic segments and query parameters.
//! The template format follows [RFC 6570](https://datatracker.ietf.org/doc/html/rfc6570) level 3,
//! which means you can use it to easily template more complex paths and queries as well.
//!
//! You can also use the [`Uri`] type or string types to represent URIs for backwards compatibility, or
//! if you don't need templated paths. In that case, the whole `PathAndQuery` string is treated as a template.
//!
//! [`handlers::Logging`] will log the used URL template as
//! `url.path.template`
//!
//! For example, you can create a [`Uri`] with a templated path like this:
//!
//! ```rust
//! # use fetch::HttpClient;
//! use templated_uri::{BaseUri, EscapedString, PathAndQueryTemplate, Uri, templated};
//!
//! #[templated(template = "/users/{user_id}/", unredacted)]
//! #[derive(Clone)]
//! struct UserPath {
//!     user_id: EscapedString, // EscapedString ensures the value is safe for use in URIs
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
//! `templated_uri` supports classification of URL paths and queries using the `data_privacy` crate.
//!
//! You can also use the `classified` attribute to mark [`PathAndQueryTemplate`](templated_uri::PathAndQueryTemplate)
//! structs as classified, allowing you to use classified types from `data_privacy` in your URL templates.
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
//! Working with JSON APIs is straightforward with the `json` feature, which offers:
//!
//! - **Send JSON data**: [`HttpRequestBuilder::json`][crate::HttpRequestBuilder::json] serializes any Rust type to JSON.
//! - **Receive zero-copy JSON**: [`HttpRequestBuilder::fetch_json`][crate::HttpRequestBuilder::fetch_json] returns a
//!   [`Json<T>`][crate::Json] wrapper that borrows strings and byte arrays directly from the response buffer for maximum
//!   performance. Use it when you can work with borrowed data.
//! - **Receive owned JSON**: [`HttpRequestBuilder::fetch_json_owned`][crate::HttpRequestBuilder::fetch_json_owned]
//!   deserializes directly into owned Rust types. Use it when the data must outlive the response or cross thread boundaries.
//! - **Convert bodies to JSON**: [`HttpBody::into_json`][crate::HttpBody::into_json] transforms response bodies into JSON values.
//!
//! ## Zero-Copy JSON with `fetch_json`
//!
//! Pair `fetch_json` with borrowed string fields (`Cow<'a, str>`) to avoid allocations; the
//! [`Json<T>`][crate::Json] wrapper borrows directly from the response buffer. Prefer `Cow<'a, str>` over
//! `&'a str`, as it transparently falls back to an owned value when a JSON string was escaped in the buffer
//! and cannot be borrowed:
//!
//! ```rust
//! # use fetch::{HttpClient, Response, StatusExt};
//! # #[cfg(feature = "json")]
//! # use fetch::Json;
//! # use serde::{Deserialize, Serialize};
//! # use std::borrow::Cow;
//! # #[cfg(feature = "json")]
//! # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
//! // Define a Person type that borrows data to avoid allocations
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
//! // Send and receive zero-copy JSON
//! let response: Response<Json<Person>> = client
//!     .put("https://api.company.com/people")
//!     .json(&person) // Add JSON payload
//!     .fetch_json::<Person>() // Returns Response<Json<Person>>
//!     .await?;
//!
//! // You can inspect the response metadata if needed
//! let response = response.ensure_success()?;
//!
//! // Extract the JSON wrapper from the response
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
//! This minimizes heap allocations and copying because string fields borrow directly from the
//! response buffer instead of allocating new memory for each string.
//!
//! ## Owned JSON with `fetch_json_owned`
//!
//! Use `fetch_json_owned` when you need owned data; it deserializes the JSON directly into your
//! target type with owned `String` fields:
//!
//! ```rust
//! # use fetch::{HttpClient, Response, StatusExt};
//! # use serde::{Deserialize, Serialize};
//! # #[cfg(feature = "json")]
//! # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
//! // Define a Person type with owned data
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
//! // Send and receive owned JSON
//! let response: Response<Person> = client
//!     .put("https://api.company.com/people")
//!     .json(&person) // Add JSON payload
//!     .fetch_json_owned::<Person>() // Returns Response<Person> directly
//!     .await?;
//!
//! // You can inspect the response metadata if needed
//! let response = response.ensure_success()?;
//!
//! // Extract the deserialized Person directly - no wrapper needed
//! let person: Person = response.into_body();
//!
//! println!("Person retrieved, name: {}", person.name);
//!
//! # Ok(())
//! # }
//! ```
//!
//! This performs more allocations than `fetch_json`, but is more convenient when the data must
//! outlive the response, cross thread boundaries, or satisfy APIs that require owned types.
//!
//! # Request Pipeline
//!
//! The HTTP client uses a pipeline architecture to process requests. Think of it like an assembly
//! line - every request passes through a sequence of [`RequestHandler`]s, each handling
//! a specific aspect of HTTP communication.
//!
//! Each handler in the pipeline can:
//!
//! - Modify the request before it's sent
//! - Intercept the request completely (e.g., for caching)
//! - Process the response after it's received
//! - Add cross-cutting functionality like logging or metrics
//!
//! At the very end of the pipeline sits the **transport handler** — the leaf [`RequestHandler`]
//! that actually performs the I/O and turns a request into a response. This is the seam that makes
//! `fetch` transport- and runtime-agnostic: everything above the transport (resilience, telemetry,
//! routing, logging, …) is supplied by `fetch`, while the transport itself can be the bundled
//! hyper-based implementation, your own runtime/I/O, or a wrapper around an existing HTTP client
//! such as [`reqwest`](https://docs.rs/reqwest/). To supply your own transport, see the
//! [`custom`] module and [`custom::create_builder`].
//!
//! ## Built-in Pipeline Types
//!
//! The client offers three types of pipelines to suit different needs - think of them as
//! different levels of "batteries included":
//!
//! ### Standard Pipeline
//!
//! The standard pipeline is what you get by default - it includes all the essential handlers
//! you'll want for production use. Handlers are applied in a nested structure, with the outermost
//! handler processing the request first and the response last.
//!
//! See the [`StandardRequestPipeline`][crate::pipeline::StandardRequestPipeline] and [`HttpClientBuilder::standard_pipeline`]
//! for more details and examples.
//!
//! ### Custom Pipeline
//!
//! When you need precise control over request processing, you can build a custom pipeline with
//! exactly the handlers you want. See the [`HttpClientBuilder::custom_pipeline`] method for
//! more details and examples.
//!
//! ### Minimal Pipeline
//!
//! For maximum flexibility, you can use the minimal pipeline that includes only the
//! essential [`Dispatch`][crate::handlers::Dispatch] handler that actually sends requests to the network.
//! This gives you a clean slate to build on:
//!
//! ```rust
//! # use fetch::HttpClientBuilder;
//! # fn example(mut builder: HttpClientBuilder) -> Result<(), Box<dyn std::error::Error>> {
//! // Create a client with just the dispatch handler
//! let minimal_client = builder.minimal_pipeline().build();
//!
//! // Then wrap it with your own processing logic
//! let wrapped_client = MyHttpWrapper::new(minimal_client);
//! # Ok(())
//! # }
//! # struct MyHttpWrapper;
//! # impl MyHttpWrapper { fn new<T>(_: T) -> Self { Self } }
//! ```
//!
//! This is great when you need to implement your own complete request processing pipeline
//! or integrate with external middleware systems.
//!
//! ## Creating Custom Handlers
//!
//! To add your own processing logic, see the [`RequestHandler`] trait documentation, which covers
//! patterns for modifying requests, processing responses, and integrating with the pipeline.
//!
//! # Testing with the HTTP Client
//!
//! The `fetch` crate makes testing easy with its built-in fake response system. Enable the
//! `test-util` feature to simulate HTTP responses without making real network requests.
//!
//! By using the fake HTTP client in your tests, you can:
//!
//! - Test your code's handling of different HTTP responses
//! - Verify retry behaviors and error handling
//! - Make tests fast and deterministic by avoiding actual network calls
//! - Test edge cases that would be challenging to reproduce with real services
//!
//! The simplest way to create a test client is `HttpClient::new_fake`, which responds with predefined
//! responses instead of making real HTTP requests. It accepts various parameters to streamline testing:
//!
//! ```rust
//! # #[cfg(feature = "test-util")]
//! # fn example() {
//! # use fetch::{HttpClient, HttpResponseBuilder};
//! # use fetch::fake::FakeHandler;
//! # use http::StatusCode;
//!
//! // Create a fake HTTP client that always returns a 200 response
//! let client = HttpClient::new_fake(StatusCode::OK);
//!
//! // Create a fake HTTP client that returns a sequence of responses without a body
//! let client = HttpClient::new_fake(vec![StatusCode::OK, StatusCode::INTERNAL_SERVER_ERROR]);
//!
//! // Create a fake response
//! let response = HttpResponseBuilder::new_fake()
//!     .status(StatusCode::INTERNAL_SERVER_ERROR)
//!     .text("fake text")
//!     .build()
//!     .expect("always succeeds");
//!
//! // Create a fake HTTP client that always returns the same response
//! let client = HttpClient::new_fake(response);
//!
//! // Create a fake HTTP client that uses a custom handler. The handler can be
//! // synchronous or asynchronous. Usually for testing, the synchronous handler is sufficient.
//! let fake_handler = FakeHandler::from_sync_handler(|req| {
//!     println!("Fake handler called for request, url: {}", req.uri());
//!
//!     HttpResponseBuilder::new_fake()
//!         .status(StatusCode::INTERNAL_SERVER_ERROR)
//!         .text("fake text")
//!         .build()
//! });
//!
//! // Create a fake HTTP client that uses the custom handler
//! let client = HttpClient::new_fake(fake_handler);
//! # }
//! ```
//!
//! # Smart Memory Management with `BytesView`
//!
//! When handling large HTTP responses or sending big requests, memory usage matters, so `fetch` uses
//! [`BytesView`] for request and response bodies. Its key features:
//!
//! - **Memory Pooling**: Reuses memory instead of constantly allocating and freeing it.
//! - **Less Copying**: Smart buffer management reduces unnecessary data copying.
//! - **Multiple Chunks**: Can handle data as multiple pieces that look like a single buffer.
//! - **Zero-Copy When Possible**: Avoids copying data when it can for better performance.
//! - **Works with Ecosystem**: Fully compatible with the popular [`bytes`] crate.
//!
//! You can use [`BytesView`] just like other byte buffer types because it implements the same
//! interfaces ([`Buf`] and [`BufMut`]) as the [`bytes`] crate:
//!
//! ```rust
//! # use fetch::HttpClient;
//! # use bytesbuf::{BytesView};
//! # use bytes::Buf;
//! #
//! # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
//! // Get a response body as a BytesView
//! let response = client.get("https://example.com").fetch().await?;
//! let mut body_bytes = response.into_body().into_bytes().await?;
//!
//! // Work with the BytesView using standard bytes methods
//! let length = body_bytes.remaining();
//!
//! // Easy to extract data when you need it
//! let mut buffer = vec![0; 10.min(length)];
//! if !body_bytes.is_empty() {
//!     body_bytes.copy_to_slice(&mut buffer);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! This lets your app handle large files, streaming media, or other big data without
//! wasting memory or hurting performance.
//!
//! [`BytesView`]: bytesbuf::BytesView
//! [`BytesBuf`]: bytesbuf::BytesBuf
//! [`Buf`]: bytes::Buf
//! [`BufMut`]: bytes::BufMut
//! [`bytes`]: https://docs.rs/bytes
//!
//! # Performance Best Practices
//!
//! Follow these tips for the best performance:
//!
//! ```rust,no_run
//! # use fetch::HttpClient;
//! # use http::Uri;
//! # #[cfg(all(feature = "tokio", any(feature = "rustls", feature = "native-tls")))]
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // 1. Create a client ONCE and reuse it
//! let client = HttpClient::new_tokio();
//!
//! // 2. Parse URIs ahead of time for repeated use
//! let users_uri: Uri = "https://api.example.com/users".parse()?;
//! let items_uri: Uri = "https://api.example.com/items".parse()?;
//!
//! // 3. Work with raw BytesView to avoid allocations when possible
//! let response = client.get(users_uri.clone()).fetch().await?;
//! let bytes = response.into_body().into_bytes().await?;
//! process_binary_data(bytes);
//! # Ok(())
//! # }
//! # fn process_binary_data(bytes: bytesbuf::BytesView) {}
//! ```
//!
//! In detail:
//!
//! - **Reuse your client**: Creating an [`HttpClient`] is expensive (connection pooling, security setup).
//!   Create it once and keep using it throughout your application. Share a single instance across
//!   multiple tasks.
//! - **Pre-parse URIs**: If you're repeatedly calling the same endpoints, parse the [`Uri`]s once and
//!   reuse them to skip the parsing overhead.
//! - **Work with raw [`BytesView`]**: Converting between formats (like [`BytesView`] to `String`) creates
//!   allocations and copies data. When handling binary data or large responses, work with [`BytesView`] directly.
//!
//! # Integration with the HTTP Ecosystem
//!
//! Instead of creating our own HTTP types from scratch, we use extensions and wrappers around
//! the widely adopted [`http`] crate. These extensions are defined in the [`http_extensions`] crate
//! and re-exported here for convenience.
//!
//! # Resilience
//!
//! The HTTP client has built-in resilience features powered by the [`seatbelt`] crate. These
//! resilience patterns help your application handle failures gracefully and maintain availability
//! even when network issues or server problems occur.
//!
//! The resilience middleware integrates directly into the request pipeline via the
//! [`Service`][`layered::Service`] trait. Because both the client's handlers and the seatbelt
//! middleware implement this trait, they compose seamlessly - no adapter code is needed to mix
//! resilience patterns with other request processing logic.
//!
//! Common resilience patterns available include:
//!
//! - **Retries**: Automatically retry failed requests with configurable backoff strategies
//! - **Timeouts**: Prevent requests from hanging indefinitely
//! - **Circuit Breakers**: Fail fast when a service is down to avoid cascading failures
//!
//! These patterns are already configured in the [standard pipeline][crate::pipeline::StandardRequestPipeline] with sensible defaults.
//!
//! # TLS Support
//!
//! The HTTP client supports two TLS backends for making HTTPS requests:
//!
//! - **`rustls`** (default): Uses [`rustls`](https://docs.rs/rustls) with the
//!   [`aws-lc-rs`](https://docs.rs/aws-lc-rs) crypto provider. This is the recommended
//!   backend and is selected by default when the `tls` feature is enabled.
//! - **`native-tls`**: Uses the platform's native TLS implementation (`SChannel` on Windows,
//!   Security Framework on `macOS`, `OpenSSL` on Linux). This can be explicitly selected, or is
//!   chosen automatically when the `native-tls` feature is enabled and `rustls` is not.
//!
//! When using the `rustls` backend, the HTTP client validates server certificates against the
//! platform trust store via [`rustls-platform-verifier`](https://docs.rs/rustls-platform-verifier),
//! which takes care of essential TLS operations:
//!
//! - Verifies certificates against the operating system's trusted root `CAs`.
//! - Validates hostnames and checks certificate expiration.
//! - Enforces TLS security policies.
//!
//! TLS is configured automatically; simply construct a client and make HTTPS requests:
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
//!     // Now you can make HTTPS requests
//!     let response = client.get("https://www.example.com").fetch_text().await?;
//!
//!     Ok(())
//! }
//! # }
//! ```
//!
//! To enable TLS support, add the `tls` feature (which enables `rustls` by default) to your dependencies:
//!
//! ```toml
//! fetch = { version = "*", features = ["tls", "tokio"] }
//! ```
//!
//! To use native TLS instead, enable the `native-tls` feature explicitly:
//!
//! ```toml
//! fetch = { version = "*", features = ["native-tls", "tokio"] }
//! ```
//!
//! You can also select the TLS backend at runtime via [`TlsOptions::builder_rustls()`](tls::TlsOptions::builder_rustls) or
//! `TlsOptions::builder_native_tls()` when both features are enabled, allowing different client
//! instances to use different backends.
//!
//! # Features
//!
//! The `fetch` crate provides several optional features that you can enable in your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! fetch = { version = "*", features = ["json", "tokio"] }
//! ```
//!
//! - **`tokio`**: Enables integration with the Tokio runtime. This feature provides the `HttpClient::builder_tokio`
//!   constructor and related APIs for using the HTTP client in a Tokio-based application.
//!
//! - **`json`**: Adds support for JSON serialization and deserialization, enabling methods like
//!   `HttpRequestBuilder::json` for sending JSON data and `HttpRequestBuilder::fetch_json` for receiving JSON responses.
//!
//! - **`tls`**: Enables TLS support using `rustls` with the `aws-lc-rs` crypto provider. This is the
//!   recommended way to enable HTTPS support and is an alias for the `rustls` feature.
//!
//! - **`rustls`**: Enables the `rustls` TLS backend with `aws-lc-rs`. This is the default TLS backend
//!   and is selected automatically by the `tls` feature.
//!
//! - **`native-tls`**: Enables the platform native TLS backend (`SChannel` on Windows, Security Framework
//!   on `macOS`, `OpenSSL` on Linux). Use this when you need the platform's native TLS stack. When both
//!   `rustls` and `native-tls` are enabled, `rustls` is the default but you can select native TLS via
//!   `TlsOptions::builder_native_tls()`.
//!
//! - **`test-util`**: Provides APIs to fake responses and HTTP client behavior for testing purposes.
//!   This feature makes it easy to write fast, deterministic tests without making real network requests.
//!
//! > **Note**: Most users should enable the `tokio` feature along with the `tls` feature for HTTPS
//! > support. The `json` feature is recommended for most applications that need to work with JSON APIs.
#[doc(inline)]
pub use ::http::{Extensions, HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode, Version};
#[doc(inline)]
pub use http_extensions::routing;
#[doc(inline)]
pub use seatbelt::{Recovery, RecoveryInfo};
#[doc(inline)]
pub use templated_uri::{BasePath, BaseUri, Origin, Uri};

/// Re-exports of the [`http`](https://docs.rs/http) crate's submodules.
///
/// These are grouped here to keep the `fetch` crate root uncluttered. The most
/// commonly used `http` types (such as [`HeaderMap`], [`Method`], [`StatusCode`],
/// and [`Version`]) are re-exported directly at the crate root.
pub mod http {
    #[doc(inline)]
    pub use ::http::{header, method, request, response, status, version};
}

pub(crate) mod constants;

mod error_labels;

pub mod tls;

mod client_builder;
pub use client_builder::HttpClientBuilder;

pub mod options;

mod client;
pub use client::HttpClient;

#[cfg(any(feature = "test-util", test))]
pub mod fake;

pub mod custom;

#[cfg(all(feature = "tokio", any(feature = "rustls", feature = "native-tls")))]
pub mod tokio;

pub mod handlers;

pub mod telemetry;

#[doc(inline)]
pub use http_extensions::{
    HeaderMapExt, HeaderValueExt, HttpBody, HttpBodyBuilder, HttpError, HttpRequest, HttpRequestBuilder, HttpRequestExt, HttpResponse,
    HttpResponseBuilder, RequestExt, RequestHandler, ResponseExt, Result, StatusExt,
};
#[cfg(any(feature = "json", test))]
pub use http_extensions::{Json, JsonError};

pub mod resilience;

pub mod pipeline;

/// Longer form documentation for [`fetch`](crate).
pub mod _documentation;
