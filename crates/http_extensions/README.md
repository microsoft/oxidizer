<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Http Extensions Logo" width="96">

# Http Extensions

[![crate.io](https://img.shields.io/crates/v/http_extensions.svg)](https://crates.io/crates/http_extensions)
[![docs.rs](https://docs.rs/http_extensions/badge.svg)](https://docs.rs/http_extensions)
[![MSRV](https://img.shields.io/crates/msrv/http_extensions)](https://crates.io/crates/http_extensions)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Shared HTTP types and extension traits for clients and servers.

This crate provides common HTTP features that build on the [`http`][__link0] crate.
It includes body handling, error types, and extension traits for HTTP requests
and responses.

## Core Types

* [`HttpRequest`][__link1] and [`HttpResponse`][__link2] are aliases for requests and responses with [`HttpBody`][__link3].
* [`HttpRequestBuilder`][__link4] constructs HTTP requests with chained method calls.
* [`HttpResponseBuilder`][__link5] constructs HTTP responses with chained method calls.
* [`HttpBody`][__link6] supports text, binary, JSON, and streaming content.
* [`HttpBodyBuilder`][__link7] creates HTTP bodies with a configurable memory provider.
* [`HttpError`][__link8] provides backtraces and recovery information.
* [`RequestHandler`][__link9] defines HTTP middleware and request pipelines.

## Extension Traits

The crate provides extension traits for standard HTTP types:

* [`StatusExt`][__link10] classifies status codes and their recovery behavior.
* [`RequestExt`][__link11] extends HTTP requests.
* [`ResponseExt`][__link12] classifies response recovery behavior and supports `Retry-After`.
* [`HttpRequestExt`][__link13] clones requests and their bodies.
* [`HeaderMapExt`][__link14] extracts and parses header values.
* [`HeaderValueExt`][__link15] constructs [`HeaderValue`][__link16] from [`Bytes`][__link17].
* [`ExtensionsExt`][__link18] extracts URI template labels from [`Extensions`][__link19].

## Quick Start

The following example creates an HTTP client, builds a request, and requires
a successful response:

```rust
// Create a builder for request and response bodies.
let body_builder = HttpBodyBuilder::new_fake();

// Create a fake handler that returns a successful response.
// The `test-util` feature provides this test handler.
let handler = FakeHandler::from(
    HttpResponseBuilder::new(&body_builder)
        .status(200)
        .header("Content-Type", "application/json")
        .text(r#"{"message": "Success"}"#)
        .build()?,
);

// Build and send an HTTP request through the handler.
let response = handler
    .request_builder()
    .get("https://api.example.com/data")
    .header("Authorization", "Bearer token")
    .fetch()
    .await?;

// Return an error if the response has a `4xx` or `5xx` status code.
let validated_response = response.ensure_success()?;

println!("response status: {}", validated_response.status());
```

This example uses the `test-util` feature to create a [`FakeHandler`][__link20].
Production code uses an HTTP client that implements [`RequestHandler`][__link21].
In both cases, [`HttpRequestBuilder`][__link22] builds the request and the handler sends it.
[`StatusExt::ensure_success`][__link23] then requires a successful response.

## Integration with the HTTP Ecosystem

This crate builds on the [`http`][__link24] crate:

* It uses [`http::Request`][__link25] and [`http::Response`][__link26] as base types.
* It reuses [`http::Method`][__link27], [`http::StatusCode`][__link28], and [`http::HeaderMap`][__link29].
* It implements standard traits such as [`http_body::Body`][__link30].
* It works with other Rust HTTP libraries.

## Examples

### Requiring a Successful Response

```rust
// Return an error if the response was not successful.
let validated_response = response.ensure_success()?;
```

### Creating Request Bodies

```rust
// Create different body types.
let text_body = builder.text("Hello, world!");
let binary_body = builder.slice(&[1, 2, 3, 4]);
let empty_body = builder.empty();
```

### Building HTTP Requests

```rust
let request = HttpRequestBuilder::new(&body_builder)
    .get("https://api.example.com/data")
    .text("Hello World")
    .build()
    .unwrap();
```

### Building HTTP Responses

```rust
let response = HttpResponseBuilder::new(&body_builder)
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

The `test-util` feature provides [`FakeHandler`][__link31].
It returns test responses without network requests.
Use it to test code that depends on HTTP clients.

## Features

* `json` enables JSON serialization and deserialization through [`Json`][__link32].
* `test-util` enables test implementations.

## Memory Management

Bodies created through [`HttpBodyBuilder`][__link33] use its configured [`bytesbuf`][__link34] memory provider.
A pooled provider can reuse memory after the body data is consumed.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/http_extensions">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbJJho48er8n8bB9RTi-mwTRUbnPdnuKE6hF4b_jsesCPD4pZhZIWCZWJ5dGVzZjEuMTIuMYJoYnl0ZXNidWZmMC4xMi4wgmRodHRwZTEuNS4wg2lodHRwLWJvZHllMS4xLjBpaHR0cF9ib2R5gm9odHRwX2V4dGVuc2lvbnNmMC4xMy4w
 [__link0]: https://crates.io/crates/http/1.5.0
 [__link1]: https://docs.rs/http_extensions/0.13.0/http_extensions/type.HttpRequest.html
 [__link10]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=StatusExt
 [__link11]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=RequestExt
 [__link12]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=ResponseExt
 [__link13]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpRequestExt
 [__link14]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HeaderMapExt
 [__link15]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HeaderValueExt
 [__link16]: https://docs.rs/http/1.5.0/http/?search=HeaderValue
 [__link17]: https://docs.rs/bytes/1.12.1/bytes/?search=Bytes
 [__link18]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=ExtensionsExt
 [__link19]: https://docs.rs/http/1.5.0/http/?search=Extensions
 [__link2]: https://docs.rs/http_extensions/0.13.0/http_extensions/type.HttpResponse.html
 [__link20]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=FakeHandler
 [__link21]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=RequestHandler
 [__link22]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpRequestBuilder
 [__link23]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=StatusExt::ensure_success
 [__link24]: https://crates.io/crates/http/1.5.0
 [__link25]: https://docs.rs/http/1.5.0/http/?search=Request
 [__link26]: https://docs.rs/http/1.5.0/http/?search=Response
 [__link27]: https://docs.rs/http/1.5.0/http/?search=Method
 [__link28]: https://docs.rs/http/1.5.0/http/?search=StatusCode
 [__link29]: https://docs.rs/http/1.5.0/http/?search=HeaderMap
 [__link3]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpBody
 [__link30]: https://docs.rs/http-body/1.1.0/http_body/?search=Body
 [__link31]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=FakeHandler
 [__link32]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=Json
 [__link33]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpBodyBuilder
 [__link34]: https://crates.io/crates/bytesbuf/0.12.0
 [__link4]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpRequestBuilder
 [__link5]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpResponseBuilder
 [__link6]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpBody
 [__link7]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpBodyBuilder
 [__link8]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpError
 [__link9]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=RequestHandler
