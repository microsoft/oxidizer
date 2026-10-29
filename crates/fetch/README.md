<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Fetch Logo" width="96">

# Fetch

[![crate.io](https://img.shields.io/crates/v/fetch.svg)](https://crates.io/crates/fetch)
[![docs.rs](https://docs.rs/fetch/badge.svg)](https://docs.rs/fetch)
[![MSRV](https://img.shields.io/crates/msrv/fetch)](https://crates.io/crates/fetch)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

A fast, safe HTTP client that works across runtimes and transports.

```rust
let response = client.get("https://example.com").fetch().await?;
assert!(response.status().is_success());
```

[`HttpClient`][__link0] supports different async runtimes and transports. It includes secure defaults,
resilience, telemetry, and test utilities.

## Why a new HTTP client?

`fetch` includes the features that services commonly need:

* Secure TLS validation, retries, circuit breaking, hedging, and OpenTelemetry telemetry.
* Fake responses through the `test-util` feature.
* A request pipeline that supports custom logging, metrics, retries, and other handlers.
* Memory pooling and zero-copy operations for large responses.

`fetch` does not require one runtime, I/O implementation, or HTTP transport. A transport handler
performs I/O at the end of the request pipeline. Resilience, telemetry, routing, logging, and
retries run above that handler.

* Tokio works with the bundled transport. A custom transport can use another runtime and I/O
  implementation.
* A transport implements [`RequestHandler`][__link1]. It can use the bundled hyper transport, a custom
  client, or another client such as [`reqwest`][__link2].

A library can use `fetch` while the application selects the runtime and transport. See the
[`custom`][__link3] module and [`custom::create_builder`][__link4] for an example.

### How does it compare to `reqwest`?

Both `fetch` and [`reqwest`][__link5] use
[`hyper`][__link6] by default. `fetch` also includes resilience and supports
custom runtimes and transports. A custom transport can use `reqwest` while the rest of the
`fetch` pipeline stays unchanged.

|Feature|`fetch`|`reqwest`|
|-------|-------|---------|
|Runtime support|Tokio and custom runtimes|Tokio on native targets; browser implementation on `wasm32`|
|Custom transport or I/O|Built in|Not supported|
|TLS/HTTPS|rustls or native-tls|rustls or native-tls|
|Resilience|Retries and circuit breaking are enabled by default; hedging is built in and optional|Built-in retries; circuit breaking and hedging require external middleware|
|JSON support|Built in|Built in|
|Testing tools|Built in|Requires custom or external tools|
|OpenTelemetry metrics|Built in|Requires custom integration|
|Structured request logging|Built-in tracing events|Requires custom middleware|
|Advanced HTTP client features [^1]|Not supported yet [^2]|Available through optional features|
|Request pipeline|Built in|Requires custom or external tools|
|Zero-copy buffers|Built in|Partial support through `Bytes`|
|Linux support|Full support|Full support|

[^1]: Advanced HTTP client features include file uploads, cookies, proxies, and redirects.

[^2]: Future versions can add the missing cookies, redirects, and forms features.

 > 
 > **Note**: `fetch` uses methods such as `get()`, `post()`, and `fetch()`, which are similar to
 > `reqwest` methods.

## Getting Started

The bundled transport runs on Tokio. Use a custom transport for another runtime. See the
[`custom`][__link7] module.

```rust
use fetch::{HttpClient, HttpError};

#[tokio::main]
async fn main() -> Result<(), HttpError> {
    // Create a client with the bundled Tokio transport.
    let client: HttpClient = HttpClient::new_tokio();

    // Return an error for a `4xx` or `5xx` status, then read the body as text.
    let body: String = client.get("https://example.com").fetch_text_body().await?;

    println!("response: {body}");

    Ok(())
}
```

 > 
 > **Note**: Use [`HttpClient::builder_tokio`][__link8] to add handlers or change timeouts and other
 > configuration.

## Making Requests

Use methods such as [`HttpClient::get`][__link9] and [`HttpClient::post`][__link10] for common requests. Each method
returns a builder for request-specific configuration.

### GET Requests

```rust
// Send a GET request.
let response: HttpResponse = client.get("https://www.example.com").fetch().await?;

```

### POST Requests

```rust
// Send a POST request with a text body.
let response = client
    .post("https://httpbin.org/post")
    .text("the exact body that is sent")
    .fetch()
    .await?;

```

### Other request methods

The client has methods such as [`HttpClient::put`][__link11] and [`HttpClient::delete`][__link12]. Use
[`HttpClient::request`][__link13] for another HTTP method:

```rust
// Send a PATCH request.
let response = client
    .request(Method::PATCH, "https://api.example.com/items/42")
    .fetch()
    .await?;
```

A request can include headers, an HTTP version, and a body:

```rust
let response = client
    .post("https://api.example.com/upload")
    // Add HTTP headers.
    .header(header::AUTHORIZATION, "Bearer token123")
    .header(header::CONTENT_TYPE, "application/json")
    // Select the HTTP version.
    .version(Version::HTTP_2)
    .text("{\"name\": \"document.pdf\"}")
    .fetch()
    .await?;
```

These methods return [`HttpRequestBuilder`][__link14], which configures and sends the request.

### Reuse a base URI

Use [`HttpClientBuilder::base_uri`][__link15] to set one [`BaseUri`][__link16] for all requests.

The client base URI overrides a base URI in a request.

```rust
let client = builder
    .base_uri(BaseUri::from_static("https://example.com/api/v1/")) // The trailing slash is required.
    .build();

// This request uses `https://example.com/api/v1/foo/bar`.
let response = client.get("/foo/bar").fetch().await?;
```

## Handling Responses

[`HttpRequestBuilder::fetch`][__link17] returns an [`HttpResponse`][__link18] with the body, status code, headers,
and extensions. `HttpResponse` is a type alias for [`Response<HttpBody>`][__link19].

A response supports these operations:

* [`HttpResponse::ensure_success`][__link20] returns an error if the status is not `2xx`.
* [`HttpResponse::status`][__link21] returns the HTTP status.
* [`HttpResponse::headers`][__link22] returns the response headers.
* [`HttpResponse::into_body`][__link23] returns the response body.
* [`HttpBody::into_text`][__link24] and [`HttpBody::into_bytes`][__link25] convert the body.
* [`HttpBody::into_json`][__link26] converts the body when the `json` feature is enabled.

```rust
// Send a GET request.
let mut response: HttpResponse = client.get("https://www.example.com").fetch().await?;

// Return an error if the status is not successful.
response = response.ensure_success()?;

// Read the headers.
println!("Headers: {}", response.headers().len());

// Consume the response and return its body.
let body: HttpBody = response.into_body();

// Read the body as text.
let text: String = body.into_text().await?;

println!("Response body: {}", text);
```

### Specialized Fetch Methods

These methods fetch a response and convert its body:

* [`fetch_text`][__link27] returns a string.
* [`fetch_bytes`][__link28] returns a `BytesView`.
* [`fetch_json`][__link29] returns owned JSON and requires `json`.
* [`fetch_json_ref`][__link30] returns zero-copy JSON and
  requires `json`.

Each method returns [`Response<T>`][__link31]. The response retains its status, headers, and extensions.

```rust
// Return the response with a text body.
let response = client
    .get("https://api.example.com/users")
    .fetch_text()
    .await?;

// Read response metadata before the body.
println!("Status: {}", response.status());
println!("Content-Type: {:?}", response.headers().get("content-type"));

// Return an error for an unsuccessful status, then extract the body.
let text: String = response.ensure_success()?.into_body();

```

### Body-Only Shortcuts

The `_body` variants call [`ensure_success`][__link32], discard the
response metadata, and return the converted body.

* [`fetch_text_body`][__link33] returns a `String`.
* [`fetch_bytes_body`][__link34] returns a `BytesView`.
* [`fetch_json_body`][__link35] returns an owned value and
  requires `json`.

```rust
// Fetch the response, require a successful status, and return the text body.
let text: String = client
    .get("https://api.example.com/users")
    .fetch_text_body()
    .await?;
println!("body: {text}");
```

## URL Handling

The HTTP client uses [`templated_uri`][__link36] for URLs. [`Uri`][__link37] supports dynamic path segments and
query parameters through templates.

Templates follow [RFC 6570][__link38] level 3. [`Uri`][__link39] and
string types also support URLs without typed templates. In that case, the full `PathAndQuery`
string becomes the template.

[`handlers::Logging`][__link40] records the URL template as `url.path.template`.

This example creates a [`Uri`][__link41] with a templated path:

```rust
use templated_uri::{BaseUri, EscapedString, PathAndQueryTemplate, Uri, templated};

#[templated(template = "/users/{user_id}/", unredacted)]
#[derive(Clone)]
struct UserPath {
    user_id: EscapedString, // EscapedString escapes the value for use in URIs.
}

let user_path = UserPath {
    user_id: EscapedString::from_static("12345"),
};

client
    .get(
        Uri::default()
            .with_base(BaseUri::from_static("https://api.example.com"))
            .with_path_and_query(user_path),
    )
    .fetch_text()
    .await?;

```

### Classification in URLs

`templated_uri` uses `data_privacy` to classify URL paths and queries. The `classified` attribute
lets a [`PathAndQueryTemplate`][__link42] use classified
`data_privacy` types.

```rust
use data_privacy::Sensitive;
use templated_uri::{EscapedString, PathAndQueryTemplate, templated};

#[templated(template = "/{org_id}/user/{user_id}")]
struct UserPath {
    #[unredacted]
    org_id: u32,
    user_id: Sensitive<EscapedString>,
}
```

## JSON Support

The `json` feature supports these operations:

* [`HttpRequestBuilder::json`][__link43] serializes a Rust type as JSON.
* [`HttpRequestBuilder::fetch_json`][__link44] deserializes into an
  owned Rust type. The value can outlive the response. It can cross thread boundaries if its
  type implements `Send`.
* [`HttpRequestBuilder::fetch_json_ref`][__link45] returns
  [`Json<T>`][__link46]. It can borrow strings and byte arrays from the response buffer.
* [`HttpBody::into_json`][__link47] converts a response body into JSON.

### Owned JSON with `fetch_json`

Use `fetch_json` for owned data. It deserializes JSON directly into the target type:

```rust
// Define a type with owned data.
#[derive(Serialize, Deserialize)]
struct Person {
    id: u32,
    name: String,
}

let person = Person {
    id: 1,
    name: "Alice Johnson".to_owned(),
};

// Send and receive owned JSON.
let response: Response<Person> = client
    .put("https://api.company.com/people")
    .json(&person)
    .fetch_json::<Person>()
    .await?;

// Read the response metadata.
let response = response.ensure_success()?;

// Extract the deserialized value.
let person: Person = response.into_body();

println!("Person retrieved, name: {}", person.name);

```

[`fetch_json_body`][__link48] fetches the response, requires a
successful status, and returns the owned value.

### Zero-Copy JSON with `fetch_json_ref`

Pair `fetch_json_ref` with borrowed string fields such as `Cow<'a, str>`. [`Json<T>`][__link49]
borrows from the response buffer. Prefer `Cow<'a, str>` over `&'a str`. `Cow` uses an owned value
when an escaped JSON string cannot be borrowed.

```rust
// Define a type that can borrow string data.
#[derive(Serialize, Deserialize)]
struct Person<'a> {
    id: u32,
    #[serde(borrow)]
    name: Cow<'a, str>,
}

let person = Person {
    id: 1,
    name: "Alice Johnson".into(),
};

// Send and receive zero-copy JSON.
let response: Response<Json<Person>> = client
    .put("https://api.company.com/people")
    .json(&person)
    .fetch_json_ref::<Person>()
    .await?;

// Read the response metadata.
let response = response.ensure_success()?;

// Extract the JSON wrapper.
let mut json_body: Json<Person> = response.into_body();

// Parse the JSON data using zero-copy deserialization.
// The parsed Person borrows string data from the underlying buffer.
let person: Person = json_body.read()?;

println!("Person retrieved, name: {}", person.name);

```

Borrowed fields reduce heap allocations and copies. The parsed value cannot outlive the response
buffer.

## Request Pipeline

Each request passes through a sequence of [`RequestHandler`][__link50] implementations.

A handler can:

* Modify the request before the next handler receives it.
* Return a response without calling the next handler, for example to serve a cached response.
* Process the response from the next handler.
* Add logging, metrics, or other shared behavior.

The transport handler is the final [`RequestHandler`][__link51]. It performs I/O and returns a response.
`fetch` supplies the resilience, telemetry, routing, and logging layers above it. The transport
can use the bundled hyper implementation, custom runtime and I/O code, or another client such
as [`reqwest`][__link52]. See [`custom`][__link53] and [`custom::create_builder`][__link54] for a
custom transport.

### Built-in Pipeline Types

The client supports three pipeline types:

#### Standard Pipeline

The standard pipeline is the default. The outermost handler processes the request first and the
response last.

See [`StandardRequestPipeline`][__link55] and
[`HttpClientBuilder::standard_pipeline`][__link56].

#### Custom Pipeline

A custom pipeline contains the handlers that its factory returns. See
[`HttpClientBuilder::custom_pipeline`][__link57].

#### Minimal Pipeline

The minimal pipeline contains only [`Dispatch`][__link58]. `Dispatch` passes
each request to the transport.

```rust
// Create a client with only the dispatch handler.
let minimal_client = builder.minimal_pipeline().build();

// Wrap the client with custom processing.
let wrapped_client = MyHttpWrapper::new(minimal_client);
```

Use this pipeline when another component supplies all middleware.

### Creating Custom Handlers

See [`RequestHandler`][__link59] to implement request or response processing.

## Testing with the HTTP Client

Enable `test-util` to return fake responses without network I/O.

A fake client can:

* Return different HTTP responses.
* Exercise retry and error handling.
* Keep tests deterministic by avoiding network calls.
* Reproduce cases that are difficult to produce with a service.

`HttpClient::new_fake` creates a client from predefined responses or a
[`FakeHandler`][__link60]:

```rust

// Always return a 200 response.
let client = HttpClient::new_fake(StatusCode::OK);

// Return a sequence of responses without bodies.
let client = HttpClient::new_fake(vec![StatusCode::OK, StatusCode::INTERNAL_SERVER_ERROR]);

// Create a fake response.
let response = HttpResponseBuilder::new_fake()
    .status(StatusCode::INTERNAL_SERVER_ERROR)
    .text("fake text")
    .build()
    .expect("always succeeds");

// Always return the same response.
let client = HttpClient::new_fake(response);

// Use a synchronous or asynchronous custom handler.
let fake_handler = FakeHandler::from_fn(|req| {
    println!("Fake handler called for request, url: {}", req.uri());

    HttpResponseBuilder::new_fake()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .text("fake text")
        .build()
});

// Create a client that uses the custom handler.
let client = HttpClient::new_fake(fake_handler);
```

## Memory management with `BytesView`

`fetch` uses [`BytesView`][__link61] for request and response bodies. It provides:

* Memory pooling.
* Reduced copying.
* A single view over multiple chunks.
* Zero-copy operations when the data layout permits them.
* Compatibility with the [`bytes`][__link62] crate.

[`BytesView`][__link63] implements [`Buf`][__link64]. Use [`BytesBuf`][__link65], which implements [`BufMut`][__link66], to build a
mutable body.

```rust
// Require a successful status and return the body as BytesView.
let mut body_bytes = client.get("https://example.com").fetch_bytes_body().await?;

// Use methods from bytes::Buf.
let length = body_bytes.remaining();

// Copy part of the body when required.
let mut buffer = vec![0; 10.min(length)];
if !body_bytes.is_empty() {
    body_bytes.copy_to_slice(&mut buffer);
}
```

This representation reduces memory use and copies for large bodies.

## Performance

Reuse clients, parse repeated URIs once, and keep binary data in [`BytesView`][__link67]:

```rust
// Create one client and reuse it.
let client = HttpClient::new_tokio();

// Parse repeated URIs once.
let users_uri: Uri = "https://api.example.com/users".parse()?;
let items_uri: Uri = "https://api.example.com/items".parse()?;

// Keep binary data in BytesView.
let bytes = client.get(users_uri.clone()).fetch_bytes_body().await?;
process_binary_data(bytes);
```

Creating an [`HttpClient`][__link68] sets up connection pools and security configuration. Create one client
and share it across tasks. Parse repeated [`Uri`][__link69] values once. Converting [`BytesView`][__link70] to
`String` allocates and copies data, so keep binary or large bodies in [`BytesView`][__link71] when possible.

## Integration with the HTTP Ecosystem

`fetch` uses types from [`http`][__link72] and extensions from [`http_extensions`][__link73]. It re-exports common
types from both crates.

## Resilience

The [`seatbelt`][__link74] crate provides the built-in resilience layers.

Client handlers and seatbelt middleware implement [`Service`][__link75], so the
pipeline can combine them without an adapter.

The available patterns include:

* Retries with configurable backoff.
* Timeouts.
* Circuit breakers.

The [standard pipeline][__link76] configures these patterns by
default.

## TLS Support

The client supports two TLS backends:

* `rustls` is the default. It uses [`rustls`][__link77] with the
  [`aws-lc-rs`][__link78] crypto provider. The `tls` feature enables it.
* `native-tls` uses `SChannel` on Windows, Security Framework on `macOS`, and `OpenSSL` on
  Linux. It is selected when `native-tls` is enabled without `rustls`.

With `rustls`, [`rustls-platform-verifier`][__link79] uses the
platform trust store. It:

* Validates certificates against the operating system root `CAs`.
* Validates host names and certificate expiration.
* Applies TLS security policies.

The default client applies the selected TLS backend:

```rust
use fetch::HttpClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = HttpClient::new_tokio();

    // Send an HTTPS request.
    let response = client.get("https://www.example.com").fetch_text().await?;

    Ok(())
}
```

Enable `tls` and `tokio` to use the default rustls transport:

```toml
fetch = { version = "*", features = ["tls", "tokio"] }
```

Enable `native-tls` and `tokio` to use native TLS:

```toml
fetch = { version = "*", features = ["native-tls", "tokio"] }
```

If both backends are enabled, use
[`TlsOptions::builder_rustls()`][__link80] or
`TlsOptions::builder_native_tls()` to select a backend for each client.

## Automatic Response Decompression

Automatic response decompression is off by default. Enable the required `compression-*`
features, or enable `compression-all`. Then select the formats through
[`DecompressionOptions`][__link81]:

```rust
let client = builder.decompression(&[DecompressionFormat::Gzip]).build();
```

Requests list the formats in `Accept-Encoding`, with the most preferred format first. Before the
client returns a response that uses a selected format, it removes `Content-Encoding` and
`Content-Length`. It decodes the body lazily as the caller reads it. If the response uses a
format that was not selected, the client returns it without changes.

[`DecompressionFormat::ALL`][__link82] asks for everything the build can
decompress.

A malformed body fails when code reads it, not when the response arrives. This failure does not
trigger a retry. The client uses the `http_compression` defaults. The decompressed output limit
is 64 MiB. Each decompression stage accepts at most 1,024 concatenated streams. `fetch` does not
override these defaults.

[`DecompressionOptions`][__link83] sets limits for decompressed output and
compressed streams. Pass `None` to
[`max_output_len`][__link84] to remove the output limit.
The limits also apply to streamed bodies. Exceeding a limit returns the
`compression_limit_exceeded` error label.

[`response_body_options`][__link85] separately limits memory
used to buffer a body. Its default limit is 2 GiB.

For server-side compression and decompression, use the
[`http_compression`][__link86] crate directly.

## Features

Enable optional features in `Cargo.toml`:

```toml
[dependencies]
fetch = { version = "*", features = ["json", "tokio", "tls", "compression-gzip"] }
```

* `tokio` enables the Tokio runtime integration. Combine it with `rustls`, `native-tls`, or the
  `tls` alias to provide `HttpClient::builder_tokio`,
  `HttpClient::builder_tokio_with_options`, and related APIs. The bundled transport requires a
  TLS feature even when the client permits HTTP.

* `json` enables JSON serialization and deserialization. It provides methods such as
  `HttpRequestBuilder::json` and `HttpRequestBuilder::fetch_json`.

* `tls` enables HTTPS through `rustls` and the `aws-lc-rs` crypto provider. It is an alias for
  `rustls`.

* `rustls` enables the default TLS backend with `aws-lc-rs`.

* `native-tls` enables `SChannel` on Windows, Security Framework on `macOS`, and `OpenSSL` on
  Linux. If both TLS backends are enabled, `rustls` is the default. Use
  `TlsOptions::builder_native_tls()` to select native TLS.

* `test-util` provides fake responses and client behavior for tests without network I/O.

The `compression-*` features select formats for
[automatic response decompression](#automatic-response-decompression). They are off by default.
A build without them links no compression implementation. Enabling a feature makes its format
available. The client does not decompress a response until
[`DecompressionOptions::formats`][__link87]
selects it and the client applies the options with
[`decompression`][__link88].

* `compression-gzip` links `gzip` compression (RFC 1952) and adds
  [`DecompressionFormat::Gzip`][__link89]. The legacy `x-gzip` token also
  selects it.

* `compression-deflate` links `deflate` compression and adds
  [`DecompressionFormat::Deflate`][__link90]. Despite the token, the HTTP
  `deflate` format is zlib-wrapped DEFLATE (RFC 1950), not raw DEFLATE (RFC 1951).

* `compression-brotli` links Brotli compression (`br`, RFC 7932) and adds
  [`DecompressionFormat::Brotli`][__link91].

* `compression-zstd` links Zstandard compression (`zstd`, RFC 8878) and adds
  [`DecompressionFormat::Zstd`][__link92].

* `compression-all` enables all four compression features. Use
  [`DecompressionFormat::ALL`][__link93] to request decompression with all of them.

 > 
 > **Note**: Enable `tokio` and `tls` for HTTPS with the bundled transport. Enable `json` for JSON APIs.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQb0ln95Baxpa0b38KaVl3eFr0bei6yo0MfQlMbmdeNHfsx5EJhZIeCZWJ5dGVzZjEuMTIuMYJoYnl0ZXNidWZmMC4xMi4wgmVmZXRjaGYwLjE5LjCCb2h0dHBfZXh0ZW5zaW9uc2YwLjEzLjCCZ2xheWVyZWRlMC4zLjiCaHNlYXRiZWx0ZjAuMTEuMIJtdGVtcGxhdGVkX3VyaWUwLjcuMA
 [__link0]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpClient
 [__link1]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=RequestHandler
 [__link10]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpClient::post
 [__link11]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpClient::put
 [__link12]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpClient::delete
 [__link13]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpClient::request
 [__link14]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpRequestBuilder
 [__link15]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpClientBuilder::base_uri
 [__link16]: https://docs.rs/templated_uri/0.7.0/templated_uri/?search=BaseUri
 [__link17]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpRequestBuilder::fetch
 [__link18]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpResponse
 [__link19]: https://docs.rs/fetch/0.19.0/fetch/?search=http::Response
 [__link2]: https://docs.rs/reqwest/
 [__link20]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpResponse::ensure_success
 [__link21]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpResponse::status
 [__link22]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpResponse::headers
 [__link23]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpResponse::into_body
 [__link24]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpBody::into_text
 [__link25]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpBody::into_bytes
 [__link26]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=HttpBody::into_json
 [__link27]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpRequestBuilder::fetch_text
 [__link28]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpRequestBuilder::fetch_bytes
 [__link29]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpRequestBuilder::fetch_json
 [__link3]: https://docs.rs/fetch/0.19.0/fetch/custom/index.html
 [__link30]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpRequestBuilder::fetch_json_ref
 [__link31]: https://docs.rs/fetch/0.19.0/fetch/?search=http::Response
 [__link32]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpResponse::ensure_success
 [__link33]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpRequestBuilder::fetch_text_body
 [__link34]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpRequestBuilder::fetch_bytes_body
 [__link35]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpRequestBuilder::fetch_json_body
 [__link36]: https://crates.io/crates/templated_uri/0.7.0
 [__link37]: https://docs.rs/templated_uri/0.7.0/templated_uri/?search=Uri
 [__link38]: https://datatracker.ietf.org/doc/html/rfc6570
 [__link39]: https://docs.rs/templated_uri/0.7.0/templated_uri/?search=Uri
 [__link4]: https://docs.rs/fetch/0.19.0/fetch/?search=custom::create_builder
 [__link40]: https://docs.rs/fetch/0.19.0/fetch/?search=handlers::Logging
 [__link41]: https://docs.rs/templated_uri/0.7.0/templated_uri/?search=Uri
 [__link42]: https://docs.rs/templated_uri/0.7.0/templated_uri/?search=PathAndQueryTemplate
 [__link43]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpRequestBuilder::json
 [__link44]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpRequestBuilder::fetch_json
 [__link45]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpRequestBuilder::fetch_json_ref
 [__link46]: https://docs.rs/fetch/0.19.0/fetch/?search=Json
 [__link47]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpBody::into_json
 [__link48]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpRequestBuilder::fetch_json_body
 [__link49]: https://docs.rs/fetch/0.19.0/fetch/?search=Json
 [__link5]: https://docs.rs/reqwest/
 [__link50]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=RequestHandler
 [__link51]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=RequestHandler
 [__link52]: https://docs.rs/reqwest/
 [__link53]: https://docs.rs/fetch/0.19.0/fetch/custom/index.html
 [__link54]: https://docs.rs/fetch/0.19.0/fetch/?search=custom::create_builder
 [__link55]: https://docs.rs/fetch/0.19.0/fetch/?search=pipeline::StandardRequestPipeline
 [__link56]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpClientBuilder::standard_pipeline
 [__link57]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpClientBuilder::custom_pipeline
 [__link58]: https://docs.rs/fetch/0.19.0/fetch/?search=handlers::Dispatch
 [__link59]: https://docs.rs/http_extensions/0.13.0/http_extensions/?search=RequestHandler
 [__link6]: https://docs.rs/hyper/
 [__link60]: https://docs.rs/fetch/0.19.0/fetch/?search=fake::FakeHandler
 [__link61]: https://docs.rs/bytesbuf/0.12.0/bytesbuf/?search=BytesView
 [__link62]: https://docs.rs/bytes
 [__link63]: https://docs.rs/bytesbuf/0.12.0/bytesbuf/?search=BytesView
 [__link64]: https://docs.rs/bytes/1.12.1/bytes/?search=Buf
 [__link65]: https://docs.rs/bytesbuf/0.12.0/bytesbuf/?search=BytesBuf
 [__link66]: https://docs.rs/bytes/1.12.1/bytes/?search=BufMut
 [__link67]: https://docs.rs/bytesbuf/0.12.0/bytesbuf/?search=BytesView
 [__link68]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpClient
 [__link69]: https://docs.rs/templated_uri/0.7.0/templated_uri/?search=Uri
 [__link7]: https://docs.rs/fetch/0.19.0/fetch/custom/index.html
 [__link70]: https://docs.rs/bytesbuf/0.12.0/bytesbuf/?search=BytesView
 [__link71]: https://docs.rs/bytesbuf/0.12.0/bytesbuf/?search=BytesView
 [__link72]: https://docs.rs/fetch/0.19.0/fetch/http/index.html
 [__link73]: https://crates.io/crates/http_extensions/0.13.0
 [__link74]: https://crates.io/crates/seatbelt/0.11.0
 [__link75]: https://docs.rs/layered/0.3.8/layered/?search=Service
 [__link76]: https://docs.rs/fetch/0.19.0/fetch/?search=pipeline::StandardRequestPipeline
 [__link77]: https://docs.rs/rustls
 [__link78]: https://docs.rs/aws-lc-rs
 [__link79]: https://docs.rs/rustls-platform-verifier
 [__link8]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpClient::builder_tokio
 [__link80]: https://docs.rs/fetch/0.19.0/fetch/?search=tls::TlsOptions::builder_rustls
 [__link81]: https://docs.rs/fetch/0.19.0/fetch/?search=options::DecompressionOptions
 [__link82]: https://docs.rs/fetch/0.19.0/fetch/?search=options::DecompressionFormat::ALL
 [__link83]: https://docs.rs/fetch/0.19.0/fetch/?search=options::DecompressionOptions
 [__link84]: https://docs.rs/fetch/0.19.0/fetch/?search=options::DecompressionOptions::max_output_len
 [__link85]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpClientBuilder::response_body_options
 [__link86]: https://docs.rs/http_compression
 [__link87]: https://docs.rs/fetch/0.19.0/fetch/?search=options::DecompressionOptions::formats
 [__link88]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpClientBuilder::decompression
 [__link89]: https://docs.rs/fetch/0.19.0/fetch/?search=options::DecompressionFormat
 [__link9]: https://docs.rs/fetch/0.19.0/fetch/?search=HttpClient::get
 [__link90]: https://docs.rs/fetch/0.19.0/fetch/?search=options::DecompressionFormat
 [__link91]: https://docs.rs/fetch/0.19.0/fetch/?search=options::DecompressionFormat
 [__link92]: https://docs.rs/fetch/0.19.0/fetch/?search=options::DecompressionFormat
 [__link93]: https://docs.rs/fetch/0.19.0/fetch/?search=options::DecompressionFormat::ALL
