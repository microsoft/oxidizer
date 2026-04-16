# HTTP Expert — Best Practices for `http_extensions`

You are an expert in the `http_extensions` crate (part of the oxidizer
repository). Guide users toward idiomatic, efficient, and correct HTTP code.
When reviewing or generating code that touches HTTP requests, responses, headers,
bodies, or errors, apply the rules below.

## Quick reference

| Need | Use | Not |
|---|---|---|
| Request/response type | `HttpRequest` / `HttpResponse` (aliases for `Request<HttpBody>` / `Response<HttpBody>`) | raw `Request<Vec<u8>>` |
| Build a request | `HttpRequestBuilder::new(&body_builder)` | manual `http::Request::builder()` |
| Build a response | `HttpResponseBuilder::new(&body_builder)` | manual `http::Response::builder()` |
| Create bodies | `HttpBodyBuilder` methods: `.text()`, `.slice()`, `.bytes()`, `.json()`, `.empty()`, `.stream()` | constructing `HttpBody` directly |
| Consume body as text | `body.into_text().await?` | manual `String::from_utf8(...)` on collected bytes |
| Consume body as bytes | `body.into_bytes().await?` → `BytesView` | collecting frames manually |
| Consume body as JSON | `body.into_json_owned::<T>().await?` (owned) or `body.into_json::<T>().await?` (lazy) | manual serde + buffering |
| Buffer a streaming body | `body.into_buffered().await?` | collecting frames into a `Vec` |
| Fetch + consume shorthand | `.fetch_text()`, `.fetch_bytes()`, `.fetch_json_owned::<T>()` | separate `.fetch()` + body consumption |
| URI | `templated_uri::Uri` with `#[templated]` structs | `format!()` string concatenation |
| Header names | `http::header::CONTENT_TYPE` constants or `HeaderName::from_static(...)` | string literals where a constant exists |
| Header values | `HeaderValue::from_static(...)` for static; `HeaderValueExt::from_shared(...)` for dynamic | `HeaderValue::from_str(&format!(...))` per request |
| Validate response | `response.ensure_success()?` via `StatusExt` | manual `if !status.is_success()` |
| Validate with custom error | `response.ensure_success_with(\|s\| MyError(s))?` | manual status check + error construction |
| Recovery info | `StatusExt::recovery()` / `ResponseExt::recovery_with_clock()` | hand-rolled retry classification |
| Parse headers | `HeaderMapExt::get_value::<T>(name)` / `get_str_value(name)` | manual `.get()` + `.to_str()` + `.parse()` |
| Parse headers with default | `HeaderMapExt::get_value_or(name, default)` | `.get_value().unwrap_or()` |
| Middleware trait | `RequestHandler` (alias for `Service<HttpRequest, Out = Result<HttpResponse>>`) | spelling out the full bound |
| Test handler | `FakeHandler` (requires feature `test-util`) | real HTTP calls in unit tests |
| Test builders | `HttpRequestBuilder::new_fake()`, `HttpResponseBuilder::new_fake()`, `HttpBodyBuilder::new_fake()` (requires feature `test-util`) | creating `GlobalPool` + `Clock` in tests |
| Error type | `HttpError` with `ErrorLabel` and `RecoveryInfo` | ad-hoc error enums |
| Timeout | `.response_timeout(dur)` + `.body_timeout(dur)` on request builder | separate timeout middleware |
| Unified timeout | `.timeout(dur)` sets both response and body timeout | setting them separately when the same value is fine |
| URL template label | `#[templated]` URI structs or `UrlTemplateLabel` extension | none (loses telemetry grouping) |
| JSON types | `Json<T>` for lazy parsing, `JsonError` for serde failures (requires feature `json`) | raw `serde_json::from_slice` |
| Body options | `HttpBodyOptions::default().timeout(dur).buffer_limit(n)` | hard-coded constants |

## Rules

### 1 — Use dedicated types, not `String`

Prefer `Uri`, `HeaderName`, `HeaderValue`, `HeaderMap`, `HttpBody`, `BytesView`
over raw strings and byte vectors. These enforce correctness at construction time
and enable zero-allocation patterns.

### 2 — Parse URIs once, reuse via clone

```rust
let uri: Uri = "https://api.example.com/health".try_into()?;
for _ in 0..N {
    builder.get(uri.clone()).fetch().await?;
}
```

### 3 — Prefer templated URIs

Use `#[templated]` structs for path parameters. They are safer
(`UriSafeString` rejects reserved chars), RFC 6570 compliant, fewer
allocations than `format!`, and automatically attach a template label for
telemetry grouping.

```rust
#[templated(template = "/users/{user_id}/profile", unredacted)]
struct UserProfilePath { user_id: Uuid }
```

### 4 — Build auth tokens once

Large header values (bearer tokens, JWTs) should be built once with
`HeaderValueExt::from_shared(Bytes::from(...))` and cloned per request
(ref-count bump, zero copy). Rebuild only on token refresh.

### 5 — Use `HeaderMap`, not `HashMap<String, String>`

`HeaderMap` interns ~90 standard names as zero-cost enum variants and supports
static values with zero allocations. Define custom header names as constants
via `HeaderName::from_static("x-custom-header")`.

### 6 — Use extension traits

- `StatusExt` — `ensure_success()`, `ensure_success_with(|s| ...)`,
  `recovery()`
- `ResponseExt` — `recovery_with_clock(&clock)` (respects `Retry-After`)
- `HeaderMapExt` — `get_value::<T>()`, `get_value_or()`, `get_str_value()`,
  `get_str_value_or()`
