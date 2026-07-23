# TODO

This file lists work that is genuinely outstanding. It is forward-looking:
completed work is removed rather than recorded here, and
[`PERF.md`](PERF.md) holds the measured results, the validation coverage, and
the acceptance-gate verdicts.

What remains below is design decisions still open, deliberate exclusions,
performance work, and future capability gaps.

## Open design questions

- Decide whether a mounted service should be able to run response
  interceptors. It cannot today because `ErasedMountRouter::route` moves the
  request head into the mounted service and `AfterContext` borrows it; giving
  mounts a borrowed head would remove `MountedRequest::into_request`, which
  exists precisely to transfer ownership. The current boundary is documented
  wherever `#[after]` is described, and the attribute is named for the
  responses it does observe.
- Decide whether the generated per-service body sum should become an exported
  named type. The current representation was chosen deliberately: the
  generated response-body and error sums stay private, and the opt-in
  `#[router(..., tower)]` constructor returns an opaque exact Tower service
  whose signature names no handler, rejection, body, or body-error type —
  those predicates live only on a hidden generated runtime contract. A
  reversible exported-sum prototype also reached zero allocations and 839
  instructions, but private body fixtures then failed with `E0603` and
  visibility warnings, so it was fully reverted; public enum variants and the
  `Body::Error` associated type expose the private types, and a second public
  wrapper would create permanent generated body and error API names solely to
  let a transport infer types. The adopted adapter allocates zero times, runs
  the same 839 instructions with the same 832/152/40-byte
  future/response/body layout as `ExactBody`, uses 115 fewer instructions than
  `SendBoxBody`, and costs 0.20% warm compile time and 0.056% `.text` and
  `.rodata` over `ExactBody`. The comparison that would justify changing this
  — a fixture that builds an exported named sum — has not been built, because
  it would measure a design this repository has not adopted. It is a
  prerequisite of *changing* the decision, not outstanding work.
- Decide whether a service-level opt-in body-limit policy is worth adding.
  Every bounded body extractor states its limit as a const generic on the
  handler parameter (`Json<T, 4_096>`, `BytesBody<64>`), and there is
  deliberately no default limit and no ambient service policy, because either
  would let an unbounded buffer appear without a source change; an application
  keeps one number in one place with a type alias. Whether an opt-in policy on
  top of that alias earns its keep needs production feedback, not another
  prototype.

## Deliberate exclusions

There is no `tower_layer::Layer` implementation. A router is a leaf service,
not a wrapper: it terminates a request by producing a response and has no
inner service to delegate to. A `Layer` would therefore have nothing to wrap.
Applications compose middleware *around* a generated router through the
existing `tower_service::Service` adapters. Revisit only if routing ever gains
a pass-through mode.

Request-parts extraction is synchronous and stays that way.
`FromRequestParts` takes `&'request Parts` and returns `Result` directly
(`src/route/extract.rs:39`), so no extractor future, allocation, box, or
`Send` bound joins the generated path and no borrow can escape dispatch. The
consequences are accepted deliberately: an extractor cannot do I/O, and a
response cannot borrow request metadata. Mutable parts access and async
authentication were the two motivating cases and both are already served by
`#[before]` interceptors, so an async variant would add a per-request future
to the hot path to serve no case that is not already served.

The matching engine stays private and unsplit. It is currently divided
between `routerama`'s private runtime modules (`src/walk.rs`, `src/rt_node.rs`,
`src/literal_edge.rs`, `src/affix_edge.rs`, `src/raw_resolver.rs`,
`src/route_match.rs`) and `routerama_build`'s framework-neutral trie,
reachable only through `route::__private`. A split must improve the
user-facing dependency graph or open a genuine reuse boundary; moving private
implementation files into another package achieves neither, and publishing
them would freeze internal representations that the performance work above
still expects to change. The same test applies to promoting `route::extract`:
only if a custom transport can use it without the router macro. Revisit when
an outside consumer actually exists.

The generated response-body error sum requires every handler response body
error to implement `core::error::Error + 'static`, and forwards `source()` to
its active variant. The alternative — the unbounded blanket `Error` impl the
sum used to carry — accepted body errors that were not themselves errors, but
made the concrete error permanently unrecoverable after erasure, since the sum
is private and `dyn Error` is its only access path. The bound normalizes an
inconsistency rather than adding a restriction: `BoxBody::new`,
`SendBoxBody::new`, both Tower boundaries, `ErasedMountService::new`, and the
crate's own hand-written `EitherBodyError` all already demand exactly it, so a
non-`Error` body error could only ever be used through `ExactBody`. The sum's
`Display` still names the failing response rather than repeating the handler's
message, which is the correct division of labour once the cause is reachable:
`Display` describes this layer, `source()` yields the layer below.

Cookies, multipart, typed headers, and WebSockets are not implemented and are
not planned by this document. Each would add a parsing surface and a
dependency question that this crate does not own, and a handler can already
read the raw header or body it needs. The extraction boundary is instead
drawn at explicit raw, bounded-bytes, and bounded-text extraction in `route`,
with JSON as an additive feature that implies `route`, and bounded forms as
an additive feature that implies `route` and `query`, lives at `route::form`,
and accepts only output types satisfying the owned higher-ranked `FromQuery`
contract.

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

