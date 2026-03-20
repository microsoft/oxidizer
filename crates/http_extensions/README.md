<div align="center">
 <img src="./logo.png" alt="Http Extensions Logo" width="96">

# Http Extensions

[![crate.io](https://img.shields.io/crates/v/http_extensions.svg)](https://crates.io/crates/http_extensions)
[![docs.rs](https://docs.rs/http_extensions/badge.svg)](https://docs.rs/http_extensions)
[![MSRV](https://img.shields.io/crates/msrv/http_extensions)](https://crates.io/crates/http_extensions)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Shared HTTP types and extension traits for clients and servers.

This crate provides common HTTP functionality built on the popular [`http`][__link0] crate,
including flexible body handling, unified error types, and ergonomic extension traits
for working with HTTP requests and responses.

## Core Types

* [`HttpRequest`][__link1] and [`HttpResponse`][__link2] - Type aliases for requests and responses with [`HttpBody`][__link3]
* [`HttpRequestBuilder`][__link4] - Builder for constructing HTTP requests with a fluent API
* [`HttpResponseBuilder`][__link5] - Builder for constructing HTTP responses with a fluent API
* [`HttpBody`][__link6] - Flexible body type supporting text, binary, JSON, and streaming content
* [`HttpBodyBuilder`][__link7] - Builder for creating HTTP bodies with memory pool optimization
* [`HttpError`][__link8] - Unified error type with automatic backtraces and recovery classification
* [`RequestHandler`][__link9] - Trait for HTTP middleware and request processing pipelines

## Extension Traits

The crate provides extension traits that add convenience methods to standard HTTP types:

* [`StatusExt`][__link10] - Status code validation and recovery classification
* [`RequestExt`][__link11] - Extensions for HTTP requests
* [`ResponseExt`][__link12] - Response recovery classification with `Retry-After` support
* [`HttpRequestExt`][__link13] - Request cloning with body support
* [`HeaderMapExt`][__link14] - Header value extraction and parsing
* [`HeaderValueExt`][__link15] - Construction of [`HeaderValue`][__link16] from [`Bytes`][__link17]

## Quick Start

Here’s a complete example showing how to create an HTTP client, build a request,
and validate the response:

```rust
// Create a body builder for constructing request/response bodies
let body_builder = HttpBodyBuilder::new_fake();

// Create a fake handler that returns a successful response
// (This uses the `test-util` feature for testing; similar workflow applies to real clients)
let handler = FakeHandler::from(
    HttpResponseBuilder::new(&body_builder)
        .status(200)
        .header("Content-Type", "application/json")
        .text(r#"{"message": "Success"}"#)
        .build()?,
);

// Build and send an HTTP request using the handler
let response = handler
    .request_builder()
    .get("https://api.example.com/data")
    .header("Authorization", "Bearer token")
    .fetch()
    .await?;

// Validate that the response succeeded (returns error for `4xx/5xx` status codes)
let validated_response = response.ensure_success()?;

println!("response status: {}", validated_response.status());
```

**Note**: This example uses the `test-util` feature to create a `FakeHandler` for testing.
In production code, you would use a real HTTP client that implements the
[`RequestHandler`][__link18] trait, but the workflow remains the same: build requests with
[`HttpRequestBuilder`][__link19], send them through a handler, and validate responses with
[`StatusExt::ensure_success`][__link20].

## Integration with the HTTP Ecosystem

This crate builds on the popular [`http`][__link21] crate rather than inventing new types:

* Uses [`http::Request`][__link22] and [`http::Response`][__link23] as base types
* Reuses [`http::Method`][__link24], [`http::StatusCode`][__link25], and [`http::HeaderMap`][__link26]
* Implements standard traits like [`http_body::Body`][__link27] for ecosystem compatibility
* Works seamlessly with other Rust HTTP libraries

## Examples

### Validating Response Status

```rust
// Check if the response succeeded and return an error if not
let validated_response = response.ensure_success()?;
```

### Creating Request Bodies

```rust
// Create different body types
let text_body = builder.text("Hello, world!");
let binary_body = builder.slice(&[1, 2, 3, 4]);
let empty_body = builder.empty();
```

### Building HTTP Requests

```rust
let request = HttpRequestBuilder::new(body_builder)
    .get("https://api.example.com/data")
    .text("Hello World")
    .build()
    .unwrap();
```

### Building HTTP Responses

```rust
let response = HttpResponseBuilder::new(body_builder)
    .status(200)
    .header("Content-Type", "text/plain")
    .body(body_builder.text("Success"))
    .build()
    .unwrap();
```

### Building Middleware with `RequestHandler`

```rust
struct LoggingMiddleware<S> {
    inner: S,
}

impl<S: RequestHandler> Service<HttpRequest> for LoggingMiddleware<S> {
    type Out = Result<HttpResponse>;

    async fn execute(&self, request: HttpRequest) -> Self::Out {
        println!("Processing request to: {}", request.uri());
        let response = self.inner.execute(request).await?;
        println!("Response status: {}", response.status());
        Ok(response)
    }
}
```

### Testing with `FakeHandler`

The `FakeHandler` type (available with the `test-util` feature) lets you mock HTTP responses
for testing without making actual network requests. This is useful for unit testing code
that depends on HTTP clients.

## Features

* `json` - Enables JSON serialization/deserialization support via `Json` type
* `test-util` - Enables fake implementations for testing

## Memory Management

Bodies created through [`HttpBodyBuilder`][__link28] use memory pools from [`bytesbuf`][__link29] to
reduce allocation overhead. When body data is consumed, memory is automatically recycled
for future requests. This makes the crate particularly efficient for high-throughput scenarios.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/http_extensions">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEGyKlgL27uQutG5NWANIkKYuiGyYWve9aLfqWG7wgKrc4ZtdfYWSFgmVieXRlc2YxLjExLjGCaGJ5dGVzYnVmZTAuNC4wgmRodHRwZTEuNC4wgmlodHRwX2JvZHllMS4wLjGCb2h0dHBfZXh0ZW5zaW9uc2UwLjIuMA
 [__link0]: https://crates.io/crates/http/1.4.0
 [__link1]: https://docs.rs/http_extensions/0.2.0/http_extensions/type.HttpRequest.html
 [__link10]: https://docs.rs/http_extensions/0.2.0/http_extensions/?search=StatusExt
 [__link11]: https://docs.rs/http_extensions/0.2.0/http_extensions/?search=RequestExt
 [__link12]: https://docs.rs/http_extensions/0.2.0/http_extensions/?search=ResponseExt
 [__link13]: https://docs.rs/http_extensions/0.2.0/http_extensions/?search=HttpRequestExt
 [__link14]: https://docs.rs/http_extensions/0.2.0/http_extensions/?search=HeaderMapExt
 [__link15]: https://docs.rs/http_extensions/0.2.0/http_extensions/?search=HeaderValueExt
 [__link16]: https://docs.rs/http/1.4.0/http/?search=HeaderValue
 [__link17]: https://docs.rs/bytes/1.11.1/bytes/?search=Bytes
 [__link18]: https://docs.rs/http_extensions/0.2.0/http_extensions/?search=RequestHandler
 [__link19]: https://docs.rs/http_extensions/0.2.0/http_extensions/?search=HttpRequestBuilder
 [__link2]: https://docs.rs/http_extensions/0.2.0/http_extensions/type.HttpResponse.html
 [__link20]: https://docs.rs/http_extensions/0.2.0/http_extensions/?search=StatusExt::ensure_success
 [__link21]: https://crates.io/crates/http/1.4.0
 [__link22]: https://docs.rs/http/1.4.0/http/?search=Request
 [__link23]: https://docs.rs/http/1.4.0/http/?search=Response
 [__link24]: https://docs.rs/http/1.4.0/http/?search=Method
 [__link25]: https://docs.rs/http/1.4.0/http/?search=StatusCode
 [__link26]: https://docs.rs/http/1.4.0/http/?search=HeaderMap
 [__link27]: https://docs.rs/http_body/1.0.1/http_body/?search=Body
 [__link28]: https://docs.rs/http_extensions/0.2.0/http_extensions/?search=HttpBodyBuilder
 [__link29]: https://crates.io/crates/bytesbuf/0.4.0
 [__link3]: https://docs.rs/http_extensions/0.2.0/http_extensions/?search=HttpBody
 [__link4]: https://docs.rs/http_extensions/0.2.0/http_extensions/?search=HttpRequestBuilder
 [__link5]: https://docs.rs/http_extensions/0.2.0/http_extensions/?search=HttpResponseBuilder
 [__link6]: https://docs.rs/http_extensions/0.2.0/http_extensions/?search=HttpBody
 [__link7]: https://docs.rs/http_extensions/0.2.0/http_extensions/?search=HttpBodyBuilder
 [__link8]: https://docs.rs/http_extensions/0.2.0/http_extensions/?search=HttpError
 [__link9]: https://docs.rs/http_extensions/0.2.0/http_extensions/?search=RequestHandler
