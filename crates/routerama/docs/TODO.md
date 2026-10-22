# TODO

The forward-looking backlog for the `routerama`, `routerama_build`, and
`routerama_macros` crates: what is still worth doing. Completed work is deleted
from this file rather than recorded, and so are decisions taken against
something — the crate's rustdoc and [`../AGENTS.md`](../AGENTS.md) hold the
adopted design and conventions, [`PERF.md`](PERF.md) the customer-facing
measured results, and [`PERF-INTERNAL.md`](PERF-INTERNAL.md) the internal ones. The **Priority** field on each item
carries the ranking; the file's length is a measure of outstanding work.

## Contents

### Performance

- [P1](#p1) — Decode form bodies and query strings across `BytesView` spans

### Competitive feature gaps

- [CFG1](#cfg1) — Optional Serde migration adapter for the query codecs
- [CFG2](#cfg2) — Optional aggregate path extractor

### Features

- [F1](#f1) — Build the three missing starter-catalog extractors
- [F2](#f2) — Decide whether mounted services can run response interceptors
- [F3](#f3) — Decide whether the generated per-service body sum becomes an exported type
- [F4](#f4) — Extend the direct query codecs
- [F5](#f5) — Binary-layout response templates
- [F6](#f6) — Span-aware JSON decoding without `bytesbuf-std`

### Benchmarks

- [B1](#b1) — Collect hardware-counter measurements
- [B2](#b2) — Per-framework minimal binaries for size and compile-time claims

## Performance

<a id="p1"></a>
### P1 — Decode form bodies and query strings across `BytesView` spans

**Area:** `routerama` · **Priority:** Low · **Effort:** Medium

Neither `route::form` nor the `query` codecs accept a `BytesView`, so a
fragmented form body must be flattened before it can be decoded, reintroducing
the copy the rest of the `bytesbuf` integration removes. Always intended as a
later optimization; worth doing only once a workload shows fragmented form
bodies are common.

**Done when:** form and query decoding accept a `BytesView` and decode across
span boundaries without an intermediate contiguous buffer, with a benchmark
showing the removed copy.

**See also:** F6 (the same gap for JSON)

---

## Competitive feature gaps

<a id="cfg1"></a>
### CFG1 — Optional Serde migration adapter for the query codecs

**Area:** `routerama`, `routerama_build` · **Priority:** High · **Effort:** Medium

Axum reaches for `serde_urlencoded` and Actix Web for `serde_qs` on this exact
surface, so an application migrating to Routerama has to rewrite every query type
against the direct `FromQuery`/`ToQuery` codecs. That makes the absence the most
likely migration blocker in the query area.

**Done when:** an optional, feature-gated adapter lets a type that already
implements `Deserialize`/`Serialize` be used where `FromQuery`/`ToQuery` is
expected, without the direct codecs losing their zero-allocation path.

**See also:** F4 (other query codec extensions)

---

<a id="cfg2"></a>
### CFG2 — Optional aggregate path extractor

**Area:** `routerama` · **Priority:** Medium · **Effort:** Medium

Captures are delivered as direct, compile-time-checked handler arguments, which
is the right default and is what makes the missing-capture and wrong-name
diagnostics possible. But a handler that wants many captures as one nested value
has no way to ask for it, whereas Axum's `Path<T>` deserializes a whole capture
set into one struct. The design explicitly did not want to force direct arguments
where aggregate extraction reads better.

**Done when:** an aggregate extractor exists that still checks capture names and
arity at compile time, rather than trading that away for a `Deserialize` round
trip.

---

## Features

<a id="f1"></a>
### F1 — Build the three missing starter-catalog extractors

**Area:** `routerama` · **Priority:** High · **Effort:** Medium

Three extractors named as targets while the crate was being built were never
reached: a complete `Request<B>` view, a raw query-text extractor, and typed and
raw path-capture views. Each is expressible today by combining `&Parts` with
`RawBody<B>`, by reading `uri.query()`, or by taking the capture arguments
directly, so this is ergonomics rather than a capability hole — but the catalog
is incomplete against what was advertised, which makes it the most visible gap to
a new user.

**Done when:** all three exist as first-class extractors with doc tests, and the
starter-catalog documentation lists no extractor that does not exist.

---

<a id="f2"></a>
### F2 — Decide whether mounted services can run response interceptors

**Area:** `routerama` · **Priority:** Medium · **Effort:** Large

A mounted service cannot run response interceptors today because
`ErasedMountRouter::route` moves the request head into the mounted service while
`AfterContext` borrows it. Giving mounts a borrowed head would remove
`MountedRequest::into_request`, which exists precisely to transfer ownership. The
boundary is documented wherever `#[after]` is described, and the attribute is
named for the responses it does observe — but it is a real capability boundary
users will hit.

- `crates/routerama/src/route/mount.rs:312` — `route` takes
  `http::Request<B>` by value

**Done when:** either mounts can run `#[after]` interceptors without giving up
ownership transfer, or the boundary is recorded as permanent in the crate's
rustdoc and this item is deleted.

---

<a id="f3"></a>
### F3 — Decide whether the generated per-service body sum becomes an exported type

**Area:** `routerama_build` · **Priority:** Low · **Effort:** Large

The current representation was chosen deliberately: the generated response-body
and error sums stay private, and the opt-in `#[router(..., tower)]` constructor
returns an opaque exact Tower service whose signature names no handler,
rejection, body, or body-error type. A reversible exported-sum prototype reached
zero allocations and 839 instructions, but private body fixtures then failed with
`E0603` and visibility warnings and it was fully reverted: public enum variants
and the `Body::Error` associated type expose the private types, and a second
public wrapper would create permanent generated body and error API names solely
to let a transport infer types. The adopted adapter allocates zero times, runs
the same 839 instructions with the same 832/152/40-byte future/response/body
layout as `ExactBody`, uses 115 fewer instructions than `SendBoxBody`, and costs
0.20% warm compile time and 0.056% `.text` and `.rodata` over `ExactBody`.

The comparison that would justify changing this — a fixture that builds an
exported named sum — has not been built, because it would measure a design this
repository has not adopted. It is a prerequisite of *changing* the decision.

**Done when:** either a fixture measures an exported named sum and the decision is
revisited on that evidence, or the item is deleted as settled.

---

<a id="f4"></a>
### F4 — Extend the direct query codecs

**Area:** `routerama`, `routerama_build` · **Priority:** Low · **Effort:** Large

Contemplated extensions to the direct `FromQuery` and `ToQuery` codecs with no
caller asking for them yet: custom per-field codecs, type and const generic
schemas, configurable scalar-duplicate and key-only policies, and compile-time
collision diagnostics across flattened schemas.

**Done when:** each extension either ships in response to a real caller, or is
struck from this item. Do not build the set speculatively.

**See also:** CFG1 (the Serde adapter, which is driven by a real gap)

---

<a id="f5"></a>
### F5 — Binary-layout response templates

**Area:** `routerama` · **Priority:** Low · **Effort:** Medium

Listed as a template target and never built: fixed-width byte fields and typed
integer encoding rendered through the same prepared-fragment machinery, so a
binary protocol response could be assembled without a formatting pass. No caller
in this repository needs one yet, so it stays a capability note rather than a
commitment.

**Done when:** a caller needs one and it ships through the existing
prepared-fragment machinery — or this item is deleted for lack of demand.

---

<a id="f6"></a>
### F6 — Span-aware JSON decoding without `bytesbuf-std`

**Area:** `routerama` · **Priority:** Medium · **Effort:** Large

`JsonView`'s `FromRequestBody` implementation is gated on
`all(feature = "json", feature = "bytesbuf-std")` and feeds the collected view to
`serde_json::from_reader`, which needs `std::io::Read`. A `no_std` target
therefore has no span-aware JSON path at all, and even with `std` a fragmented
body is read through the reader adapter rather than decoded across spans
directly.

- `crates/routerama/src/route/bytesbuf.rs:133` — the `bytesbuf-std` feature gate
- `crates/routerama/src/route/bytesbuf.rs:169` — the `serde_json::from_reader` call

**Done when:** a decoder pulls across span boundaries without an intermediate
contiguous buffer and the `no_std` target has a span-aware JSON path. This is a
separate investigation, not a small change.

**See also:** P1 (the same gap for form and query decoding)

---

## Benchmarks

The three benchmark families and the naming that distinguishes them are
documented in [`benches/README.md`](../benches/README.md). The
instruction-count suite covers every major functional area, and the audience
split between [`PERF.md`](PERF.md) and [`PERF-INTERNAL.md`](PERF-INTERNAL.md) is
in place, so what remains are the two measurement gaps neither side covers.

The split rule: **instruction-count benchmarks (Callgrind/gungraun) are ours** —
they find and prevent regressions in our own code, so they cover every major
functional area exhaustively and do not appear in a customer-facing document.
**Differential scenario benchmarks (Criterion) are the customer's** — they answer
"how does Routerama compare to the alternative I would otherwise pick, on a
workload I recognize", so they are scenario-oriented and always comparative.

See [`docs/callgrind-benchmarks.md`](../../../docs/callgrind-benchmarks.md) for
the file template and the Criterion-pairing convention, and
[`docs/naming.md`](../../../docs/naming.md) for naming.

<a id="b1"></a>
### B1 — Collect hardware-counter measurements

**Area:** `routerama` benches · **Priority:** Medium · **Effort:** Medium

All checked-in measurement is Criterion wall time plus Callgrind instruction
counts; branches, branch misses, and cache behaviour were asked for and never
collected. This is not merely for completeness: generated code shows a 9%
instructions against 29% wall-time divergence between 16 and 1,024 routes that
instruction counting alone cannot explain, and an instruction-cache counter is
the obvious next probe.

**Done when:** hardware counters are collected for the generated-code scenarios,
published to [`PERF-INTERNAL.md`](PERF-INTERNAL.md), and the 16-to-1,024-route
divergence is either explained or shown not to be cache-driven.

---

<a id="b2"></a>
### B2 — Per-framework minimal binaries for size and compile-time claims

**Area:** `routerama` benches · **Priority:** Medium · **Effort:** Medium

Binary-size and compile-time conclusions need per-framework minimal application
binaries. What exists measures Routerama alone, or measures one shared benchmark
binary that also links Axum, Actix Web, Rocket, Warp, Tokio, and Serde — a
fixture-wide engineering cost, not a per-framework application measurement. Until
the minimal binaries exist, no size or compile-time comparison may be stated.

**Done when:** a minimal application binary exists per framework and
[`PERF.md`](PERF.md) states size and compile-time comparisons derived from them.
The per-framework binaries are a comparative claim and so belong in `PERF.md`;
the existing single-representation size controls stay in
[`PERF-INTERNAL.md`](PERF-INTERNAL.md).