There is no low-level `try_route` API. Generated `route` always returns a
response, and a programmatic routing-error API has not demonstrated enough
value to add, because every generated associated method enlarges the symbol
surface a user sees. Revisit only with a concrete use case.

There is no default body limit and no unbounded body variant. An early draft
of the design asked for a conservative default; that was overridden on
purpose, because a default limit lets an unbounded buffer appear without a
source change, and an unbounded variant is a denial-of-service switch with no
compensating ergonomics.

Response bodies are restricted to `Data = bytes::Bytes`. Generating a second
data-buffer sum would add a per-frame representation and transport questions
that nothing here has measured. A `compile_fail` doctest pins the
restriction so it cannot be relaxed accidentally.

`#[capture]` is the only dynamic-capture marker. A configured dynamic route
has no compile-time template, so the marker is what lets the macro tell a
capture from a request-parts extractor, name the generated `add_<handler>`
argument checks, and reject a borrowed capture or a name the registered
template does not contain; those diagnostics are asserted in
`routerama_build`'s expansion tests. Revisit only if a second dynamic
declaration form appears.

## Query codec follow-ups

Potential extensions to the direct `FromQuery` and `ToQuery` codecs: an
optional Serde migration adapter, custom per-field codecs, type and const
generic schemas, configurable scalar-duplicate and key-only policies, and
compile-time collision diagnostics across flattened schemas.

## bytesbuf integration follow-ups

`BytesView` is already a first-class body and data type on both the response
and extraction sides. What follows is what that integration did not attempt.

Span-aware JSON decoding is only available with `bytesbuf-std`.
`JsonView`'s `FromRequestBody` implementation is gated on
`all(feature = "json", feature = "bytesbuf-std")` and feeds the collected
view to `serde_json::from_reader`, which needs `std::io::Read`
(`src/route/bytesbuf.rs:153`). A `no_std` target therefore has no span-aware
JSON path at all, and even with `std` a fragmented body is read through the
reader adapter rather than decoded across spans directly. Closing this needs
a decoder that can pull across span boundaries without an intermediate
contiguous buffer; it is a separate investigation, not a small change.

Form and query decoding have no span-aware path. Neither `route::form` nor
the `query` codecs accept a `BytesView`, so a fragmented form body must be
flattened before it can be decoded, which reintroduces the copy the rest of
the integration removes. This was always intended as a later optimization,
and it is worth doing only once a workload shows fragmented form bodies are
common.

`BytesViewTemplate` has no compile-time escaping enforcement. The `Bytes`
template surface exposes the `json_body_template!` and `html_body_template!`
macros, which decide at expansion time which escaping each slot gets and offer
no verbatim slot at all. The `BytesView` surface is the manual
`prepare`/`render` pair, so the choice is made per call rather than per
template: it offers the same escaping slots (`json_number`, `json_string`,
`html_text`) and names the verbatim one `unescaped_text` so that reaching for
it is explicit, but nothing stops a caller using it inside a JSON or HTML
fragment. A macro over `BytesViewTemplate` that enforces the choice per
template would close the gap.

Binary-layout templates were listed as a template target and never built.
The idea was fixed-width byte fields and typed integer encoding rendered
through the same prepared-fragment machinery, so a binary protocol response
could be assembled without a formatting pass. No caller in this repository
needs one yet, so it stays a capability note rather than a commitment.

Handing a contiguous `BytesView` to `HeaderValue::from_maybe_shared` to
avoid a copy on the header side was explicitly ruled out of scope for the
integration, on the grounds that header values are small and the
sharing check would cost more than the copy it avoids. Revisit only with a
measurement showing header construction on a hot path.

## Capability gaps carried over from the implementation plan

These were named as targets while the crate was being built and were never
reached. None is a defect; each is promised or contemplated surface that does
not exist.

Add JSON and form *response* wrappers. The `json` and `form` features are
extraction-only: `src/route/json.rs` and `src/route/form.rs` implement
`IntoResponse` for their rejection types and nothing else, so a handler that
wants to answer with JSON serializes by hand and sets `Content-Type` itself —
`examples/json_api.rs:41` does exactly that with
`serde_json::to_string(body).expect(...)`. This is the one place where the
request and response sides of a feature are not paired. A wrapper must decide
what happens when serialization fails, which is why it was deferred rather
than dropped in as an afterthought.

Consider an optional aggregate path extractor. Captures are delivered as
direct, compile-time-checked handler arguments, which is the right default and
is what makes the missing-capture and wrong-name diagnostics possible. But a
handler that wants many captures as one nested value has no way to ask for it,
and the design explicitly did not want to force direct arguments where
aggregate extraction reads better. Any such extractor has to keep the
compile-time checking rather than trade it away for a `Deserialize` round trip.

Three extractors from the intended starter catalog were never built: a
complete `Request<B>` view, a raw query-text extractor, and typed and raw
path-capture views. Each is expressible today by combining `&Parts` with
`RawBody<B>`, by reading `uri.query()`, or by taking the capture arguments
directly, so this is ergonomics rather than a capability hole — but the
catalog is incomplete against what was advertised.

