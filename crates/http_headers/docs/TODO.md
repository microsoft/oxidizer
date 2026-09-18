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

### Benchmarks

- [B1](#b1) — Cover semantic reads separately from decode-and-drop
- [B2](#b2) — Measure source representations and ownership boundaries
- [B3](#b3) — Cover nonliteral grammar and fallback distributions
- [B4](#b4) — Gate parser instruction and allocation regressions
- [B5](#b5) — Measure downstream release code generation and layouts
- [B6](#b6) — Establish workload shares and benchmark repeatability

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

## Benchmarks

<a id="b1"></a>
### B1 — Cover semantic reads separately from decode-and-drop

**Area:** per-header metabench coverage · **Priority:** High · **Effort:** Medium

**Purpose:** maintain and improve. Several per-header rows only decode and
drop, so they cannot price repeated semantic reads.
Measure both `Field::view` and `Field::owned`, then separately consume semantic
outputs: weighted-list items, Content-Type parameters/lookups, Referrer
preference, CORS counts/wildcards, conditional tags, Range specs and WebSocket
extensions including parameters. Include cheap cached scalar/date getters as
controls. Use 1/2/8 reads and short versus multi-member inputs; do not infer
traffic weights from fixtures.

- `crates/http_headers/benches/http_headers_operations.rs:44` — generic owned operation ends in `consume(header)`
- `crates/http_headers/benches/http_headers_operations.rs:52` — borrowed operation does the same
- `crates/http_headers/benches/http_headers_per_header.rs:221` — negotiation registrations use these operations
- `crates/http_headers/benches/http_headers_micro.rs:663` — existing Content-Type semantic cases are a starting point, not complete coverage

**Done when:** cases report wall time, instructions and allocations separately
for decode-only and decode-plus-read, consume actual semantic values, and are
wired through B4 to fail on unexpected allocation increases or any instruction
increase against per-case baselines.

---

<a id="b2"></a>
### B2 — Measure source representations and ownership boundaries

**Area:** source acquisition and raw conversion benchmarks · **Priority:** High · **Effort:** Medium

**Purpose:** maintain and improve. Current per-header setup uses HTTP storage,
which is exempt from custom budgets and can share owned data. Add equivalent
`Single`, `Borrowed`, stored-`FieldValue` slice and HTTP cases. Cover 1/2/3/4/5/
16/128 lines, 63/64/65-byte values, totals around 64 KiB and 1,024/1,025 items.
Measure canonical/noncanonical Accept-Ranges and repeated collectors explicitly.
Separately compare CORS `TryFrom<FieldValue>`, `FromStr`, vector-taking conversion
and source `owned` without counting caller setup as parser allocation.

- `crates/http_headers/benches/http_headers_per_header.rs:32` — `fn map(...) -> HeaderMap`
- `crates/http_headers/src/source/field_lines.rs:273` — representation-aware singleton acquisition
- `crates/http_headers/src/source/field_lines.rs:352` — repeated acquisition
- `crates/http_headers/src/headers/range/accept_ranges.rs:399` — ownership probes

**Done when:** instruction, wall-time, allocation-count and allocated-byte
baselines cover validation and ownership transitions and preserve
sharing/singleton properties; expected limit errors remain distinct from
HTTP-exempt outcomes. Maintain cases fail through B4 on allocation increases
or any instruction increase. Include unknown size hints rather than testing
only exact slices.

---

<a id="b3"></a>
### B3 — Cover nonliteral grammar and fallback distributions

**Area:** parser success/fallback corpus · **Priority:** High · **Effort:** Large

**Purpose:** maintain and improve. Add a stratified corpus rather than only
whole-line literals that coincide with fast paths. Measure wall time,
instructions and allocations, reporting each stratum without inventing
production frequencies. This supplies evidence for fallback paths and input
shapes that the common-case fixtures do not exercise.

- `crates/http_headers/benches/http_headers_per_header.rs:331` — fixed Content-Length example `"348"`
- `crates/http_headers/benches/http_headers_per_header.rs:355` — Host is `example.com:8443`
- `crates/http_headers/benches/http_headers_micro.rs:69` — one three-parameter Content-Type fixture
- `crates/http_headers_simd/benches/http_headers_simd_no_std_dispatch.rs:51` — existing crossover-length scanner cases

Include numeric 19/20-digit and leading-zero inputs, equal/conflicting
duplicates, long/obs-text tags, all date formats, range unit/casing/order forms,
and errors competing with malformed delimiters. Cover Content-Type common and
nonliteral forms, 0/1/2/3/16/49 parameters, first/last/missing/duplicate lookups
and compact-offset boundaries. Exercise late quoted Accept/WebSocket fallback,
weighted relaxed fractions, canonical/noncanonical HSTS, CORS wildcard/mixed-case
tokens and IPv6, Host percent/IPv6/IPvFuture/IDNA, and relaxed Location
backslashes. For Authorization, include scheme spacing, early/late decoded
colon, padding, fresh/warm extraction and success/failure reuse; retain all
zeroization in the measured lifecycle. Include large-to-small credentials and
lengths around the 64-KiB retention cap, recording retained capacity as well as
allocations. Scanner cases cover every tail length, invalid-byte position and
supported backend. Literal-recognition cases pair hits with equal-length misses
and casing variants.

**Done when:** value and exact error-kind/index assertions cover these strata,
current baselines are recorded, and B4 gates selected maintain cases at any
unexpected allocation increase or any instruction increase.

---

<a id="b4"></a>
### B4 — Gate parser instruction and allocation regressions

**Area:** benchmark execution and regression enforcement · **Priority:** High · **Effort:** Medium

**Purpose:** maintain. The generated benchmark check only compiles benchmarks;
it does not notice slower code. Wire a bounded, representative parser suite to
execute instruction/allocation comparisons against a current recorded baseline.
Preserve generated Anvil ownership: express supported configuration upstream
or add justified repository-specific performance automation, not a hand edit
to the generated recipe.

- `justfiles/anvil/checks/bench.just:17` — `cargo ... bench ... --all-features --no-run`
- `justfiles/anvil/groups/scheduled-exhaustive.just:22` — scheduled tier invokes that compile-only recipe
- `crates/http_headers/docs/PERF.md:3` — timing table is explicitly an imported snapshot

Protect short borrowed/owned singleton decode, retained shared HTTP storage,
all header-family semantic readers, custom-source validation and SIMD
crossovers. Proposed deterministic gates are any unexpected allocation-count
increase and any instruction increase per fixed case. Compare identical inputs,
compiler versions, target flags and operation boundaries; never offset a
regression with gains in other cases. Calibrate execution cost and wall-time
repeatability through B6 rather than claiming hosted timing is stable.

**Done when:** an intentional regression fails the chosen automated entry point,
baseline/toolchain/hardware provenance and update rules are documented, and B1–B3
maintain cases actually execute. A reporting-only job does not close this item.

---

<a id="b5"></a>
### B5 — Measure downstream release code generation and layouts

**Area:** consumer-profile instruction and layout evidence · **Priority:** Medium · **Effort:** Medium

**Purpose:** improve, then maintain accepted wins. Compare a downstream caller
under ordinary release settings with the current fat-LTO benchmark profile.
Black-box input and semantic results. Inspect retained calls, indirect dispatch,
bounds/overflow checks, aggregate copies, type sizes and emitted error/panic
machinery; include Content-Type layout controls. Account for monomorphized build
cost, code-size growth and hot-field/cache-line layout
instead of assuming all inlining is beneficial.

- `Cargo.toml:423` — release config specifies debug information, not fat LTO
- `Cargo.toml:428` — bench enables fat LTO and one codegen unit
- `crates/http_headers/src/headers/cors/shared.rs:1102` — erased validator boundary
- `crates/http_headers/src/headers/content_type.rs:140` — boxed/general metadata tradeoff
- `crates/http_headers/src/headers/negotiation/host.rs:400` — optional-port layout

**Done when:** each codegen hypothesis has actual layout/disassembly and
instruction/time/code-size evidence supporting acceptance or rejection.
Report-only experiments satisfy the improve portion; retained improvements
must gain B4 maintain cases with unchanged allocation expectations and the
zero-regression instruction gate. No unchecked access or removed invariant is an acceptable
substitute for compiler-visible proof.

---

<a id="b6"></a>
### B6 — Establish workload shares and benchmark repeatability

**Area:** request-shaped parsing and measurement calibration · **Priority:** Medium · **Effort:** Medium

**Purpose:** improve. Existing per-header microbenchmarks cannot establish what
fraction of a real request each parser consumes, its input frequencies, or
tail-latency effects. Assemble a non-network request-shaped decode/read bundle
using explicitly sourced, sanitized workload distributions; until those are
available, report separate JSON-request, browser-negotiation and response-policy
strata without claiming a representative weighted average. Vary absent fields,
wire lengths, repeated lines, reader counts and borrowed/owned retention.

- `crates/http_headers/benches/http_headers_micro.rs:87` — existing request fixtures can seed separate strata
- `crates/http_headers/benches/http_headers_per_header.rs:47` — 60-sample Criterion configuration does not establish current variance
- `crates/http_headers/docs/PERF.md:8` — imported per-header timing/instruction/allocation dimensions are not end-to-end shares

Measure total bundle time, per-parser contribution, allocation traffic and
latency distribution; repeat across relevant architectures/profiles. Establish
run-to-run noise and suite duration before selecting timed gates or CI cadence.
This resolves currently unassessed workload shares, tail behavior,
instruction-cache tradeoffs, variance and execution-budget practicality.

**Done when:** reporting baselines bound plausible end-to-end benefits for
parser optimizations, record distribution provenance and repeatability, and
determine a practical B4 suite/cadence. This is an improve-only measurement: a report is
sufficient here, but it does not replace B4's regression gate.
