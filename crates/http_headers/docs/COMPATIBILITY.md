# Compatibility, wire guarantees, and versioning

This document states what a caller may rely on across releases: the semver
policy this pre-1.0 crate follows, the minimum supported Rust version, the
feature flags and how they interact, and which parts of the wire behavior
described in [`DESIGN.md`](DESIGN.md) are guaranteed rather than incidental.

## Semantic versioning

`http_headers` is `0.y.z`. Cargo therefore treats every `0.(y+1).0` release as
potentially breaking, and this crate uses exactly that latitude: a minor
version bump may add, change, or remove public API, tighten or loosen a
parser's accepted grammar, or change the concrete type returned by an
existing method. A patch release (`0.y.(z+1)`) never changes public API and
never changes what wire input is accepted or what bytes are produced from a
given typed value — it is reserved for bug fixes, performance improvements,
and documentation.

Once this crate reaches `1.0.0` it will adopt standard semver: no breaking
change without a major version bump, and any acceptance-grammar change that
could reject a previously-accepted value or newly accept a previously-rejected
one will be treated as breaking regardless of whether the public API text
changed.

There is no commitment yet about when `1.0.0` ships. Track breaking changes
between `0.y` releases in the crate's release notes rather than assuming any
particular stability beyond what is stated above.

## Minimum supported Rust version (MSRV)

The workspace declares `rust-version = "1.95"` with the 2024 edition
(`Cargo.toml`). Raising the MSRV is treated as a breaking change under the
policy above: it happens only in a `0.(y+1).0` release, and the new minimum is
stated in that release's notes. A patch release never raises the MSRV.

## Feature flags

Declared in `crates/http_headers/Cargo.toml`:

| Feature | Default | Effect |
|---|---|---|
| No features (`default-features = false`) | — | Core field names, values, errors, `Field`/`SingleValueField`, and source/sink APIs. No built-in header families or optional dependencies are enabled by this crate. |
| `headers-all` | on | Enables all built-in header families listed below, including their optional dependencies. |
| `http` | off | Adds an optional `http::HeaderMap` adapter and conversions to and from the `http` crate's name, value, and method types. The adapter may retain `HeaderValue` storage internally; typed-header semantics do not change. |
| `serde` | off | Enables serialization and deserialization for `FieldName`, `FieldValue`, `EncodedValues`, and owned headers from enabled families (`dep:serde`). Does not enable header families itself. |
| `benchmarking` | off | Exposes private-backend instrumentation (`http_headers_simd/benchmarking`) needed by the workspace benchmarks. Not for downstream use; the surface it exposes is not covered by the semver policy above. |

To select individual families, disable default features and enable the required
`headers-*` features. Each family below is enabled by `headers-all`.
`BasicCredentials` belongs to `headers-authorization`, not the feature-free core.

| Family feature | Additional optional dependencies or family features |
|---|---|
| `headers-authorization` | `base64`, `zeroize` |
| `headers-cache-control` | `compact_str` |
| `headers-conditional` | `httpdate`, `headers-etag` |
| `headers-content-length` | None |
| `headers-content-type` | None |
| `headers-cors` | None |
| `headers-etag` | None |
| `headers-location` | `fluent-uri` |
| `headers-negotiation` | `idna` |
| `headers-range` | None |
| `headers-security` | `compact_str` |
| `headers-set-cookie` | None |
| `headers-user-agent` | None |
| `headers-websocket` | `base64`, `sha1` |

For example, this selects only the two named families and the HTTP adapter:

```toml
http_headers = { version = "0.1", default-features = false, features = ["headers-content-length", "headers-content-type", "http"] }
```

Cargo features are additive: another dependency enabling a family or
`headers-all` can enable it for the same package in the resolved build.

Enabling `benchmarking` is not a supported way to depend on this crate; the
items it exposes can change or disappear in a patch release. Ordinary header
decoding, encoding, and map operations never require it.

`http_headers_simd`, the sibling crate holding the SIMD scanning kernels, is
published only so Cargo can resolve `http_headers`. Its rustdoc API is hidden,
and it carries no independent compatibility guarantee. Downstream manifests
should depend on `http_headers`, never on `http_headers_simd` directly.

## Extension points

Every public trait in this crate except one is open to downstream
implementation, and none is sealed. Two extensions are supported: teaching the
crate about a new container, and teaching it about a new field.

| Trait | Implement it to | Required | Provided |
|---|---|---|---|
| `source::FieldSource` | read fields from your container | `lines` | `contains` |
| `sink::FieldSink` | write fields to your container | `set_values`, `append_values`, `remove_values` | `set_encoded`, `append_encoded` |
| `sink::FieldEncodeOutput` | let encoders write straight into your container's storage | `Writer`, `begin_value`, `push_value` | `push_u64` |
| `sink::FieldValueWriter` | receive one value's bytes for the above | `write_bytes`, `finish` | — |
| `sink::FieldEncoder` | define how a value becomes field lines | `encode` | — |
| `SingleValueField` | define a field carried by exactly one field line | `View`, `Owned`, `name`, `decode_view`, `decode_owned`, `as_field_value`, `into_field_value` | `decode_view_with`, `decode_owned_with` |
| `Field` | define a field that may span repeated field lines | `View`, `Owned`, `name`, `view_with`, `owned_with`, `insert` | `view`, `owned`, `remove` |

`sink::FieldSinkExt` is the exception. Its blanket implementation covers every
`FieldSink`, so no type in any crate can implement it; it exists to be called,
not implemented, and it is listed here only to state that implementing it is
not an available extension.