## Performance work outstanding

- Move the hand-rolled SIMD to `core::simd` once portable SIMD stabilizes.
  `src/codegen_helpers/scan.rs` and `src/query/scan.rs` carry parallel SSE2 and
  NEON implementations behind `cfg(target_arch)`, which is eight of the
  crate's twenty-three `unsafe` blocks. Rewriting both with `Simd<u8, 16>`,
  `simd_eq`, and `to_bitmask` expresses the same scan in entirely safe code,
  and on a baseline `x86-64` target it selects the same instructions the
  intrinsics do (`movdqu`, `pcmpeqb`, `por`, `pmovmskb`), so the migration is
  a pure deletion of `unsafe` with no codegen change and one implementation
  instead of three. It is blocked only on stabilization: `portable_simd` is
  still nightly-only, and the workspace targets stable with an MSRV of
  1.93.1. The usual objection — that portable SIMD lowers to whatever target
  features are enabled at compile time and so cannot dispatch at run time —
  does not apply here, because these scanners deliberately use only the
  architecture baseline and never dispatch.

  Two alternatives were considered and rejected. `#[target_feature(enable =
  "sse2")]` on the scan functions does make the *compute* intrinsics safe to
  call today, but the loads stay `unsafe` and the annotated function itself
  becomes unsafe to call, so the `unsafe` moves to a boundary rather than
  disappearing. Depending on `memchr` would relocate the `unsafe` into another
  crate rather than remove it, add a dependency, lose runtime dispatch under
  `no_std`, and not fit the workload, since the path scan is a single fused
  pass that locates separators while rejecting `?` and `#` and recording
  segment bounds.

- Index or prefilter affix edges in `src/walk.rs`. Literal edges get a
  fanout-aware binary search, but affix edges are still matched by a linear
  scan, so a request reaching such a node costs O(routes × suffix length).
  Measured per resolve on a 600-byte segment with exactly one matching edge:
  0.178 µs at width 1, 1.59 µs at 128, 14.1 µs at 1024, and 100.6 µs at 8192,
  against 0.066 µs to 0.235 µs for literals over the same range; the worst
  case observed, 4096 affix routes against a 4 KB path with every subtree
  failing, was 805 µs. The work is bounded — the trie is a tree, each node is
  visited at most once, and there is no recursion — so this is a throughput
  ceiling, not a hang, which is why it was left for an explicit measurement
  to justify rather than driving an architectural change to `RtNode`.

## Measurement work carried over from the implementation plan

`docs/PERF.md` holds every measurement that exists and states its own
boundaries. The following are gaps that were opened and never closed; they are
recorded here so nobody has to re-derive them from the benchmark tree.

Scenario groups 8 through 12 of the comparative evaluation have no
five-framework fixtures. Guards and extractors at zero, one, and four;
interceptors at zero, one, and four; fixed versus streaming versus SSE bodies;
host and media predicates with fallback recovery and required-state checks;
and a body-transform interceptor feeding a handler `#[body]` all exist only as
Routerama-against-Routerama controls (`benches/routerama_interceptors.rs`,
`routerama_route_policy.rs`, `routerama_response_body.rs`, and
`routerama_tower.rs`; none of them mentions another framework). `PERF.md` says
plainly that such controls prove nothing about other frameworks, so these
scenarios currently carry no comparative claim at all.

Binary-size and compile-time conclusions need per-framework minimal
application binaries. What exists measures Routerama alone, or measures one
shared benchmark binary that also links Axum, Actix Web, Rocket, Warp, Tokio,
and Serde — a fixture-wide engineering cost, not a per-framework application
measurement. `PERF.md:1125` already warns that those numbers must not be used
to rank frameworks. Until the minimal binaries exist, no size or compile-time
comparison may be stated.

No hardware-counter measurement exists. All checked-in measurement is
Criterion wall time plus Callgrind instruction counts; branches, branch
misses, and cache behaviour were asked for and never collected. This is not
merely for completeness: generated code shows a 9%-instructions against
29%-wall-time divergence between 16 and 1,024 routes that instruction
counting alone cannot explain, and an instruction-cache counter is the
obvious next probe.

Response-body representation scaling stops at 16 distinct body sources. The
shipped fixtures (`benches/routerama_response_body_variants_{1,4,16}.rs`) hold
the route count at 16 and vary only the number of distinct handler body types,
which is the better-isolated design and should be kept — but the 128- and
1,024-source sets that would show where deduplication, per-source variants,
and boxed normalization diverge were never built.

`PERFORMANCE-REPORT.md` is cited but does not exist. `docs/PERF.md:407` and
`docs/PERF.md:1213` both point at `../PERFORMANCE-REPORT.md` for the final
retained and rejected disposition of the performance-integration candidates —
the shared static/dynamic path scan, parse-once overlap predicates, primitive
decoding, exact query hints, typed response templates, literal-chain fusion,
and generated route-header plans. The file is absent from the crate and from
the repository's history, so that disposition is recorded nowhere and both
links are broken. Either write the file or rewrite the two sentences.

