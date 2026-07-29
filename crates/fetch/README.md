<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Fetch Logo" width="96">

# Fetch

[![crate.io](https://img.shields.io/crates/v/fetch.svg)](https://crates.io/crates/fetch)
[![docs.rs](https://docs.rs/fetch/badge.svg)](https://docs.rs/fetch)
[![MSRV](https://img.shields.io/crates/msrv/fetch)](https://crates.io/crates/fetch)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

A fast, safe HTTP client that just works.

This crate provides a powerful HTTP client that works with different async runtimes, handles
security properly by default, and makes testing easy. The [`HttpClient`][__link0] provides a clean API
for making HTTP requests without worrying about the complex details of modern HTTP.

## Why a new HTTP client?

`fetch` bundles the capabilities real-world services need into a single client, ready to use
out of the box:

* **Secure, resilient and observable by default**: Strong TLS validation, built-in resilience
  (retries, circuit breaking, hedging), and OpenTelemetry-compatible observability are
  pre-configured for real-world use.
* **Built-in testability**: The `test-util` feature lets you mock HTTP responses without complex
  setup, making tests fast and deterministic.
* **Composable pipeline**: Modular request handlers make it easy to add or customize behaviors
  like logging, metrics, or retries.
* **Memory efficient**: Uses smart pooling and zero-copy techniques to handle large responses
  with minimal overhead.

Crucially, `fetch` delivers these features **without forcing a runtime, an I/O implementation, or
a particular HTTP transport on you**. The request pipeline is built around a *transport handler*
at its leaf that you can swap out, with everything above it — resilience, observability, routing,
logging, retries — layered on top. This makes `fetch`:

* **runtime-agnostic**: Tokio works out of the box, or plug in any async runtime and I/O by
  supplying your own transport handler; and
* **transport-agnostic**: the transport handler is just a [`RequestHandler`][__link1] that turns a request
  into a response, so you can keep the bundled hyper transport, wrap a hand-rolled client, or even
  reuse an existing one like [`reqwest`][__link2].

That makes `fetch` an excellent fit for **libraries that want to stay runtime- and
transport-agnostic**: they depend on `fetch` for its features while leaving the runtime and
transport choice to the consuming application, which plugs in whatever it already uses. See the
[`custom`][__link3] module and [`custom::create_builder`][__link4] for a worked example.

### How does it compare to `reqwest`?

By default both `fetch` and [`reqwest`][__link5] are built on top of the powerful
[`hyper`][__link6] HTTP implementation. While `reqwest` has been the go-to HTTP
client for many Rust applications, `fetch` offers a different set of trade-offs that may
better suit your needs, especially for crates that require resilience and multi-runtime support.
Unlike `reqwest`, `fetch` is not tied to its default transport at all: you can swap hyper out for
any transport — including `reqwest` itself — and keep all of `fetch`’s surrounding features.

|Feature|`fetch`|`reqwest`|
|-------|-------|---------|
|**Runtime Support**|✅ Tokio **and custom runtimes**|✅ Tokio only|
|**Custom Transport / IO**|✅ **Built-in** — plug in your own runtime, I/O, or even another HTTP client (e.g. `reqwest`) as the transport|❌ Not supported|
|**TLS/HTTPS**|✅ Via rustls or native-tls|✅ Via rustls or native-tls|
|**Resilience**|✅ Built-in and default|❌ Optional, external crates required|
|**JSON support**|✅ Built-in|✅ Built-in|
|**Testing tools**|✅ Built-in|❌ Custom, external crates required|
|**`OTel` Metrics/Logging**|✅ Built-in|❌ Custom implementation needed|
|**Advanced HTTP Client Features [^1]**|❌ Not yet supported [^2]|✅ Via optional features|
|**Request Pipeline**|✅ Built-in|❌ Custom, external crates required|
|**Zero-copy Buffers**|✅ Built-in|❌ Partial, uses `Bytes`|
|**Linux support**|✅ Full support|✅ Full support|

[^1]: Advanced HTTP client features include things like file uploads, cookies, proxies, and redirects.

[^2]: The features currently missing (cookies, redirects, forms) may be added in future versions as the
    client matures.

 > 
 > **Note**: If you’re already familiar with `reqwest`, you’ll feel right at home with `fetch`.
 > The APIs are intentionally similar, with familiar methods like `get()`, `post()`, and `fetch()`. Most
 > basic HTTP operations follow the same patterns, making it easy to switch between the two libraries.

## Getting Started

This client runs on the Tokio runtime by default. (Other runtimes can be plugged in via a
custom transport — see the [`custom`][__link7] module.)

```rust
use fetch::{HttpClient, HttpError};

#[tokio::main]
async fn main() -> Result<(), HttpError> {
    // Create a client using the builder
    let client: HttpClient = HttpClient::new_tokio();

    // Retrieve the response body as text. This validates the status (returning an
    // error for `4xx`/`5xx` responses) and hands back the body in a single step.
    let body: String = client.get("https://example.com").fetch_text_body().await?;

    println!("response: {body}");

    Ok(())
}
```

 > 
 > **Customization**: If you need to customize the HTTP client (e.g., add custom handlers, modify timeouts,
 > or configure other options), use [`HttpClient::builder_tokio`][__link8] instead of `new_tokio` to access
 > the full builder API.

## Making Requests

The HTTP client makes it easy to send different types of requests. Use convenient methods like
[`HttpClient::get`][__link9] and [`HttpClient::post`][__link10] for common operations, and the builder pattern to customize
your requests.

### GET Requests

```rust
// Simple GET request
let response: HttpResponse = client
    .get("https://www.example.com")
    .fetch() // Fetch executes the request and returns a response
    .await?;

```

### POST Requests

```rust
// POST request with text body
let response = client
    .post("https://httpbin.org/post")
    .text("the exact body that is sent") // Attaches a text body to the request
    .fetch()
    .await?;

```

### Handling Complex Requests

The client supports all standard HTTP methods through dedicated methods like [`HttpClient::put`][__link11],
[`HttpClient::delete`][__link12], and more. For anything else, use [`HttpClient::request`][__link13] with any HTTP method:

```rust
// Using a custom method
let response = client
    .request(Method::PATCH, "https://api.example.com/items/42")
    .fetch()
    .await?;
```

You can customize requests with headers, specific HTTP versions, or by attaching bodies:

```rust
let response = client
    .post("https://api.example.com/upload")
    // Add HTTP headers
    .header(header::AUTHORIZATION, "Bearer token123")
    .header(header::CONTENT_TYPE, "application/json")
    // Set HTTP version
    .version(Version::HTTP_2)
    .text("{\"name\": \"document.pdf\"}")
    .fetch()
    .await?;
```

All these methods return a [`HttpRequestBuilder`][__link14] object that lets you customize and then execute your request.

### Handling Multiple Requests to the Same Base URI

If you need to make multiple requests to the same base URI efficiently, use the [`HttpClientBuilder::base_uri`][__link15] builder method.
This allows you to set a [`BaseUri`][__link16] for all requests, so you don’t have to repeat the base URI each time.

This setting overrides any base URI set in the URI you pass to the request methods.

```rust
let client = builder
    .base_uri(BaseUri::from_static("https://example.com/api/v1/")) // Trailing slash is mandatory
    .build();

let response = client.get("/foo/bar").fetch().await?; // Full URL called by this is `https://example.com/api/v1/foo/bar`
```

## Handling Responses

When you call [`HttpRequestBuilder::fetch`][__link17], you get an [`HttpResponse`][__link18] with everything about the response -
the body, status code, headers, and more. Under the hood, `HttpResponse` is just a type alias for
[`Response<HttpBody>`][__link19].

Here’s what you can do with a response:

* Check if it worked: [`HttpResponse::ensure_success`][__link20] returns an error if the status isn’t `2xx`.
* Look at status codes: [`HttpResponse::status`][__link21] gives you the HTTP status.
* Read headers: [`HttpResponse::headers`][__link22] lets you access the response headers.
* Get the body: [`HttpResponse::into_body`][__link23] gives you just the response body.
* Process the data: Convert the body to different formats using methods like [`HttpBody::into_text`][__link24],
  [`HttpBody::into_bytes`][__link25], or when the `json` feature is enabled, [`HttpBody::into_json`][__link26].

```rust
// Make a GET request
let mut response: HttpResponse = client.get("https://www.example.com").fetch().await?;

// Check if the response was successful
response = response.ensure_success()?;

// Check the headers
println!("Headers: {}", response.headers().len());

// Consume the response and extract the body
let body: HttpBody = response.into_body();

// Process the body as text
let text: String = body.into_text().await?;

println!("Response body: {}", text);
```

### Specialized Fetch Methods

Instead of calling [`HttpRequestBuilder::fetch`][__link27] and then converting the response body separately, use these
convenient shortcut methods:

* [`fetch_text`][__link28]: Gets the response body as a string in one step.
* [`fetch_bytes`][__link29]: Gets the body as a memory-efficient `BytesView`.
* [`fetch_json`][__link30]: Gets the response body as owned JSON (requires `json` feature).
* [`fetch_json_ref`][__link31]: Gets the response body as zero-copy JSON (requires `json` feature).

These methods automatically convert the response body to the format you want (string, JSON, etc.),
saving you from handling the raw [`HttpBody`][__link32] type directly. They return a [`Response<T>`][__link33] where `T`
is your desired format, so you still get all response details and can check the status and headers
before using the body.

```rust
// Retrieve the response as text
let response = client
    .get("https://api.example.com/users")
    .fetch_text()
    .await?;

// We can examine response metadata before handling the body
println!("Status: {}", response.status());
println!("Content-Type: {:?}", response.headers().get("content-type"));

// Then ensure success and extract the body
let text: String = response
    .ensure_success()? // Ensure the response was successful
    .into_body(); // Discard the response metadata and get the body as a string

```

### Body-Only Shortcuts

When you only need the body of a *successful* response, the `_body` variants go one step further:
they call [`ensure_success`][__link34] for you, discard the response
metadata, and return just the materialized body.

* [`fetch_text_body`][__link35]: Validates the status and returns the body as a `String`.
* [`fetch_bytes_body`][__link36]: Validates the status and returns the body as a `BytesView`.
* [`fetch_json_body`][__link37]: Validates the status and deserializes the body into an owned value (requires `json` feature).

```rust
// Fetch, validate the status, and extract the body in a single call
let text: String = client
    .get("https://api.example.com/users")
    .fetch_text_body()
    .await?;
println!("body: {text}");
```

## URL Handling

The HTTP client uses the [`templated_uri`][__link38] crate for
URL handling, which provides a powerful and flexible way to work with URIs.

You can use the [`Uri`][__link39] type to build URIs with templated paths and queries, allowing you to
create URLs with dynamic segments and query parameters.
The template format follows [RFC 6570][__link40] level 3,
which means you can use it to easily template more complex paths and queries as well.

You can also use the [`Uri`][__link41] type or string types to represent URIs for backwards compatibility, or
if you don’t need templated paths. In that case, the whole `PathAndQuery` string is treated as a template.

[`handlers::Logging`][__link42] will log the used URL template as
`url.path.template`

For example, you can create a [`Uri`][__link43] with a templated path like this:

```rust
use templated_uri::{BaseUri, EscapedString, PathAndQueryTemplate, Uri, templated};

#[templated(template = "/users/{user_id}/", unredacted)]
#[derive(Clone)]
struct UserPath {
    user_id: EscapedString, // EscapedString ensures the value is safe for use in URIs
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

`templated_uri` supports classification of URL paths and queries using the `data_privacy` crate.

You can also use the `classified` attribute to mark [`PathAndQueryTemplate`][__link44]
structs as classified, allowing you to use classified types from `data_privacy` in your URL templates.

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

Working with JSON APIs is straightforward with the `json` feature, which offers:

* **Send JSON data**: [`HttpRequestBuilder::json`][__link45] serializes any Rust type to JSON.
* **Receive owned JSON**: [`HttpRequestBuilder::fetch_json`][__link46] deserializes
  directly into owned Rust types. This is the common case: the data can outlive the response and cross thread boundaries.
* **Receive zero-copy JSON**: [`HttpRequestBuilder::fetch_json_ref`][__link47] returns a
  [`Json<T>`][__link48] wrapper that borrows strings and byte arrays directly from the response buffer for maximum
  performance. Reach for it in hot paths where you can work with borrowed data.
* **Convert bodies to JSON**: [`HttpBody::into_json`][__link49] transforms response bodies into JSON values.

### Owned JSON with `fetch_json`

Use `fetch_json` when you need owned data; it deserializes the JSON directly into your
target type with owned `String` fields. This is the most common choice:

```rust
// Define a Person type with owned data
#[derive(Serialize, Deserialize)]
struct Person {
    id: u32,
    name: String,
}

let person = Person {
    id: 1,
    name: "Alice Johnson".to_owned(),
};

// Send and receive owned JSON
let response: Response<Person> = client
    .put("https://api.company.com/people")
    .json(&person) // Add JSON payload
    .fetch_json::<Person>() // Returns Response<Person> directly
    .await?;

// You can inspect the response metadata if needed
let response = response.ensure_success()?;

// Extract the deserialized Person directly - no wrapper needed
let person: Person = response.into_body();

println!("Person retrieved, name: {}", person.name);

```

If you only need the body of a successful response, [`fetch_json_body`][__link50]
goes one step further and folds the fetch, status check, and owned deserialization into a single call.

### Zero-Copy JSON with `fetch_json_ref`

Pair `fetch_json_ref` with borrowed string fields (`Cow<'a, str>`) to avoid allocations; the
[`Json<T>`][__link51] wrapper borrows directly from the response buffer. Prefer `Cow<'a, str>` over
`&'a str`, as it transparently falls back to an owned value when a JSON string was escaped in the buffer
and cannot be borrowed:

```rust
// Define a Person type that borrows data to avoid allocations
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

// Send and receive zero-copy JSON
let response: Response<Json<Person>> = client
    .put("https://api.company.com/people")
    .json(&person) // Add JSON payload
    .fetch_json_ref::<Person>() // Returns Response<Json<Person>>
    .await?;

// You can inspect the response metadata if needed
let response = response.ensure_success()?;

// Extract the JSON wrapper from the response
let mut json_body: Json<Person> = response.into_body();

// Parse the JSON data using zero-copy deserialization.
// The parsed Person borrows string data from the underlying buffer.
let person: Person = json_body.read()?;

println!("Person retrieved, name: {}", person.name);

```

This minimizes heap allocations and copying because string fields borrow directly from the
response buffer instead of allocating new memory for each string, at the cost of tying the parsed
value to the response buffer’s lifetime.

## Request Pipeline

The HTTP client uses a pipeline architecture to process requests. Think of it like an assembly
line - every request passes through a sequence of [`RequestHandler`][__link52]s, each handling
a specific aspect of HTTP communication.

Each handler in the pipeline can:

* Modify the request before it’s sent
* Intercept the request completely (e.g., for caching)
* Process the response after it’s received
* Add cross-cutting functionality like logging or metrics

At the very end of the pipeline sits the **transport handler** — the leaf [`RequestHandler`][__link53]
that actually performs the I/O and turns a request into a response. This is the seam that makes
`fetch` transport- and runtime-agnostic: everything above the transport (resilience, telemetry,
routing, logging, …) is supplied by `fetch`, while the transport itself can be the bundled
hyper-based implementation, your own runtime/I/O, or a wrapper around an existing HTTP client
such as [`reqwest`][__link54]. To supply your own transport, see the
[`custom`][__link55] module and [`custom::create_builder`][__link56].

### Built-in Pipeline Types

The client offers three types of pipelines to suit different needs - think of them as
different levels of “batteries included”:

#### Standard Pipeline

The standard pipeline is what you get by default - it includes all the essential handlers
you’ll want for production use. Handlers are applied in a nested structure, with the outermost
handler processing the request first and the response last.

See the [`StandardRequestPipeline`][__link57] and [`HttpClientBuilder::standard_pipeline`][__link58]
for more details and examples.

#### Custom Pipeline

When you need precise control over request processing, you can build a custom pipeline with
exactly the handlers you want. See the [`HttpClientBuilder::custom_pipeline`][__link59] method for
more details and examples.

#### Minimal Pipeline

For maximum flexibility, you can use the minimal pipeline that includes only the
essential [`Dispatch`][__link60] handler that actually sends requests to the network.
This gives you a clean slate to build on:

```rust
// Create a client with just the dispatch handler
let minimal_client = builder.minimal_pipeline().build();

// Then wrap it with your own processing logic
let wrapped_client = MyHttpWrapper::new(minimal_client);
```

This is great when you need to implement your own complete request processing pipeline
or integrate with external middleware systems.

### Creating Custom Handlers

To add your own processing logic, see the [`RequestHandler`][__link61] trait documentation, which covers
patterns for modifying requests, processing responses, and integrating with the pipeline.

## Testing with the HTTP Client

The `fetch` crate makes testing easy with its built-in fake response system. Enable the
`test-util` feature to simulate HTTP responses without making real network requests.

By using the fake HTTP client in your tests, you can:

* Test your code’s handling of different HTTP responses
* Verify retry behaviors and error handling
* Make tests fast and deterministic by avoiding actual network calls
* Test edge cases that would be challenging to reproduce with real services

The simplest way to create a test client is `HttpClient::new_fake`, which responds with predefined
responses instead of making real HTTP requests. It accepts various parameters to streamline testing:

```rust

// Create a fake HTTP client that always returns a 200 response
let client = HttpClient::new_fake(StatusCode::OK);

// Create a fake HTTP client that returns a sequence of responses without a body
let client = HttpClient::new_fake(vec![StatusCode::OK, StatusCode::INTERNAL_SERVER_ERROR]);

// Create a fake response
let response = HttpResponseBuilder::new_fake()
    .status(StatusCode::INTERNAL_SERVER_ERROR)
    .text("fake text")
    .build()
    .expect("always succeeds");

// Create a fake HTTP client that always returns the same response
let client = HttpClient::new_fake(response);

// Create a fake HTTP client that uses a custom handler. The handler can be
// synchronous or asynchronous. Usually for testing, the synchronous handler is sufficient.
let fake_handler = FakeHandler::from_fn(|req| {
    println!("Fake handler called for request, url: {}", req.uri());

    HttpResponseBuilder::new_fake()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .text("fake text")
        .build()
});

// Create a fake HTTP client that uses the custom handler
let client = HttpClient::new_fake(fake_handler);
```

## Smart Memory Management with `BytesView`

When handling large HTTP responses or sending big requests, memory usage matters, so `fetch` uses
[`BytesView`][__link62] for request and response bodies. Its key features:

* **Memory Pooling**: Reuses memory instead of constantly allocating and freeing it.
* **Less Copying**: Smart buffer management reduces unnecessary data copying.
* **Multiple Chunks**: Can handle data as multiple pieces that look like a single buffer.
* **Zero-Copy When Possible**: Avoids copying data when it can for better performance.
* **Works with Ecosystem**: Fully compatible with the popular [`bytes`][__link63] crate.

You can use [`BytesView`][__link64] just like other byte buffer types because it implements the same
interfaces ([`Buf`][__link65] and [`BufMut`][__link66]) as the [`bytes`][__link67] crate:

```rust
// Fetch the response body directly as a BytesView (validating the status along the way)
let mut body_bytes = client.get("https://example.com").fetch_bytes_body().await?;

// Work with the BytesView using standard bytes methods
let length = body_bytes.remaining();

// Easy to extract data when you need it
let mut buffer = vec![0; 10.min(length)];
if !body_bytes.is_empty() {
    body_bytes.copy_to_slice(&mut buffer);
}
```

This lets your app handle large files, streaming media, or other big data without
wasting memory or hurting performance.

## Performance Best Practices

Follow these tips for the best performance:

```rust
// 1. Create a client ONCE and reuse it
let client = HttpClient::new_tokio();

// 2. Parse URIs ahead of time for repeated use
let users_uri: Uri = "https://api.example.com/users".parse()?;
let items_uri: Uri = "https://api.example.com/items".parse()?;

// 3. Work with raw BytesView to avoid allocations when possible
let bytes = client.get(users_uri.clone()).fetch_bytes_body().await?;
process_binary_data(bytes);
```

In detail:

* **Reuse your client**: Creating an [`HttpClient`][__link68] is expensive (connection pooling, security setup).
  Create it once and keep using it throughout your application. Share a single instance across
  multiple tasks.
* **Pre-parse URIs**: If you’re repeatedly calling the same endpoints, parse the [`Uri`][__link69]s once and
  reuse them to skip the parsing overhead.
* **Work with raw [`BytesView`][__link70]**: Converting between formats (like [`BytesView`][__link71] to `String`) creates
  allocations and copies data. When handling binary data or large responses, work with [`BytesView`][__link72] directly.

## Integration with the HTTP Ecosystem

Instead of creating our own HTTP types from scratch, we use extensions and wrappers around
the widely adopted [`http`][__link73] crate. These extensions are defined in the [`http_extensions`][__link74] crate
and re-exported here for convenience.

## Resilience

The HTTP client has built-in resilience features powered by the [`seatbelt`][__link75] crate. These
resilience patterns help your application handle failures gracefully and maintain availability
even when network issues or server problems occur.

The resilience middleware integrates directly into the request pipeline via the
[`Service`][__link76] trait. Because both the client’s handlers and the seatbelt
middleware implement this trait, they compose seamlessly - no adapter code is needed to mix
resilience patterns with other request processing logic.

Common resilience patterns available include:

* **Retries**: Automatically retry failed requests with configurable backoff strategies
* **Timeouts**: Prevent requests from hanging indefinitely
* **Circuit Breakers**: Fail fast when a service is down to avoid cascading failures

These patterns are already configured in the [standard pipeline][__link77] with sensible defaults.

## TLS Support

The HTTP client supports two TLS backends for making HTTPS requests:

* **`rustls`** (default): Uses [`rustls`][__link78] with the
  [`aws-lc-rs`][__link79] crypto provider. This is the recommended
  backend and is selected by default when the `tls` feature is enabled.
* **`native-tls`**: Uses the platform’s native TLS implementation (`SChannel` on Windows,
  Security Framework on `macOS`, `OpenSSL` on Linux). This can be explicitly selected, or is
  chosen automatically when the `native-tls` feature is enabled and `rustls` is not.

When using the `rustls` backend, the HTTP client validates server certificates against the
platform trust store via [`rustls-platform-verifier`][__link80],
which takes care of essential TLS operations:

* Verifies certificates against the operating system’s trusted root `CAs`.
* Validates host names and checks certificate expiration.
* Enforces TLS security policies.

TLS is configured automatically; simply construct a client and make HTTPS requests:

```rust
use fetch::HttpClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = HttpClient::new_tokio();

    // Now you can make HTTPS requests
    let response = client.get("https://www.example.com").fetch_text().await?;

    Ok(())
}
```

To enable TLS support, add the `tls` feature (which enables `rustls` by default) to your dependencies:

```toml
fetch = { version = "*", features = ["tls", "tokio"] }
```

To use native TLS instead, enable the `native-tls` feature explicitly:

```toml
fetch = { version = "*", features = ["native-tls", "tokio"] }
```

You can also select the TLS backend at runtime via [`TlsOptions::builder_rustls()`][__link81] or
`TlsOptions::builder_native_tls()` when both features are enabled, allowing different client
instances to use different backends.

## Features

The `fetch` crate provides several optional features that you can enable in your `Cargo.toml`:

```toml
[dependencies]
fetch = { version = "*", features = ["json", "tokio"] }
```

* **`tokio`**: Enables integration with the Tokio runtime. This feature provides the `HttpClient::builder_tokio`
  constructor and related APIs for using the HTTP client in a Tokio-based application.

* **`json`**: Adds support for JSON serialization and deserialization, enabling methods like
  `HttpRequestBuilder::json` for sending JSON data and `HttpRequestBuilder::fetch_json` for receiving JSON responses.

* **`tls`**: Enables TLS support using `rustls` with the `aws-lc-rs` crypto provider. This is the
  recommended way to enable HTTPS support and is an alias for the `rustls` feature.

* **`rustls`**: Enables the `rustls` TLS backend with `aws-lc-rs`. This is the default TLS backend
  and is selected automatically by the `tls` feature.

* **`native-tls`**: Enables the platform native TLS backend (`SChannel` on Windows, Security Framework
  on `macOS`, `OpenSSL` on Linux). Use this when you need the platform’s native TLS stack. When both
  `rustls` and `native-tls` are enabled, `rustls` is the default but you can select native TLS via
  `TlsOptions::builder_native_tls()`.

* **`test-util`**: Provides APIs to fake responses and HTTP client behavior for testing purposes.
  This feature makes it easy to write fast, deterministic tests without making real network requests.

 > 
 > **Note**: Most users should enable the `tokio` feature along with the `tls` feature for HTTPS
 > support. The `json` feature is recommended for most applications that need to work with JSON APIs.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbRcdYrc3P77cbVjz14MYzPFkbTKiKwHYuBbcbSr09Rcd_lPZhZIeCZWJ5dGVzZjEuMTIuMIJoYnl0ZXNidWZlMC43LjCCZWZldGNoZjAuMTQuMIJvaHR0cF9leHRlbnNpb25zZTAuOC4wgmdsYXllcmVkZTAuMy42gmhzZWF0YmVsdGUwLjYuMYJtdGVtcGxhdGVkX3VyaWUwLjMuNQ
 [__link0]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpClient
 [__link1]: https://docs.rs/http_extensions/0.8.0/http_extensions/?search=RequestHandler
 [__link10]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpClient::post
 [__link11]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpClient::put
 [__link12]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpClient::delete
 [__link13]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpClient::request
 [__link14]: https://docs.rs/http_extensions/0.8.0/http_extensions/?search=HttpRequestBuilder
 [__link15]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpClientBuilder::base_uri
 [__link16]: https://docs.rs/templated_uri/0.3.5/templated_uri/?search=BaseUri
 [__link17]: https://docs.rs/http_extensions/0.8.0/http_extensions/?search=HttpRequestBuilder::fetch
 [__link18]: https://docs.rs/http_extensions/0.8.0/http_extensions/?search=HttpResponse
 [__link19]: https://docs.rs/fetch/0.14.0/fetch/?search=http::Response
 [__link2]: https://docs.rs/reqwest/
 [__link20]: https://docs.rs/http_extensions/0.8.0/http_extensions/?search=HttpResponse::ensure_success
 [__link21]: https://docs.rs/http_extensions/0.8.0/http_extensions/?search=HttpResponse::status
 [__link22]: https://docs.rs/http_extensions/0.8.0/http_extensions/?search=HttpResponse::headers
 [__link23]: https://docs.rs/http_extensions/0.8.0/http_extensions/?search=HttpResponse::into_body
 [__link24]: https://docs.rs/http_extensions/0.8.0/http_extensions/?search=HttpBody::into_text
 [__link25]: https://docs.rs/http_extensions/0.8.0/http_extensions/?search=HttpBody::into_bytes
 [__link26]: https://docs.rs/http_extensions/0.8.0/http_extensions/?search=HttpBody::into_json
 [__link27]: https://docs.rs/http_extensions/0.8.0/http_extensions/?search=HttpRequestBuilder::fetch
 [__link28]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpRequestBuilder::fetch_text
 [__link29]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpRequestBuilder::fetch_bytes
 [__link3]: https://docs.rs/fetch/0.14.0/fetch/custom/index.html
 [__link30]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpRequestBuilder::fetch_json
 [__link31]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpRequestBuilder::fetch_json_ref
 [__link32]: https://docs.rs/http_extensions/0.8.0/http_extensions/?search=HttpBody
 [__link33]: https://docs.rs/fetch/0.14.0/fetch/?search=http::Response
 [__link34]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpResponse::ensure_success
 [__link35]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpRequestBuilder::fetch_text_body
 [__link36]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpRequestBuilder::fetch_bytes_body
 [__link37]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpRequestBuilder::fetch_json_body
 [__link38]: https://crates.io/crates/templated_uri/0.3.5
 [__link39]: https://docs.rs/templated_uri/0.3.5/templated_uri/?search=Uri
 [__link4]: https://docs.rs/fetch/0.14.0/fetch/?search=custom::create_builder
 [__link40]: https://datatracker.ietf.org/doc/html/rfc6570
 [__link41]: https://docs.rs/templated_uri/0.3.5/templated_uri/?search=Uri
 [__link42]: https://docs.rs/fetch/0.14.0/fetch/?search=handlers::Logging
 [__link43]: https://docs.rs/templated_uri/0.3.5/templated_uri/?search=Uri
 [__link44]: https://docs.rs/templated_uri/0.3.5/templated_uri/?search=PathAndQueryTemplate
 [__link45]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpRequestBuilder::json
 [__link46]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpRequestBuilder::fetch_json
 [__link47]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpRequestBuilder::fetch_json_ref
 [__link48]: https://docs.rs/fetch/0.14.0/fetch/?search=Json
 [__link49]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpBody::into_json
 [__link5]: https://docs.rs/reqwest/
 [__link50]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpRequestBuilder::fetch_json_body
 [__link51]: https://docs.rs/fetch/0.14.0/fetch/?search=Json
 [__link52]: https://docs.rs/http_extensions/0.8.0/http_extensions/?search=RequestHandler
 [__link53]: https://docs.rs/http_extensions/0.8.0/http_extensions/?search=RequestHandler
 [__link54]: https://docs.rs/reqwest/
 [__link55]: https://docs.rs/fetch/0.14.0/fetch/custom/index.html
 [__link56]: https://docs.rs/fetch/0.14.0/fetch/?search=custom::create_builder
 [__link57]: https://docs.rs/fetch/0.14.0/fetch/?search=pipeline::StandardRequestPipeline
 [__link58]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpClientBuilder::standard_pipeline
 [__link59]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpClientBuilder::custom_pipeline
 [__link6]: https://docs.rs/hyper/
 [__link60]: https://docs.rs/fetch/0.14.0/fetch/?search=handlers::Dispatch
 [__link61]: https://docs.rs/http_extensions/0.8.0/http_extensions/?search=RequestHandler
 [__link62]: https://docs.rs/bytesbuf/0.7.0/bytesbuf/?search=BytesView
 [__link63]: https://docs.rs/bytes
 [__link64]: https://docs.rs/bytesbuf/0.7.0/bytesbuf/?search=BytesView
 [__link65]: https://docs.rs/bytes/1.12.0/bytes/?search=Buf
 [__link66]: https://docs.rs/bytes/1.12.0/bytes/?search=BufMut
 [__link67]: https://docs.rs/bytes
 [__link68]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpClient
 [__link69]: https://docs.rs/templated_uri/0.3.5/templated_uri/?search=Uri
 [__link7]: https://docs.rs/fetch/0.14.0/fetch/custom/index.html
 [__link70]: https://docs.rs/bytesbuf/0.7.0/bytesbuf/?search=BytesView
 [__link71]: https://docs.rs/bytesbuf/0.7.0/bytesbuf/?search=BytesView
 [__link72]: https://docs.rs/bytesbuf/0.7.0/bytesbuf/?search=BytesView
 [__link73]: https://docs.rs/fetch/0.14.0/fetch/http/index.html
 [__link74]: https://crates.io/crates/http_extensions/0.8.0
 [__link75]: https://crates.io/crates/seatbelt/0.6.1
 [__link76]: https://docs.rs/layered/0.3.6/layered/?search=Service
 [__link77]: https://docs.rs/fetch/0.14.0/fetch/?search=pipeline::StandardRequestPipeline
 [__link78]: https://docs.rs/rustls
 [__link79]: https://docs.rs/aws-lc-rs
 [__link8]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpClient::builder_tokio
 [__link80]: https://docs.rs/rustls-platform-verifier
 [__link81]: https://docs.rs/fetch/0.14.0/fetch/?search=tls::TlsOptions::builder_rustls
 [__link9]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpClient::get