A blanket implementation also relates the last two rows: implementing
`SingleValueField` supplies `Field` automatically. Implement `Field` directly
only for a field whose value spans repeated field lines, and never implement
both for one type — the implementations would overlap.

Implementing `FieldEncodeOutput` and `FieldValueWriter` is optional. A
container that implements only `FieldSink` still accepts every typed field,
because the default `set_encoded` collects into `EncodedValues` first. The two
traits exist so a container can skip that intermediate collection; the bundled
`http::HeaderMap` adapter overrides `set_encoded` for exactly that reason, and
the traits are public so a third-party container can reach the same path.

The split between required and provided methods above is the part that matters
across releases: adding a required method to any of these traits breaks every
downstream implementation, while adding a provided one does not. New methods
will therefore always arrive with defaults. Under the pre-1.0 policy stated
above this remains a `0.(y+1).0`-only concern either way, but it is the
commitment that will carry into `1.0.0`.

Note that the reverse direction is not symmetric: because these traits are
unsealed, adding a *blanket* implementation of any of them in a later release
could conflict with a downstream implementation. No such blanket
implementation will be added except where one already exists.

## Decode modes

`DecodeMode::Strict` is the default used by `view`, `owned`, direct
`TryFrom` constructors, and Basic credential decoding.
For typed reads from a `FieldSource`, select `DecodeMode::Relaxed` through
`Field::view_with` or `Field::owned_with`. Direct single-value decoding accepts
the mode through `SingleValueField::decode_view_with` and
`SingleValueField::decode_owned_with`; standalone quality parsing accepts it
through `QualityView::parse`. Relaxed decoding is not a raw-value or
skip-validation mode.

Relaxed decoding recognizes the following interoperability deviations:

| Headers | Relaxation | Example |
|---|---|---|
| `Accept`, `Accept-Encoding`, `Accept-Language` | Spaces or tabs around the quality `=`, a fractional value without a leading zero, more than three fractional digits below one, or more than three zero digits after one | `gzip; q = .12345` |
| `ETag`, `If-Match`, `If-None-Match` | A lowercase weak-validator prefix | `w/"revision"` |
| `Content-Type` | Optional whitespace around the type/subtype slash | `text / html` |
| `Range`, `Content-Range` | Optional whitespace around range delimiters | `bytes = 0 - 499` |
| `Last-Modified`, `If-Modified-Since`, `If-Unmodified-Since`, `If-Range` | Outer whitespace, the `UTC` zone, or one-digit date/time components in an IMF-style date | ` Sun, 6 Nov 1994 8:49:37 UTC ` |
| `Host` | UTF-8 internationalized registered names that successfully convert through IDNA | `münich.example:443` |
| `Location` | Backslashes treated as slashes for URI-reference validation | `/a\b\c` |

These are acceptance exceptions, not a general recovery mode. Numeric bounds,
token and list structure, entity-tag contents, range ordering, valid IDNA and
ports, parameter ordering and count, quoting and escaping, list boundaries,
cardinality, framing, authorization, CORS, security, and WebSocket invariants
remain enforced. Relaxed quality values still reject signs, exponents,
multiple decimal points, quoted values, nonzero fractional digits after one,
and empty leading-dot fractions. A decoder never drops an invalid list member
to salvage the rest of a field. Headers absent from the table process
`DecodeMode::Relaxed` exactly as `DecodeMode::Strict`.

Accepted relaxed values retain their original bytes. Encoding therefore does
not silently canonicalize a relaxed value, and callers inspecting `items()` or
`values()` see the same nonconforming quality syntax that arrived on the wire.

## Sensitivity inputs

Sensitivity-setting APIs use `FieldSensitivity::{Sensitive, NonSensitive}`
rather than Boolean arguments. This applies to `FieldValue`,
`FieldValueRef`, `ValueRefsEncoder`, and `FieldEncodeOutput::begin_value`.
The enum's supported path is `http_headers::FieldSensitivity`, alongside the
crate-root field value types that consume it. Code written against the earlier
pre-1.0 draft should replace `set_sensitive(bool)` and `with_sensitive(bool)`
with `set_sensitivity(FieldSensitivity)` and
`with_sensitivity(FieldSensitivity)`; custom encode outputs receive the same
enum directly. The duplicate `http_headers::sink::FieldSensitivity` alias was
removed before 1.0 so the type has one documented public path.

## Wire guarantees

The behaviors [`DESIGN.md`](DESIGN.md#protocol-behavior) documents per header
family are part of the semver-covered contract above: a patch release will
not change which bytes a constructor accepts, which bytes a decoder accepts
or rejects, or which bytes an encoder produces for a value that already
round-trips today. Two guarantees apply across every header type:

- **Byte-valued, not string-valued.** Field values that are not guaranteed
  UTF-8 by their grammar are exposed as `&[u8]`; an explicit `*_str` accessor
  performs the fallible UTF-8 conversion. A decoder never panics on bytes a
  `FieldValue` can hold, including `obs-text` (`0x80..=0xff`).
- **Field-line boundaries are preserved.** A header delivered as several
  repeated field lines is never silently flattened into one comma-joined
  value before the caller sees it; `FieldLines::repeated()` and the borrowed
  iterators reconstruct the same boundaries that were parsed.

A change to either of the two guarantees above, or to any bullet in
`DESIGN.md`'s protocol-behavior list, is a breaking change under the semver
policy stated above regardless of which release channel introduces it.
