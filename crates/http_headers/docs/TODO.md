# TODO

This file tracks outstanding work for the `http_headers` family. Completed
items are deleted rather than retained as history.

## Contents

### Features

- [F1](#f1) — Classify semantically sensitive typed headers with `data_privacy_core`
- [F2](#f2) — Add byte-oriented redaction before redacting typed headers
- [F3](#f3) — Convert `templated_uri::Uri` into `LocationOwned`

### Documentation

- [D1](#d1) — Document typed headers in the workspace HTTP APIs

## Features

<a id="f1"></a>
### F1 — Classify semantically sensitive typed headers with `data_privacy_core`

**Area:** `http_headers` typed header values · **Priority:** Medium · **Effort:** Medium

Integrate at the typed-header boundary, where the header name supplies stable
semantics, rather than on `FieldValue`. The raw value types can represent both
sensitive and nonsensitive data, while `Classified::data_class` must always
return a class. Handwritten implementations should depend on
`data_privacy_core`, not the full macro and redaction-engine crate. The
existing `is_sensitive` marker remains the transport and secret-safe `Debug`
signal used by `http::HeaderValue` interoperability.

- `crates/http_headers/src/headers/authorization.rs` — Authorization descriptor and typed owned/view values
- `crates/http_headers/src/headers/set_cookie.rs` — Set-Cookie descriptor and typed owned/view values
- `crates/http_headers/src/headers/location.rs` — Location descriptor and typed owned/view values
- `crates/http_headers/src/field_value.rs` — representation-level Boolean sensitivity marker
- `crates/data_privacy_core/src/classified.rs` — `Classified` requires an unconditional `DataClass`

**Done when:** an approved taxonomy defines classes for Authorization,
Set-Cookie, and Location; their borrowed and owned typed values implement
`Classified` through `data_privacy_core`; API tests cover every implementation;
and encoding still preserves the existing `is_sensitive` behavior.

---

<a id="f2"></a>
### F2 — Add byte-oriented redaction before redacting typed headers

**Area:** `data_privacy_core` redaction API and `http_headers` typed values · **Priority:** Medium · **Effort:** Large

Do not expose policy-driven redaction for typed headers through the current
string-only API. HTTP field values can contain valid non-UTF-8 `obs-text`, and
Set-Cookie exposes its values as bytes. Converting such values lossily before
hashing or redaction would change their identity; rejecting them would leave a
valid sensitive-value path unsupported.

- `crates/data_privacy_core/src/redactor.rs` — `Redactor::redact` accepts only `&str` and `fmt::Write`
- `crates/http_headers/src/headers/set_cookie.rs` — owned cookies iterate as byte-valued `FieldValue`s
- `crates/http_headers/src/headers/set_cookie.rs` — borrowed cookies iterate as byte-valued `FieldValueRef`s
- `crates/http_headers/src/headers/set_cookie.rs` — valid cookie field values are checked as bytes

**Done when:** `data_privacy_core` provides an explicit byte-oriented
redaction contract, typed header redaction uses it without UTF-8 coercion, and
tests cover arbitrary valid field bytes including `obs-text` for pass-through,
replacement, and hashing policies.

---

<a id="f3"></a>
### F3 — Convert `templated_uri::Uri` into `LocationOwned`

**Area:** `templated_uri` integration with `http_headers::headers::LocationOwned` · **Priority:** Low · **Effort:** Small

Provide an opt-in, one-way construction adapter for applications that build
classified redirect targets with `templated_uri`. Keep the integration in
`templated_uri` behind an optional dependency on `http_headers`; the
lower-level header crate should not acquire a dependency on the templating and
privacy stack. The adapter must materialize the URI for transmission and
produce a normal sensitive `LocationOwned`, without claiming to preserve the
template's per-component classifications after conversion.

This conversion is deliberately not a replacement for `Location` validation.
`Location` accepts the complete RFC 3986 URI-reference grammar, including
relative, empty, and fragment-only references, while `templated_uri` models
HTTP request URIs and does not support fragments.

- `crates/http_headers/src/headers/location.rs` — `Location` accepts RFC 3986 URI-references
- `crates/http_headers/src/headers/location.rs` — string conversion validates and marks the value sensitive
- `crates/templated_uri/src/lib.rs` — URI templates intentionally exclude fragments
- `crates/templated_uri/src/uri.rs` — `Uri` already materializes fallibly as `http::Uri`

**Done when:** an optional `http_headers` integration feature in
`templated_uri` implements `TryFrom<templated_uri::Uri> for LocationOwned`
with an error that preserves materialization and Location-validation failures;
converted values remain sensitive; and tests cover absolute and relative
path/query targets plus rejection or exclusion of unsupported fragment forms.

## Documentation

<a id="d1"></a>
### D1 — Document typed headers in the workspace HTTP APIs

**Area:** `http_extensions`, `fetch`, and `rest_over_grpc` examples · **Priority:** Medium · **Effort:** Small

Add concise, compiling examples showing how the workspace's HTTP-facing APIs
use `http_headers` without duplicating its parsing or encoding methods.
`http_extensions` and `fetch` should demonstrate typed fields on their built
`http::Request` and `http::Response` values and through builder
`headers_mut()` access. `rest_over_grpc` should use the explicit request and
response header-map accessors so the direction of each operation remains
clear.

- `crates/http_extensions/src/http_request_builder.rs` — mutable request-builder headers
- `crates/http_extensions/src/http_response_builder.rs` — mutable response-builder headers
- `crates/fetch/src/lib.rs` — public `http` request, response, and header-map re-exports
- `crates/rest_over_grpc/src/context.rs` — explicit request-header access
- `crates/rest_over_grpc/src/context.rs` — explicit response-header access
- `crates/rest_over_grpc/src/http_response.rs` — mutable neutral-response headers

**Done when:** checked examples decode Authorization or User-Agent from
requests and encode representative Location, ETag, and repeated Set-Cookie
response fields in each applicable crate, with feature requirements documented
and malformed input handled as `DecodeError`.