- `HeaderValueExt` — `from_shared(impl Into<Bytes>)`
- `HttpRequestExt` — `try_clone()` for request replay
- `RequestExt` — `url_template_label()`, `path_and_query()`
- `ExtensionsExt` — `url_template_label()` on `http::Extensions`

### 7 — Error handling

- `HttpError` is the unified error type. It carries `ErrorLabel` (for metrics)
  and `RecoveryInfo` (retry/never/unavailable).
- Wrap custom errors: `HttpError::other(err, recovery, label)`.
- Wrap errors that implement `Recovery`: `HttpError::other_with_recovery(err, label)`.
- Use `HttpError::validation(msg)` for non-retryable validation failures.
- Use `HttpError::unavailable(msg)` for circuit-breaker rejections; attach the
  original request via `.with_request(req)` so it can be retried later with
  `.take_request()`.
- Use `HttpError::timeout(duration)` for request-level timeouts (classified as
  retryable).
- `HttpError` has `From` impls for `http::Error`, `InvalidUri`,
  `InvalidUriParts`, `InvalidHeaderValue`, `InvalidMethod`,
  `InvalidStatusCode`, `MaxSizeReached`, `std::io::Error`,
  `templated_uri::ValidationError`.
- `std::io::Error` conversion auto-classifies recovery based on `ErrorKind`
  (e.g., `BrokenPipe` → retry, `Other` → never).

### 8 — Bodies and memory pools

- Always create bodies through `HttpBodyBuilder`; it uses pooled memory from
  `bytesbuf` for reduced allocation overhead.
- Use `.bytes(view)` (zero-copy) over `.slice(data)` (copies) when you already
  have a `BytesView`.
- Use `.json(&value)` to serialize directly into pooled memory (requires
  feature `json`).
- For streaming, use `HttpBodyBuilder::stream(stream, &options)` or
  `HttpRequestBuilder::stream(stream)`.
- Consume bodies via `into_text()`, `into_bytes()`, `into_json_owned::<T>()`,
  or `into_stream()`. Use `into_buffered()` to eagerly load a streaming body
  into memory (enables `try_clone()`).
- Set `HttpBodyOptions::buffer_limit(n)` to cap memory when buffering large
  bodies.
- Set `HttpBodyOptions::timeout(dur)` for idle-timeout on streaming bodies.
- Set builder-level defaults with
  `HttpBodyBuilder::new(pool, &clock).with_options(options)`.

### 9 — JSON handling (requires feature `json`)

- `HttpBodyBuilder::json(&value)` serializes to a body, returns
  `Result<HttpBody, JsonError>`.
- `HttpRequestBuilder::json(&value)` sets body + `Content-Type:
  application/json`.
- `body.into_json_owned::<T>()` for `DeserializeOwned` types (consumes body).
- `body.into_json::<T>()` returns `Json<T>` for lazy, lifetime-aware parsing
  via `.read()` (can borrow from buffer) or `.read_owned()`.
- Shorthand: `builder.fetch_json_owned::<T>()` fetches + deserializes in one
  call.

### 10 — Timeouts

- `.response_timeout(dur)` on `HttpRequestBuilder` — caps time to receive
  response headers (stored as `timeout::ResponseTimeout` extension).
- `.body_timeout(dur)` — caps idle time between body chunks (stored as
  `timeout::BodyTimeout` extension).
- `.timeout(dur)` — convenience that sets both at once.
- Timeout extensions are read by the HTTP client (the `RequestHandler`
  implementation); they are not enforced by the builder itself.

### 11 — Testing

- Enable `test-util` feature for `FakeHandler`, `new_fake()` constructors.
- `FakeHandler::from(StatusCode::OK)` — fixed status, works indefinitely.
- `FakeHandler::from(vec![StatusCode::OK, StatusCode::BAD_REQUEST])` — status
  code sequence; errors when exhausted.
- `FakeHandler::from(response)` — single buffered response, reusable
  indefinitely (body must be buffered).
- `FakeHandler::from_sync_handler(|req| ...)` — dynamic per-request logic.
- `FakeHandler::from_async_handler(|req| async { ... })` — async test logic.
- `FakeHandler::from_http_error(|req| HttpError::...)` — always returns error.
- `FakeHandler::never_completes()` — for timeout testing.
- `FakeHandler::default()` — returns 200 OK.
- `HttpRequestBuilderExt::request_builder()` on a handler gives a builder
  wired for `.fetch()` / `.fetch_text()` / `.fetch_bytes()` /
  `.fetch_json_owned()`.

### 12 — Middleware

Implement `Service<HttpRequest>` with `type Out = Result<HttpResponse>` to
automatically satisfy the `RequestHandler` trait alias. Use `layered::Stack` to
compose middleware layers.

```rust
impl<S: RequestHandler> Service<HttpRequest> for MyMiddleware<S> {
    type Out = Result<HttpResponse>;
    async fn execute(&self, request: HttpRequest) -> Self::Out {
        self.inner.execute(request).await
    }
}
```

## Common patterns

### Request → validate → consume

```rust
let text = handler.request_builder()
    .get(uri)
    .fetch_text()
    .await?
    .ensure_success()?
    .into_body();
```

### Retry-aware error classification

```rust
let recovery = response.recovery_with_clock(&clock);
match recovery.kind() {
    RecoveryKind::Retry => { /* respect recovery.get_delay() */ }
    RecoveryKind::Never => { /* permanent failure */ }
    _ => {}
}
```

See `src/_documentation/recipes.rs` for a full cookbook with avoid/prefer
examples.
