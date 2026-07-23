# TODO

This file lists work that is genuinely outstanding. Everything the plan's
phases 0 through 6 required is implemented, including every performance
workload the plan asked for; see [`../PLAN.md`](../PLAN.md) — its **Final
status** section, the per-phase findings, and the validation-matrix coverage
table — and [`PERF.md`](PERF.md) for the measured results and the
acceptance-gate verdicts. Nothing below is a missing
measurement: what remains is design decisions, deliberate exclusions, and
future capability work.

## Open design questions

- Decide whether a mounted service should be able to run response
  interceptors. It cannot today because `ErasedMountRouter::route` moves the
  request head into the mounted service and `AfterContext` borrows it; giving
  mounts a borrowed head would remove `MountedRequest::into_request`, which
  exists precisely to transfer ownership. The current boundary is documented
  wherever `#[after]` is described, and the attribute is named for the
  responses it does observe.
- Decide whether a `tower_layer::Layer` implementation is worth adding.
  Nothing in the current design needs one: a router is a leaf service, not a
  wrapper.
- Decide whether erased mounts should gain a `Send` boundary. A
  `SendErasedMountService` (with `SendBoxBody` instead of `BoxBody`) would let a
  mounted router reach a `Send` transport without the local boxing boundary. The
  local contract must stay unchanged.
- Decide whether the generated response-body error sum should forward
  `source()` to its active variant. It implements `Error` with the default
  `source()` today, so after erasure the concrete body error's type is not
  recoverable through a generated router, and the sum's `Display` names the
  failing response rather than repeating the handler's message. The
  `streaming_responses` example asserts that behavior so a change to it is
  visible.
- Decide whether the generated per-service body sum should become an exported
  named type. `PLAN.md` open decision 2 keeps the current representation and
  lists the measurements that support it. The comparison that would justify
  changing it — a fixture that builds an exported named sum — has not been
  built, because it would measure a design this repository has not adopted. It
  is a prerequisite of *changing* the decision, not a workload the plan asked
  for.

## Deliberate exclusions

Cookies, multipart, typed headers, and WebSockets are not implemented and are
not planned by this document. Each would add a parsing surface and a
dependency question that the plan never required, and a handler can already
read the raw header or body it needs. Plan open decision 6 records the same
boundary.

A loopback-transport throughput benchmark is deliberately excluded, and its
absence is not an outstanding measurement. Transport equality across the five
comparison subjects is not controllable: Routerama ships no server, so one
side of the comparison would be hyper glue written by this repository; Actix
Web serves on `actix-rt` workers and Rocket drives its own runtime, so a
single Tokio runtime cannot host all five, especially with Routerama's and
Actix Web's `!Send` futures; and the transports differ structurally enough
that the result would rank HTTP/1 parsers rather than routers. The throughput
gate was therefore rescoped, in the open and with that justification, to
concurrent *in-process* CPU-bound throughput, which
`benches/routerama_throughput.rs` measures and [`PERF.md`](PERF.md) reports.
Nothing in this repository may describe those numbers as transport-level.

The five-framework form fixture deliberately has no invalid-UTF-8 scenario.
Routerama rejects a non-UTF-8 encoded form with 400, while `form_urlencoded`
(Axum, Actix Web, Warp) and Rocket's `url_decode_lossy` substitute U+FFFD and
succeed. Reconciling that would mean replacing each framework's own decoder,
which would stop measuring the framework. The JSON fixture keeps its
`invalid_utf8` row because bytes/text extraction is genuinely comparable there.

`routerama_build`'s own test targets do not compile under every partial
feature selection (`tests/macro_impl.rs` and one `RoutePredicates` unit test
assume `query` and `route`). The library itself checks clean across the whole
feature powerset; only that crate's dev targets are affected.

## Query codec follow-ups

The direct `FromQuery` and `ToQuery` codecs are implemented. Potential extensions
are an optional Serde migration adapter, custom per-field codecs, type and const
generic schemas, configurable scalar-duplicate and key-only policies, and
compile-time collision diagnostics across flattened schemas.

## Path canonicalization

Add an explicit path-preparation API that runs before route matching. A
`PreparedPath` should borrow unchanged input and own rewritten input, expose
whether canonicalization changed the path so callers can redirect, and keep the
prepared value alive while resolved routes borrow from it.

Provide policies for preserving, rejecting, or normalizing repeated slashes,
dot segments, and trailing slashes, plus preserving or rejecting encoded
separators. Query or fragment delimiters and malformed percent escapes should be
rejected. Percent-encoded `/`, `\`, `.`, and `..` must never become routing
structure through decoding. Include exact and strict presets, with core route
matching remaining exact and normalization explicitly selected by the caller.

An initial API shape could be:

```rust
pub mod path {
    use alloc::borrow::Cow;

    pub struct PreparedPath<'p> {
        value: Cow<'p, str>,
        changed: bool,
    }

    impl<'p> PreparedPath<'p> {
        pub fn new(path: &'p str, policy: PathPolicy) -> Result<Self, Error>;
        pub fn as_str(&self) -> &str;
        pub const fn was_changed(&self) -> bool;
    }

    pub struct PathPolicy {
        pub repeated_slashes: RepeatedSlashes,
        pub dot_segments: DotSegments,
        pub trailing_slash: TrailingSlash,
        pub encoded_separators: EncodedSeparators,
    }

    impl PathPolicy {
        pub const EXACT: Self;
        pub const STRICT: Self;
    }

    pub enum RepeatedSlashes {
        Preserve,
        Reject,
        Collapse,
    }

    pub enum DotSegments {
        Preserve,
        Reject,
        Remove,
    }

    pub enum TrailingSlash {
        Preserve,
        Reject,
        Remove,
    }

    pub enum EncodedSeparators {
        Preserve,
        Reject,
    }
}
```

The caller would retain ownership of the prepared path while using any route
that borrows from it:

```rust
let prepared = routerama::path::PreparedPath::new(
    uri_path,
    routerama::path::PathPolicy::STRICT,
)?;

if prepared.was_changed() {
    return redirect_permanently(prepared.as_str());
}

let route = resolver.resolve(method, prepared.as_str())?;
```
