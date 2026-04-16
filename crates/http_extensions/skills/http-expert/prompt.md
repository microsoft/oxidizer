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
| URI | `templated_uri::Uri` with `#[templated]` structs | `format!()` string concatenation |
| Header names | `http::header::CONTENT_TYPE` constants or `HeaderName::from_static(...)` | string literals where a constant exists |
| Header values | `HeaderValue::from_static(...)` for static; `HeaderValueExt::from_shared(...)` for dynamic | `HeaderValue::from_str(&format!(...))` per request |
| Validate response | `response.ensure_success()?` via `StatusExt` | manual `if !status.is_success()` |
| Recovery info | `StatusExt::recovery()` / `ResponseExt::recovery_with_clock()` | hand-rolled retry classification |
| Parse headers | `HeaderMapExt::get_value::<T>(name)` | manual `.get()` + `.to_str()` + `.parse()` |
| Middleware trait | `RequestHandler` (alias for `Service<HttpRequest, Out = Result<HttpResponse>>`) | spelling out the full bound |
| Test handler | `FakeHandler` (feature `test-util`) | real HTTP calls in unit tests |
| Test builders | `HttpRequestBuilder::new_fake()`, `HttpResponseBuilder::new_fake()`, `HttpBodyBuilder::new_fake()` | creating `GlobalPool` + `Clock` in tests |
| Error type | `HttpError` with `ErrorLabel` and `RecoveryInfo` | ad-hoc error enums |
| Timeout | `.response_timeout(dur)` + `.body_timeout(dur)` on the request builder | separate timeout middleware |
| URL template label | `#[templated]` URI structs or `UrlTemplateLabel` extension | none (loses telemetry grouping) |

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
static values with zero allocations.

### 6 — Use extension traits

- `StatusExt` — `ensure_success()`, `recovery()`
- `ResponseExt` — `recovery_with_clock()` (respects `Retry-After`)
- `HeaderMapExt` — `get_value::<T>()`, `get_str_value()`
- `HeaderValueExt` — `from_shared()`
- `HttpRequestExt` — `try_clone()` for request replay
- `RequestExt` — `url_template_label()`
- `ExtensionsExt` — `url_template_label()` on `Extensions`

### 7 — Error handling

- `HttpError` is the unified error type. It carries `ErrorLabel` (for metrics)
  and `RecoveryInfo` (retry/never/unavailable).
- Wrap custom errors with `HttpError::other(err, recovery, label)`.
- Use `HttpError::validation(msg)` for non-retryable validation failures.
- Use `HttpError::unavailable(msg)` for circuit-breaker rejections; attach the
  original request via `.with_request(req)` so it can be retried later.
- `HttpError` has `From` impls for `http::Error`, `InvalidUri`,
  `InvalidHeaderValue`, `std::io::Error`, `templated_uri::ValidationError`, etc.

### 8 — Bodies and memory pools

- Always create bodies through `HttpBodyBuilder`; it uses pooled memory from
  `bytesbuf` for reduced allocation overhead.
- Use `.bytes(view)` (zero-copy) over `.slice(data)` (copies) when you already
  have a `BytesView`.
- For streaming, use `.stream(stream, &options)` or
  `HttpRequestBuilder::stream(stream)`.
- Set `HttpBodyOptions::buffer_limit(n)` to cap memory when buffering large
  bodies. Default limit is 2 GB.
- Set `HttpBodyOptions::timeout(dur)` for idle-timeout on streaming bodies.

### 9 — Testing

- Enable `test-util` feature for `FakeHandler`, `new_fake()` constructors.
- `FakeHandler::from(StatusCode::OK)` — fixed status, works indefinitely.
- `FakeHandler::from(vec![StatusCode::OK, StatusCode::BAD_REQUEST])` — sequence.
- `FakeHandler::from(response)` — single buffered response, reusable.
- `FakeHandler::from_sync_handler(|req| ...)` — dynamic per-request logic.
- `FakeHandler::never_completes()` — for timeout testing.
- `HttpRequestBuilderExt::request_builder()` on a handler gives a builder
  wired for `.fetch()`.

### 10 — Middleware

Implement `Service<HttpRequest>` with `type Out = Result<HttpResponse>` to
automatically satisfy the `RequestHandler` trait alias. Use `layered::Stack` to
compose middleware layers.

See `src/_documentation/recipes.rs` for a full cookbook with avoid/prefer
examples.
