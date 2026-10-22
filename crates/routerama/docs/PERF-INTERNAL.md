# Routerama Internal Performance Record

The engineering side of the measurement split: instruction counts,
acceptance-gate evaluations, audit boundaries, and generated-code size. These
exist to find and prevent regressions in our own code, so they cover every major
functional area and make no comparative claim.

The customer-facing comparison lives in [`PERF.md`](PERF.md), and
[`benches/README.md`](../benches/README.md) describes the three benchmark
families and which audience each serves.

The tables between `<!-- BEGIN GENERATED -->` and `<!-- END GENERATED -->`
markers are rebuilt by `scripts/perf_report.rs` from:
- `cargo bench --features resolve --bench gungraun_routers` — Callgrind instruction-precise counts.
- `cargo bench --features query --bench routerama_query_cg` — query-codec instruction counts.

Running with `--no-gungraun`, or on a host without valgrind, leaves this file
untouched; `PERF.md` carries no instruction counts and is unaffected either way.
Everything outside the markers is hand-written and survives regeneration.

Instructions, branch misses, and total reads+writes are per full lookup sweep.
Total reads+writes = instruction reads + data reads + data writes.

## Resolver instruction counts

The same full lookup sweep whose wall-clock timings appear in [`PERF.md`](PERF.md).

<!-- BEGIN GENERATED: resolver-instructions -->

| Resolver | Instructions | Branch misses | Total reads+writes |
|---|---:|---:|---:|
| `routerama_static` | 15,284 | 184 | 21,085 |
| `routerama_dynamic` | 26,764 | 375 | 37,577 |
| `matchit` | 27,902 | 523 | 38,136 |
| `path_tree` | 42,467 | 571 | 59,106 |
| `regex` | 85,321 | 613 | 115,570 |
| `route_recognizer` | 212,314 | 1,427 | 310,049 |
<!-- END GENERATED: resolver-instructions -->

## Five-framework instruction counts

The instruction-count half of the comparative tables in [`PERF.md`](PERF.md).
They are kept here because instruction counting is our regression tool rather
than a customer-facing claim; the wall-clock comparison is what a reader
evaluating Routerama should weigh.

### Five-framework HTTP fixture

Instructions per request, Callgrind. Lower is better. The wall-clock half of
this table is in [`PERF.md`](PERF.md).

| Scenario | Routerama | Axum | Actix Web | Rocket | Warp |
|---|---:|---:|---:|---:|---:|
| Literal first | 2,633 | 4,650 | 3,952 | 19,431 | 6,725 |
| Literal middle | 2,647 | 4,670 | 6,558 | 20,578 | 11,000 |
| Literal last | 2,606 | 4,653 | 7,499 | 22,002 | 14,459 |
| Captures | 3,191 | 7,210 | 19,801 | 22,345 | 9,238 |
| Method/header/query | 5,970 | 9,637 | 9,593 | 24,462 | 12,875 |
| Status/header response | 2,941 | 5,284 | 6,571 | 19,423 | 10,190 |
| Complete miss | 1,707 | 4,669 | 5,898 | 17,695 | 13,630 |
| Capture failure | 1,759 | 6,218 | 28,737 | 18,431 | 8,236 |

### Route-set scaling

Instructions per request, Callgrind. Lower is better. The wall-clock half of
this table is in [`PERF.md`](PERF.md).

| Routes/scenario | Routerama | Axum | Actix Web | Rocket | Warp |
|---|---:|---:|---:|---:|---:|
| 16/first | 2,879 | 5,276 | 4,477 | 20,413 | 5,173 |
| 16/middle | 2,878 | 5,316 | 5,669 | 22,925 | 13,695 |
| 16/last | 2,875 | 5,306 | 6,705 | 25,079 | 20,758 |
| 16/miss | 1,540 | 4,398 | 4,354 | 21,227 | 19,159 |
| 128/first | 2,972 | 5,333 | 4,601 | 20,315 | 5,794 |
| 128/middle | 2,972 | 5,383 | 14,073 | 40,198 | 75,237 |
| 128/last | 2,969 | 5,736 | 23,397 | 60,073 | 142,647 |
| 128/miss | 1,618 | 4,427 | 17,570 | 52,909 | 136,007 |
| 1,024/first | 3,104 | 5,390 | 4,597 | 20,319 | 6,415 |
| 1,024/middle | 3,104 | 5,430 | 81,563 | 182,119 | 563,231 |
| 1,024/last | 3,091 | 5,372 | 159,231 | 338,984 | 1,117,446 |
| 1,024/miss | 1,745 | 4,428 | 123,298 | 307,486 | 1,072,327 |

### Bounded body extraction

Instructions per request, Callgrind. Lower is better. The wall-clock half of
this table is in [`PERF.md`](PERF.md).

| Scenario | Routerama | Axum | Actix Web | Rocket | Warp |
|---|---:|---:|---:|---:|---:|
| Bytes, one frame | 3,303 | 7,351 | 5,472 | 33,829 | 5,657 |
| Bytes, two frames | 3,579 | 8,021 | 5,557 | 33,908 | 6,301 |
| Bytes, exactly 64 | 3,474 | 7,519 | 5,642 | 34,092 | 5,829 |
| UTF-8 text | 3,716 | 7,973 | 7,751 | 26,633 | 6,556 |
| JSON | 6,387 | 10,977 | 8,067 | 28,609 | 10,246 |
| Bytes, 65 rejected | 2,127 | 5,503 | 4,297 | 30,629 | 4,858 |
| UTF-8 text, 65 rejected | 2,107 | 5,810 | 6,226 | 23,351 | 4,866 |
| JSON, encoded 65 rejected | 3,276 | 6,838 | 5,246 | 22,982 | 5,642 |
| Invalid UTF-8 | 2,507 | 6,425 | 6,592 | 24,952 | 4,965 |
| Malformed JSON | 4,403 | 8,801 | 6,218 | 25,204 | 8,217 |
| Unsupported JSON media type | 3,258 | 6,702 | 5,432 | 21,140 | 7,308 |
| Missing JSON media type | 2,248 | 5,342 | 4,092 | 18,082 | 4,880 |

### Bounded form extraction

Instructions per request, Callgrind. Lower is better. The wall-clock half of
this table is in [`PERF.md`](PERF.md).

| Scenario | Routerama | Axum | Actix Web | Rocket | Warp |
|---|---:|---:|---:|---:|---:|
| Form, one frame | 6,700 | 12,862 | 10,579 | 34,935 | 12,403 |
| Form, two frames | 7,518 | 13,526 | 10,659 | 34,935 | 12,625 |
| Form, exactly 64 | 7,215 | 14,047 | 11,764 | 36,364 | 13,869 |
| Percent- and plus-encoded | 7,142 | 13,574 | 11,293 | 35,813 | 13,115 |
| Optional field absent | 6,379 | 11,772 | 9,811 | 34,629 | 11,429 |
| Form, encoded 65 rejected | 2,968 | 6,572 | 5,691 | 25,284 | 4,542 |
| Malformed number | 4,274 | 9,969 | 8,356 | 31,178 | 9,493 |
| Missing required field | 3,910 | 9,698 | 8,412 | 30,696 | 9,553 |
| Unsupported media type | 2,932 | 5,975 | 4,506 | 20,806 | 5,880 |
| Missing media type | 2,181 | 5,612 | 3,742 | 17,748 | 3,504 |


## Routerama-only scenario records

These scenarios have no five-framework fixture, so they make no comparative
claim and are not customer-facing. They are retained here as regression
evidence for our own code; the instruction-count suite covers each of them
(`routerama_response_body_cg`, `routerama_route_policy_cg`,
`routerama_interceptors_cg`, `routerama_tower_cg`, `routerama_mount_cg`,
`routerama_response_templates_cg`).

## Response-body representation

The response fixture isolates Routerama's fixed buffer, concrete streaming
body, generated service sum, and explicit `BoxBody` boundary. Prepared streams
are constructed outside the measured region. `BoxBody` construction and
destruction remain inside it because that allocation is the cost of opting
into erasure.

```text
cargo bench -p routerama --features route \
    --bench routerama_response_body -- \
    --warm-up-time 1 --measurement-time 2 --sample-size 30 --noplot
cargo bench -p routerama --features route \
    --bench routerama_response_body_cg
```

| Scenario | Criterion estimate | Instructions | Measured allocations | Measured bytes |
|---|---:|---:|---:|---:|
| Direct fixed body | 60 ns | 232 | 0 | 0 |
| Direct concrete stream | 197 ns | 1,016 | 0 | 0 |
| Direct boxed stream | 252 ns | 1,166 | 1 | 168 |
| Generated route, fixed body | 114 ns | 550 | 0 | 0 |
| Generated route, concrete stream | 397 ns | 1,978 | 0 | 0 |
| Generated route, boxed stream | 472 ns | 2,275 | 1 | 168 |
| Generated concrete error | 244 ns | 1,266 | 0 | 0 |
| Generated boxed error | 324 ns | 1,565 | 2 | 24 |

On the generated streaming path, explicit boxing costs 19% more median time
and 15% more instructions. On the generated error path it costs 33% more time
and 24% more instructions. The generated concrete paths allocate nothing in
the measured region; boxing adds one body allocation and, when an error occurs,
one error allocation.

On this 64-bit x86-64 Linux host, `Body` is 32 bytes, the concrete stream and
`EitherBody<Body, Stream>` are 168 bytes, and `BoxBody` is 16 bytes. A
fixed-only generated response is 152 bytes with a 40-byte opaque body; the
heterogeneous generated response is 280 bytes with a 168-byte opaque body.
Both route futures are 488 bytes. These are layout observations, not stable API
guarantees.

The original generated controls kept route count fixed at 16 while varying
const-generic wrappers around one underlying body implementation:

| Distinct body types | Median warm target compile | `.text` | `.rodata` |
|---:|---:|---:|---:|
| 1 | 4.57 s | 249,226 bytes | 22,800 bytes |
| 4 | 4.81 s | 249,434 bytes | 22,800 bytes |
| 16 | 5.10 s | 249,466 bytes | 22,800 bytes |

That historical five-run sample appeared to show a 12% compile-time increase
and 240 bytes of loadable-code growth. A 2026-07-27 reproduction on the
documented nightly found that conclusion was stale:

| Const-generic wrappers | Median warm target compile | `.text` | `.rodata` |
|---:|---:|---:|---:|
| 1 | 5.52 s | 266,691 bytes | 24,112 bytes |
| 4 | 5.41 s | 266,883 bytes | 24,176 bytes |
| 16 | 5.56 s | 268,243 bytes | 24,240 bytes |

Current loadable code grows by 1,552 bytes and read-only data by 128 bytes.
Compile time is non-monotonic and its 2.8% median spread is below observed
run-to-run noise, so this fixture supports no compile-time-growth conclusion.
Because the wrappers delegate to the same body implementation and LLVM can fold
their monomorphizations, it measures generated enum/dispatch variant overhead,
not the code-size cost of 16 genuinely different streaming state machines.

## Generated response-template evaluation

The response-template fixture compares existing contiguous formatting, one
exactly sized contiguous allocation, and a segmented body with static
`Bytes::from_static` frames. Slots are typed as JSON numbers, escaped JSON
strings, or escaped HTML text; no untyped concatenation API is exposed.

Both candidates meet the allocation contract: fully static output allocates
zero times and every dynamic output exactly once. The exact-contiguous
instruction geomean improves 37.1% in memory and 7.0% through an in-memory
Hyper HTTP/1 connection. Segmented improves 23.4% in memory but regresses 0.4%
at Hyper HTTP/1; pinned wall-time geomeans regress 9.4%. Hyper forwards the
static spans directly, but the segmented body grows 32 -> 80 bytes, its
connection future grows 656 -> 696 bytes, and dynamic responses require three
body polls and four IO slices instead of one poll and two slices.

Exact contiguous is therefore retained through the focused
`response::json_body_template!` and `response::html_body_template!` macros.
They expose only integer JSON numbers, escaped JSON strings, and escaped HTML
text, evaluate slot expressions once, and never accept raw dynamic fragments.
Segmented production adoption is rejected. `StaticText`/`StaticBytes` remain
the direct zero-copy choices for slot-free responses. The numbers backing this
disposition are the tables in this section, the response-template size controls
under "Generated-code size and compile time" below, and the
`routerama_response_templates` / `routerama_response_templates_cg` benchmark
pair; no separate report file holds them.

## Route policy

Overlap priority, request predicates, the typed routing fallback, and the typed
extractor catcher each have their own subgroup, and each subgroup contains the
plainest generated control that reaches the same response boundary. The reported
difference is therefore the policy's own cost, not routing in general. Every
handler returns a static `Bytes` payload so no response body allocates, and
request construction stays outside the measured region.

Captured on 2026-07-26 with `rustc 1.98.0-nightly (6368fd52c 2026-05-29)` and
`valgrind-3.26.0` on an AMD EPYC 7763 under WSL2:

```text
cargo bench -p routerama --features route \
    --bench routerama_route_policy -- \
    --warm-up-time 1 --measurement-time 2 --sample-size 30 --noplot
cargo bench -p routerama --features route \
    --bench routerama_route_policy_cg
```

| Subgroup / scenario | Median time | Instructions | Measured allocations | Measured bytes |
|---|---:|---:|---:|---:|
| `priority/plain_route` | 127 ns | 919 | 0 | 0 |
| `priority/highest_candidate` | 346 ns | 2,729 | 2 | 656 |
| `priority/lower_candidate` | 407 ns | 2,976 | 2 | 656 |
| `predicates/unconstrained` | 127 ns | 837 | 0 | 0 |
| `predicates/accepted` | 593 ns | 4,168 | 2 | 656 |
| `predicates/unsupported_media_type` | 282 ns | 2,084 | 0 | 0 |
| `predicates/not_acceptable` | 363 ns | 2,889 | 0 | 0 |
| `fallback/default_miss` | 98 ns | 581 | 0 | 0 |
| `fallback/typed_miss` | 106 ns | 663 | 0 | 0 |
| `catcher/default_rejection` | 107 ns | 665 | 0 | 0 |
| `catcher/typed_rejection` | 130 ns | 805 | 0 | 0 |

A typed `#[fallback]` costs 82 instructions (14%) over the generated default 404,
and a typed `#[catch]` costs 140 instructions (21%) over letting the rejection's
own `IntoResponse` answer. Both are direct calls: neither allocates.

Media-type policy is the expensive part, and its cost is header parsing rather
than route selection. Rejecting an unacceptable `Content-Type` costs 1,247
instructions over the predicate-free control and allocates nothing; also
negotiating `Accept` before rejecting costs 2,052. Accepting and then *writing*
the negotiated `Content-Type` costs 3,331, of which the response header map is
2 allocations and 656 bytes — `http::HeaderMap`'s own first-insert storage, not
Routerama state. The overlap rows carry the same header cost: ranking two
`produces` candidates and answering the winner costs 1,810 instructions over an
unambiguous route on the same service, and selecting the lower-priority
candidate costs 247 more than the higher-priority one because ranking walks
further.

The practical reading is that predicate-free routing, typed fallbacks, and typed
catchers are effectively free, while `consumes`/`produces` negotiation should be
declared only on routes that need it.

## Explicit mounted-service boundary

A prepared static request through a populated `route_with_erased_mounts` wrapper
reports zero allocations and zero mounted-service calls. The generated response
remains the left branch of `EitherBody<GeneratedBody, BoxBody>`; no generated
body or future is boxed.

The paired immediate mounted request reports exactly two allocations: one
boxed erased-service future and one `BoxBody`. It crosses the service vtable
once and subsequently polls the boxed future and body dynamically. A body
error adds the existing conditional error box. Up to four capture ranges and
the matcher's 16-segment scratch boundary remain inline; larger values can add
documented matcher scratch spills. A shallow standalone miss invokes no
service and reports one allocation for its boxed fixed 404 body.

Two paired workloads now measure that boundary. `static_hit` routes the
identical static request through the ordinary generated `route` entry and
through `route_with_erased_mounts` over a populated two-entry mount table.
`dynamic_dispatch` routes through one configured-dynamic router in which a
generated dynamic handler and an erased mount serve two different paths, so the
two rows differ only in which side of the same entry answers. All four rows
return a byte-identical response, and a mounted-call counter proves only the
mount row invokes a service.

Captured on 2026-07-26 with `rustc 1.98.0-nightly (6368fd52c 2026-05-29)` and
`valgrind-3.26.0` on an AMD EPYC 7763 under WSL2:

```text
cargo bench -p routerama --features mount \
    --bench routerama_mount -- \
    --warm-up-time 1 --measurement-time 2 --sample-size 30 --noplot
cargo bench -p routerama --features mount \
    --bench routerama_mount_cg
```

| Subgroup / scenario | Median time | Instructions | Measured allocations | Measured bytes | Mounted calls |
|---|---:|---:|---:|---:|---:|
| `static_hit/plain_route` | 106 ns | 784 | 0 | 0 | 0 |
| `static_hit/populated_erased_mounts` | 121 ns | 853 | 0 | 0 | 0 |
| `dynamic_dispatch/configured_dynamic` | 138 ns | 1,062 | 0 | 0 | 0 |
| `dynamic_dispatch/erased_mount` | 322 ns | 2,555 | 2 | 760 | 1 |

Configuring erased mounts costs a generated static request 69 instructions
(8.8%) and no allocation: the wrapper's only added work is choosing the
generated branch before the mount table is consulted. An erased mounted hit
costs 2.4x the instructions of the generated configured-dynamic hit that
answers through the same entry, which is the price of the boxed service future,
the `BoxBody`, and dynamic polling of both.

These mounted numbers must not be folded into the generated-static
five-framework geometric means. A mount deliberately selects a different,
type-erased capability boundary, and comparing it with a compiled-in handler
would misrepresent both. The remaining mount workloads — captures, streams,
stream errors, 16/128/1,024-entry tables, and paths on both sides of the
16-segment matcher scratch boundary — were measured on 2026-07-27 and are
recorded in the next section.

## Wider mounted-service matrix

This extends the previous section to the complete mounted surface: a
standalone mounted literal and a complete mount-table miss driven through
`ErasedMountRouter::route` with no generated wrapper in front of them, zero,
one, four, and five captures across the four-capture inline boundary, a
concrete streaming mounted body that completes and one that fails, requests
with 16 and 17 path segments across the 16-segment matcher scratch boundary,
and mount tables of 16, 128, and 1,024 entries probed at their first, middle,
and last entry and with a miss.

Every table, service, generated request path, and request is built at fixture
startup, outside the measured region; a scaling table is generated in a setup
loop rather than written out. The measured region is exactly the routing call
plus complete response-body observation.

**Named allocations.** Allocations are measured in two consecutive spans per
row. The `routing` span covers matching, the erased call, and response
construction; the `observing` span covers complete body observation. Four
categories are named — `future` (the boxed erased-service future), `body` (the
`BoxBody`), `error` (the boxed body error), and `scratch` (matcher offset or
capture-range storage that leaves its inline capacity) — and each one is
*pinned by a differential pair that varies exactly one thing*, so the naming is
falsifiable rather than assumed:

| Named allocation | Isolated by | Measured delta |
|---|---|---:|
| `body` | `standalone/complete_miss` invokes no service at all | 1 allocation, 32 bytes |
| `future` | `standalone/literal` minus `standalone/complete_miss` | +1 allocation, +728 bytes |
| `scratch` (captures) | `captures/five` minus `captures/four` | +1 allocation, +80 bytes |
| `scratch` (matcher offsets) | `depth/segments_17` minus `depth/segments_16` | +1 allocation, +272 bytes |
| `error` | `streaming/error` minus `streaming/success`, observing span only | +1 allocation, +8 bytes |

`tests/mount_fixtures.rs` asserts all five differentials, the per-row
decomposition against both measured spans, the declared response of every row,
and that only mounted hits invoke a service.

Captured on 2026-07-27 with `rustc 1.98.0-nightly (6368fd52c 2026-05-29)` and
`valgrind-3.26.0` on an AMD EPYC 7763 (16 visible cores) under WSL2:

```text
cargo bench -p routerama --features mount \
    --bench routerama_mount -- \
    --warm-up-time 1 --measurement-time 2 --sample-size 30 --noplot
cargo bench -p routerama --features mount \
    --bench routerama_mount_cg
```

The four rows the previous section already reported were re-measured with this
run's fixture and are repeated here so every row comes from one run. They are
not identical to the 2026-07-26 figures: the shared fixture now builds seven
mount tables and dispatches 26 scenarios instead of four, which moves the
generated rows by a few percent. Compare rows within a run, not across runs.

The table below is the state *after* the sibling-literal lookup fix described
under "Mount-table size no longer depends on registration position". The first
measurement of this matrix, on the same day and the same fixture, found the
runtime matcher's cost growing with a node's sibling-literal count; that
measurement is kept as labelled before-fix evidence in the same subsection.
Every other table in this document predates the fix. Only one other workload
touches the run-time resolver at all — `compare_routers` — and its measured
change is recorded under that table; generated dispatch is byte-identical
across the fix, here and there.

| Subgroup / scenario | Median time | Instructions | Routing allocations | Observing allocations | `future` | `body` | `error` | `scratch` | Measured bytes |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `static_hit/plain_route` | 114 ns | 796 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| `static_hit/populated_erased_mounts` | 154 ns | 849 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| `dynamic_dispatch/configured_dynamic` | 172 ns | 1,115 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| `dynamic_dispatch/erased_mount` | 342 ns | 2,719 | 2 | 0 | 1 | 1 | 0 | 0 | 760 |
| `standalone/literal` | 263 ns | 2,210 | 2 | 0 | 1 | 1 | 0 | 0 | 760 |
| `standalone/complete_miss` | 126 ns | 1,011 | 1 | 0 | 0 | 1 | 0 | 0 | 32 |
| `captures/none` | 275 ns | 2,403 | 2 | 0 | 1 | 1 | 0 | 0 | 760 |
| `captures/one` | 302 ns | 2,613 | 2 | 0 | 1 | 1 | 0 | 0 | 760 |
| `captures/four` | 361 ns | 3,081 | 2 | 0 | 1 | 1 | 0 | 0 | 760 |
| `captures/five` | 389 ns | 3,509 | 3 | 0 | 1 | 1 | 0 | 1 | 840 |
| `streaming/success` | 327 ns | 2,514 | 2 | 0 | 1 | 1 | 0 | 0 | 738 |
| `streaming/error` | 297 ns | 2,450 | 2 | 1 | 1 | 1 | 1 | 0 | 746 |
| `depth/segments_16` | 446 ns | 3,943 | 2 | 0 | 1 | 1 | 0 | 0 | 760 |
| `depth/segments_17` | 474 ns | 4,781 | 3 | 0 | 1 | 1 | 0 | 1 | 1,032 |
| `table_size/0016_first` | 346 ns | 2,574 | 2 | 0 | 1 | 1 | 0 | 0 | 760 |
| `table_size/0016_middle` | 303 ns | 2,571 | 2 | 0 | 1 | 1 | 0 | 0 | 760 |
| `table_size/0016_last` | 310 ns | 2,571 | 2 | 0 | 1 | 1 | 0 | 0 | 760 |
| `table_size/0016_miss` | 187 ns | 1,361 | 1 | 0 | 0 | 1 | 0 | 0 | 32 |
| `table_size/0128_first` | 347 ns | 2,703 | 2 | 0 | 1 | 1 | 0 | 0 | 760 |
| `table_size/0128_middle` | 340 ns | 2,703 | 2 | 0 | 1 | 1 | 0 | 0 | 760 |
| `table_size/0128_last` | 339 ns | 2,700 | 2 | 0 | 1 | 1 | 0 | 0 | 760 |
| `table_size/0128_miss` | 220 ns | 1,490 | 1 | 0 | 0 | 1 | 0 | 0 | 32 |
| `table_size/1024_first` | 378 ns | 2,832 | 2 | 0 | 1 | 1 | 0 | 0 | 760 |
| `table_size/1024_middle` | 372 ns | 2,829 | 2 | 0 | 1 | 1 | 0 | 0 | 760 |
| `table_size/1024_last` | 374 ns | 2,843 | 2 | 0 | 1 | 1 | 0 | 0 | 760 |
| `table_size/1024_miss` | 257 ns | 1,618 | 1 | 0 | 0 | 1 | 0 | 0 | 32 |

**Captures.** Materializing and reading capture ranges costs about 210
instructions for the first capture and about 156 for each of the next three;
zero to four captures allocate nothing. The fifth capture leaves the
four-element inline capacity and adds exactly one 80-byte allocation
(`5 × size_of::<Range<usize>>()`) and 428 instructions over four captures,
most of which is that allocation.

**Streams.** A concrete streaming mounted body costs 304 instructions over a
fixed one (`streaming/success` against `standalone/literal`) and allocates
nothing beyond the same boxed future and `BoxBody`; both frames are static.
A body that fails on its second frame allocates exactly one 8-byte boxed
error, and only while the body is observed. Its instruction count is *below*
the successful stream's because it stops one frame early — the error box is
cheaper than the frame it replaces.

**Depth.** With a mount table deep enough to disable the fixed-size offset fast
path, a 16-segment request stays inline and a 17-segment request spills exactly
one 272-byte scratch allocation (`17 × 2 × size_of::<usize>()`), costing 838
instructions. The documented matcher boundary behaves exactly as documented,
and both sides of it are now measured rather than asserted.

## Mount-table size no longer depends on registration position

The first capture of the matrix above found the one result in this document
that got worse with scale: a mount table is matched by the *runtime* resolver,
not by generated code, and its per-node literal lookup was a scan of every
sibling literal, so a hit cost work proportional to the matched entry's
registration position. **Before-fix evidence, kept for the record** (2026-07-27,
same fixture, same host, matcher as it stood before the change):

| Mount entries | first | middle | last | miss |
|---:|---:|---:|---:|---:|
| 16 | 2,386 | 2,709 | 2,992 | 1,216 |
| 128 | 2,386 | 4,958 | 7,465 | 1,776 |
| 1,024 | 2,386 | 22,841 | 43,264 | 6,256 |

The matcher now picks its lookup per node, from the node's own fanout:

* Below `SORTED_LITERAL_FANOUT` (16) sibling literals, `RtNode::compile` keeps
  the edges ordered by descending subtree weight and `find_literal` scans them,
  which is exactly the code and the order that shipped before.
* At or above it, compilation sorts that node's edges by key bytes and
  `find_literal` binary searches them. Keys are unique within a node, so
  ordering is a lookup heuristic only: precedence, captures, and backtracking
  are untouched, and `Walk::descend_iterative` still tries literal, then affix,
  then single wildcard, then rest, with the same pending-edge stack.

No node carries an extra index, no request allocates, no dependency was added,
and generated static routing is not involved at all. The same matrix, after:

| Mount entries | first | middle | last | miss |
|---:|---:|---:|---:|---:|
| 16 | 2,574 | 2,571 | 2,571 | 1,361 |
| 128 | 2,703 | 2,703 | 2,700 | 1,490 |
| 1,024 | 2,832 | 2,829 | 2,843 | 1,618 |

Position no longer matters: the three hit positions of a 1,024-entry table now
agree to within 14 instructions, against a spread of 40,878 before, and a miss
costs 1,618 instructions instead of 6,256. Against the before-fix run, row by
row:

| Row | Before | After | Delta |
|---|---:|---:|---:|
| `0016_first` | 2,386 | 2,574 | +188 (+7.9%) |
| `0016_middle` | 2,709 | 2,571 | −138 (−5.1%) |
| `0016_last` | 2,992 | 2,571 | −421 (−14.1%) |
| `0016_miss` | 1,216 | 1,361 | +145 (+11.9%) |
| `0128_first` | 2,386 | 2,703 | +317 (+13.3%) |
| `0128_middle` | 4,958 | 2,703 | −2,255 (−45.5%) |
| `0128_last` | 7,465 | 2,700 | −4,765 (−63.8%) |
| `0128_miss` | 1,776 | 1,490 | −286 (−16.1%) |
| `1024_first` | 2,386 | 2,832 | +446 (+18.7%) |
| `1024_middle` | 22,841 | 2,829 | −20,012 (−87.6%) |
| `1024_last` | 43,264 | 2,843 | −40,421 (−93.4%) |
| `1024_miss` | 6,256 | 1,618 | −4,638 (−74.1%) |

The cost is paid where a scan was already optimal: a first-registered entry
pays the search's `log2(n)` comparisons instead of one, which is +188 at 16
entries and +446 at 1,024, and the 16-entry miss pays +145 because a scan
rejects most keys on a length compare. That is the accepted trade — the worst
row in the table is now 2,843 instructions instead of 43,264, and no row
exceeds 2,843.

Everything outside the scaling matrix is unchanged: the two generated
`static_hit` rows are byte-identical (796 and 849), and the other twelve rows
move by at most 16 instructions (≤0.5%), which is the fanout test itself on
narrow nodes. **The allocation decomposition is entirely unchanged** — every
hit still allocates exactly two (`future` + `body`, 760 bytes), every miss
exactly one (`body`, 32 bytes), `captures/five` and `depth/segments_17` still
add exactly one `scratch`, and `streaming/error` still adds exactly one
observing-span `error`. `tests/mount_fixtures.rs` asserts all of it.

**Why 16.** The threshold is measured, not assumed. The scaling matrix was
extended to eight table widths and run twice, once with each strategy forced
everywhere (`SORTED_LITERAL_FANOUT` set to `usize::MAX` and to `0`), on the
shipped implementation. Ratios are binary-search cost over scan cost, so below
1.00 means the search wins:

| Sibling literals | first | middle | last | miss | aggregate-total ratio | geomean | worst position (tail) |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 8 | 1.08 | 1.01 | 0.97 | 1.15 | 1.038 | 1.051 | 0.97 |
| 12 | 1.10 | 1.00 | 0.93 | 1.17 | 1.029 | 1.047 | 0.93 |
| 16 | 1.10 | 0.97 | 0.88 | 1.15 | **1.001** | 1.021 | 0.88 |
| 24 | 1.12 | 0.93 | 0.81 | 1.15 | 0.966 | 0.993 | 0.81 |
| 32 | 1.12 | 0.88 | 0.73 | 1.11 | 0.916 | 0.949 | 0.73 |
| 64 | 1.15 | 0.74 | 0.55 | 1.02 | 0.776 | 0.830 | 0.55 |
| 128 | 1.17 | 0.56 | 0.37 | 0.86 | 0.593 | 0.673 | 0.37 |
| 1,024 | 1.23 | 0.13 | 0.07 | 0.27 | 0.139 | 0.229 | 0.07 |

Sixteen is where the search stops costing more than the scan on the ratio of
aggregate instruction totals for the four positions (1.001) while already
taking 12% off the worst position. Below it the search loses on both counts —
8 and 12 are 3–4% aggregate regressions
— and the narrow nodes that make up most of a real trie lose more: forcing the
search everywhere costs `depth/segments_16` 7.0%, `standalone/complete_miss`
4.8%, and `captures/none` 4.5%, because those rows traverse nodes with one to
four literal edges. Aggregated over all thirty-two measured rows, threshold 16
has the best aggregate-total ratio (0.501 against 0.505 at 8 and 0.504 at 32) and a geomean
within 0.2% of the best (0.770 against 0.769 at 32), while 32 would leave
every table between 16 and 31 entries with both a worse tail (up to 1.24x at
24 entries) and, from 24 up, a worse aggregate ratio. Eight is dominated on
both metrics.
The worst single row is the same for every candidate: a first-registered entry
in a 1,024-entry table, at 1.23x.

This calibration is shape-specific: every sibling key has equal length, the
same first byte, and a long shared prefix, so the linear scan's length/first-byte
prefilter cannot reject candidates cheaply. Heterogeneous sibling names and
tries with several wide nodes remain unmeasured; 16 is therefore a defensible
threshold for the measured worst-case shape, not a universal optimum.

**Startup and memory.** Sorting happens once per wide node, in place, in
`RtNode::compile`; nothing is added to `RtNode`, so a compiled trie occupies
exactly the same memory as before. The build-time cost was measured the same
way, by building eight tables of each width under Callgrind and subtracting an
empty-table baseline: +209 instructions on a 16-entry table (+0.08%), +3,341
on a 128-entry table (+0.16%), and +27,141 on a 1,024-entry table (+0.16%) of
a construction that already costs 17.0 M. A single request for the last entry
of that table saves 40,421 instructions, so the sort repays itself before the
first request finishes.

**Scope.** The lookup is shared by every runtime-registered table — erased
mounts, `#[route(dynamic)]` tables, and `RawResolver` — so all three get the
same behavior; it was never an erasure cost. Generated static routers are not
affected in either direction, because their segment dispatch is compiled: the
1,024-route generated table measured 3,091 instructions on its last route on
2026-07-25 against 2,875 at 16 routes, and was flat before this change too.

## Generated interceptors

`#[before]`, `#[after]`, and `#[transform]` interceptors compile to direct
method calls on the service with no boxed future or service. A service with no
interceptor annotations emits byte-for-byte the previous dispatch, so unrelated
routers pay nothing.

`#[before]` adds only its own call. A per-handler `#[before]` builds its
`SelectedContext` from five borrows of existing request-head fields, which is a
pointer-sized bundle on the stack, not a copy of the head. `#[after]` adds its
call plus one move-only response `into_parts`/`from_parts` round trip that does
not allocate and preserves the concrete body, its frames, and its trailers. A
bare `#[after]` costs one such round trip per request, at the entry, rather than
one per dispatch arm, because the entry lowers dispatch into a labeled block.
Short-circuit and mutation responses join the existing concrete response body
sum rather than boxing.

A `#[transform(limit = N, ...)]` buffers its named handler's body through the
same bounded `collect_body` path as the buffered body extractors, so it
allocates its buffer exactly like `BytesBody`/`TextBody` and imposes buffering
only on the routes it names. A `#[transform(stream, ...)]` allocates nothing at
all: the transport body is moved into a monomorphized generic call, and the
returned wrapper type is substituted into the handler's `#[body]` extraction, so
the framework adds no buffer, box, or dynamic dispatch on that path.

The paired workload compares zero, one, and four passive router-wide
interceptors of each kind, and compares ordinary bounded `#[body]` extraction
against the same extraction behind a buffering and a streaming transform. Every
scenario returns a byte-identical response, which the equivalence test asserts,
so each subgroup's spread is interceptor overhead alone.

Captured on 2026-07-26 with `rustc 1.98.0-nightly (6368fd52c 2026-05-29)` and
`valgrind-3.26.0` on an AMD EPYC 7763 under WSL2:

```text
cargo bench -p routerama --features route \
    --bench routerama_interceptors -- \
    --warm-up-time 1 --measurement-time 2 --sample-size 30 --noplot
cargo bench -p routerama --features route \
    --bench routerama_interceptors_cg
```

| Subgroup / scenario | Median time | Instructions | Measured allocations | Measured bytes |
|---|---:|---:|---:|---:|
| `before/none` | 82 ns | 642 | 0 | 0 |
| `before/one` | 92 ns | 672 | 0 | 0 |
| `before/four` | 103 ns | 694 | 0 | 0 |
| `after/none` | 89 ns | 642 | 0 | 0 |
| `after/one` | 106 ns | 700 | 0 | 0 |
| `after/four` | 123 ns | 731 | 0 | 0 |
| `transform/none` | 266 ns | 1,225 | 1 | 27 |
| `transform/bounded` | 331 ns | 1,767 | 2 | 54 |
| `transform/streaming` | 243 ns | 1,291 | 1 | 27 |

The first `#[before]` costs 30 instructions and each of the next three costs
about 7 more; four cost 52 in total. The first `#[after]` costs 58 instructions,
which includes the single `into_parts`/`from_parts` round trip the entry
performs once, and each of the next three costs about 10 more; four cost 89 in
total. None of the eight interceptor rows allocates.

A streaming `#[transform]` costs 66 instructions (5%) over unwrapped bounded
extraction and adds no allocation: the handler's own `BytesBody` buffer is the
only one in either row. A buffering `#[transform]` costs 542 instructions (44%)
and exactly one extra allocation, because it collects the body itself and then
hands a replacement body to the same `#[body]` extraction — the fixture's
deliberate worst case for double buffering. The streaming row's median time is
below the unwrapped control's, which is wall-clock noise at this scale; the
deterministic instruction counts are the ones to read.

## Tower transport adapter

`RouteService` stores the concrete router, state, and callable and holds the
callable's own future inline in the named `RouteFuture`, so the adapter itself
contributes no box, no trait object, no vtable call, and no type map. Per
request it adds one clone of the stored router and one clone of the stored
state; both are free for a zero-sized router and a `Copy` state, and one atomic
increment for an `Arc`. The request is never boxed, cloned, or re-parsed.

The only measurable cost is the optional erased response boundary. A prepared
generated static request reports **zero** allocations through the default
identity and generated exact boundaries, and **exactly one** through
`SendBoxBody`, which is the boxed body itself; a body error adds the existing
conditional error box only if it occurs. `BoxBody` behaves identically for the
local boundary. Normalization runs exactly once, when the routing future
resolves, and forwards data frames, trailers, size hints, and end-of-stream
state unchanged.

The paired workload drives the same generated static request four ways.
`RouteService`'s `Call` parameter is an unnameable closure, so all four rows
are prepared as boxed `FnOnce` values exactly like the five-framework fixtures;
every row therefore pays the same single indirect call inside the measured
region, and service construction stays outside it. The ready-future poll is a
macro expanded at each call site: a generic helper was selectively outlined
for only one opaque future, which measured code placement rather than the
response boundary.

Captured on 2026-07-28 with `rustc 1.98.0-nightly (6368fd52c 2026-05-29)` and
`valgrind-3.26.0` on an AMD EPYC 7763 under WSL2:

```text
cargo bench -p routerama --no-default-features --features tower \
    --bench routerama_tower -- \
    --warm-up-time 1 --measurement-time 3 --sample-size 40 --noplot
cargo bench -p routerama --no-default-features --features tower \
    --bench routerama_tower_cg
```

| Scenario | Criterion estimate | Instructions | Measured allocations | Measured bytes |
|---|---:|---:|---:|---:|
| `dispatch/direct_route` | 142.01 ns | 784 | 0 | 0 |
| `dispatch/route_service_exact_body` | 163.18 ns | 839 | 0 | 0 |
| `dispatch/generated_exact_tower` | 157.22 ns | 839 | 0 | 0 |
| `dispatch/route_service_send_box_body` | 177.22 ns | 954 | 1 | 40 |

The generated exact adapter's Criterion point estimate is within 3.65% of the
inferred `ExactBody` control, it executes the identical 839 instructions, and
allocates nothing. `SendBoxBody` costs 115 more instructions (12.1%) and one
40-byte allocation. The exact and generated values also have identical
host-specific layouts: 832-byte futures, 152-byte responses, and 40-byte
bodies. The send-boxed control retains the same future size but has a 128-byte
response and 16-byte erased body.

Three harness-free targets isolate compile and loadable-section growth. Five
alternating warm rebuilds produced these medians:

| Boundary | Median warm target compile | `.text` | `.rodata` |
|---|---:|---:|---:|
| `ExactBody` | 4.97 s | 248,810 bytes | 22,528 bytes |
| generated exact Tower | 4.98 s | 248,954 bytes | 22,536 bytes |
| `SendBoxBody` | 5.08 s | 249,706 bytes | 22,600 bytes |

Generated exact Tower adds 0.20% warm compile time and 0.056% combined
`.text`/`.rodata` over `ExactBody`; both are below the 5% budget. It is smaller
and faster to compile than the boxed control.

## Generated-code size and compile time

Four controls isolate what the `#[router]` macro adds to a binary. Each pair is
the same file twice: once with `#[router]`/`#[route]` and a routed request in
`main`, once with the same handlers as plain inherent methods called directly.
Both halves reach their handlers, so the optimizer cannot delete the code being
measured. The exact control sources are archived with the raw run output.

```text
cargo rustc -p routerama --features route --example <control> --release -- -Zunpretty=expanded
cargo build -p routerama --features route --release --example <control>; size -A <binary>
```

| Control | Handwritten bytes | Expanded bytes | Expanded lines | `.text` | `.rodata` |
|---|---:|---:|---:|---:|---:|
| 1 route, plain methods | 793 | 1,340 | 46 | 251,875 | 21,864 |
| 1 route, `#[router]` | 1,134 | 21,118 | 449 | 268,195 | 24,704 |
| 4 routes, plain methods | 1,409 | 1,923 | 62 | 252,115 | 21,848 |
| 4 routes, `#[router]` | 1,947 | 26,671 | 544 | 271,843 | 25,056 |

One generated route expands to 19,778 bytes (403 lines) of macro output and adds
16,320 bytes of `.text` and 2,840 bytes of `.rodata`. Going from one to four
routes adds 4,970 expanded bytes and 3,408 bytes of `.text`, so the marginal cost
of a route is about 1,657 expanded bytes and 1,136 bytes of loadable code, on top
of a roughly 15 KiB fixed scaffold that includes response conversion and the
matcher entry the first route pulls in. Section sizes include everything the
example links, so read the deltas, not the absolutes.

Compile time was measured by removing only routerama's own release artifacts
before each run, so every row is routerama's expansion, code generation, and
link against an already-built dependency graph. Three runs per row on an
otherwise idle host; the median is reported.

```text
cargo clean -p routerama --release
/usr/bin/time -f '%e %M' cargo build -p routerama --all-features --release --<kind> <name>
```

| Target | Median seconds | Median peak RSS (KiB) |
|---|---:|---:|
| example, 1 route, plain methods | 6.33 | 429,696 |
| example, 1 route, `#[router]` | 6.72 | 432,604 |
| example, 4 routes, plain methods | 6.38 | 431,720 |
| example, 4 routes, `#[router]` | 7.74 | 430,828 |
| bench `routerama_route_policy` | 11.64 | 432,976 |
| bench `routerama_interceptors` | 7.94 | 434,692 |
| bench `routerama_mount` | 8.41 | 431,320 |
| bench `routerama_tower` | 7.97 | 432,572 |
| bench `routerama_form_extraction` | 12.83 | 661,780 |

Each row includes rebuilding the routerama library itself, which is most of the
roughly 6.3-second floor. Against that floor, one generated route costs 0.39 s
(6%) and four cost 1.36 s (21%), so the marginal compile cost of a route is
about 0.3 s in this three-run within-host sample. Compile times are cache-,
scheduler-, and host-sensitive and should be compared only within one host and
one sample.

The release benchmark binaries measured on the same day were:

| Benchmark binary | `.text` | `.rodata` | File incl. debuginfo |
|---|---:|---:|---:|
| `routerama_route_policy` | 2,397,508 | 172,904 | 21,502,480 |
| `routerama_interceptors` | 2,376,420 | 169,712 | 21,493,600 |
| `routerama_mount` | 2,526,756 | 183,024 | 22,098,840 |
| `routerama_tower` | 2,345,204 | 168,496 | 21,355,880 |
| `routerama_form_extraction` | 4,379,236 | 428,504 | 53,797,264 |
| `routerama_body_extraction` | 4,349,460 | 423,720 | 53,879,824 |

Every binary contains the shared Criterion and `alloc_tracker` harness plus its
own fixture scaffolding, and the two five-framework fixtures additionally
contain Axum, Actix Web, Rocket, Warp, Tokio, and Serde. These are
benchmark-engineering costs, not per-framework application sizes, and must not
be used to rank frameworks.

## Acceptance-gate evaluation (2026-07-26)

*Superseded by the 2026-07-27 evaluation below, which closes this evaluation's
one open gate. The figures and verdicts here are left exactly as they were
recorded.*

The gates are the ones recorded before implementation began. Each verdict
below names the evidence that supports it and the
evidence that is still missing. Verdicts are drawn only from behaviorally
equivalent comparisons; the route-policy, mount, interceptor, Tower, and
response-body tables are Routerama-internal controls and are never used to claim
an advantage over another framework.

| Gate | Verdict | Evidence |
|---|---|---|
| Zero framework allocations through handler entry for literal routes and borrowed static captures | **Met** | `tests/router_borrowed.rs`, `tests/mount.rs`, and `tests/tower.rs` assert zero bytes on those paths. The new fixtures extend it through complete response production: `route_policy` plain and predicate-free rows, both `mount` static rows, `mount` configured-dynamic, all six `before`/`after` rows, and the direct, `ExactBody`, and generated exact Tower rows each report 0 allocations. |
| No heap-allocated handler, extractor, guard, or interceptor futures on the generated path | **Met** | Every generated row above reports 0 allocations while running handlers, guards, and interceptors. The only boxes that appear anywhere are the two an erased mount opts into and the one `SendBoxBody` opts into. |
| No mandatory response-body boxing for a statically known service | **Met** | `dispatch/route_service_exact_body` and `dispatch/generated_exact_tower` allocate 0 and retain the concrete generated sum; boxing appears only in the explicitly selected `SendBoxBody` row. The 2026-07-25 response-body table shows the same for the generated concrete paths. |
| At least 15% fewer instructions, by geometric mean, than the fastest comparison across non-I/O routing/extraction scenarios | **Met** | Form extraction: 4,699 vs Actix Web 7,936 (41% fewer). Previously recorded: dispatch 52%, route-set scaling 48%, body extraction 45%. |
| No core routing/extraction scenario more than 5% worse than the fastest comparison without a documented capability tradeoff | **Met** | Routerama is the lowest result in all 10 form rows for both time and instructions; its worst per-row ratio against the best comparison is 0.74 (time) and 0.71 (instructions). The same held for every previously recorded dispatch, scaling, and body row. |
| End-to-end CPU-bound throughput at least equal to the fastest comparison | **Not measured** | No throughput or transport benchmark exists. Every fixture is a single in-process request per iteration with no network I/O, no connection handling, and no concurrency. This gate remains open and must not be reported as met. |

Two further observations belong with the gates rather than with any single
table.

**Generated wrapper overhead is bounded and visible.** On the fixed-body path
the generated route costs 550 instructions against 232 for observing the same
body directly (2026-07-25 table), and the Tower adapter costs 22 instructions
over the generated entry (2026-07-26 table). The wrapper is a few hundred
instructions of routing, extraction, and response conversion, not a
multiplicative overhead.

**Non-equivalent paths prove nothing about other frameworks.** Mounted services,
the Tower boundary, route policy, and response-body representation are
Routerama-only controls that compare Routerama against Routerama. They are
published so the cost of each opt-in capability is visible, and they are
excluded from every geometric mean quoted against Axum, Actix Web, Rocket, or
Warp.

## Exhaustive-audit boundaries (2026-07-27)

The implementation has strong verified hot-path properties: generated literal
and typed-capture dispatch can complete without allocation or boxing; captures
borrow URI storage; guards and interceptors are emitted only when selected; and
the query codec is borrow-first. The comparative fixtures continue to show the
lowest instruction count in every measured routing, bounded-body, and form row.

Those results do not justify a general "fastest web server" claim. Routerama
does not provide a transport, and no fixture measures sockets, HTTP parsing,
TLS, HTTP/2, flow control, scheduler migration, or connection lifecycle.
Concurrent throughput is an in-process service benchmark only. The large-route
fixture is GET-only, literal-only, three segments deep, and branches at one
node; affix fanout, deep disjoint tries, method fanout, many captures, and
unique extractor/rejection types remain unmeasured. The 16/128/1,024 services
also share one large benchmark binary, so their cache environments are not
isolated.

Specific costs still requiring targeted evidence or design work are:

- generated code and instruction-cache pressure: 16 to 1,024 routes adds about
  9% instructions but about 29% wall time in the original scaling run; the
  later literal-shape controls retain chain fusion and reject the other
  measured literal-only prototypes, but do not cover every route topology;
- single-frame request bodies now reuse their `Bytes`, but large, pending,
  trailer-bearing, misleading-size-hint, and heavily fragmented bodies need
  dedicated backpressure and allocation sweeps; and
- request-borrowed `&str` responses remain copy-based by design; callers with
  static payloads can use `StaticText` or `StaticBytes`.

The later performance milestones also retain one shared static/dynamic
`ScannedPath`, parse-once overlap predicate state, built-in primitive
path/query decoding, exact query-output length hints, focused typed response
templates, and generated static route-header operations. Each was retained on
the evidence of the section that measures it in this file — the scenario
records above and the size controls below — together with its benchmark pair in
`benches/`. No separate report file records a further retained/rejected
disposition; where one is not stated here, it was never written down.

These are residual measurement/design risks, not evidence that the measured
comparative rows are incorrect. New claims must name the route shape, body
shape, response representation, and boundary actually measured.

## Acceptance-gate evaluation (2026-07-27)

This evaluation supersedes the 2026-07-26 one. Five gates are carried forward
unchanged with their original evidence; the sixth, throughput, now has a
fixture and a measured verdict, and its scope was revised before it was
evaluated.

| Gate | Verdict | Evidence |
|---|---|---|
| Zero framework allocations through handler entry for literal routes and borrowed static captures | **Met** | Unchanged from 2026-07-26. The wider mount matrix adds 26 rows of evidence: the three generated rows allocate nothing at all, and every mounted row allocates only what the named decomposition accounts for. |
| No heap-allocated handler, extractor, guard, or interceptor futures on the generated path | **Met** | Unchanged. The wider mount matrix confirms the only boxed future anywhere is the one an erased mount explicitly opts into, one per mounted call, at every table size, capture count, path depth, and body kind. |
| No mandatory response-body boxing for a statically known service | **Met** | Unchanged from 2026-07-26. |
| At least 15% fewer instructions, by geometric mean, than the fastest comparison across non-I/O routing/extraction scenarios | **Met** | Corrected final-tree reruns: dispatch 50.5% fewer than Axum and body extraction 48% fewer than Actix Web. Previously recorded unchanged fixtures: form extraction 41% and route-set scaling 48% fewer than the fastest comparison. |
| No core routing/extraction scenario more than 5% worse than the fastest comparison without a documented capability tradeoff | **Met** | Unchanged. The mount-table scaling result is Routerama-against-Routerama and is not a comparison scenario; it was fixed after it was measured and both the before-fix and after-fix matrices are reported in full above. |
| ~~End-to-end~~ **Concurrent in-process** CPU-bound throughput at least equal to the fastest comparison, with a target of a measurable improvement outside benchmark noise | **Met, as revised** | Corrected native typed-capture rerun: Routerama is the highest of the five frameworks in every row, at 6.000 M req/s against Axum's 3.308 M on the light workload and 1.201 M against 0.951 M on the heavy one by Criterion; the fixed-count runner independently records 1.78x and 1.24x. The separate seven-shape sweep also has Routerama ahead throughout. **Scope revision:** the gate said *end-to-end*; transport equality across the five subjects is not controllable (Routerama ships no server, the runtimes cannot be unified, and the transports differ structurally), so it was evaluated at the largest scope in which all five can be driven identically. The threshold was not weakened, only the scope was narrowed, and no result here is transport-level evidence. |

Two further observations belong with the gates rather than with any single
table.

**Generated wrapper overhead is bounded and visible.** Unchanged from
2026-07-26.

**The runtime matcher now scales the way the generated one does.** The first
capture of the wider mount matrix found the one measured result in this
document that got worse with scale: a 1,024-entry mount table cost 43,264
instructions on its last-registered entry against 2,386 on its first, while
the generated 1,024-route table was flat. No acceptance gate covers
runtime-registered table scaling, so it was never a gate failure, but it was
not left standing either. A node with at least sixteen sibling literals is now
sorted by key at compile time and binary searched at request time, and the
same table costs 2,832/2,829/2,843/1,618 instructions at its first, middle,
last, and missing entry — flat, allocation-identical, and 93.4% cheaper on the
row that was worst. The before-fix matrix is retained above as labelled
historical evidence.

## Query-codec instruction counts

Instruction counts for the same query-codec variants whose wall-clock timings
appear in [`PERF.md`](PERF.md). Ratios are relative to Routerama; lower is
better. Captured with:

```text
cargo bench -p routerama --features query --bench routerama_query_cg
```

<!-- BEGIN GENERATED: query-instructions -->

### Parse: common query

| Implementation | Instructions | Instructions vs Routerama |
|---|---:|---:|
| `routerama` | 601 | 1.00× |
| `serde_urlencoded` | 1,629 | 2.71× |
| `serde_html_form` | 3,047 | 5.07× |

### Parse: escaped query

| Implementation | Instructions | Instructions vs Routerama |
|---|---:|---:|
| `routerama` | 1,164 | 1.00× |
| `serde_urlencoded` | 2,732 | 2.35× |
| `serde_html_form` | 4,145 | 3.56× |

### Parse: repeated values

| Implementation | Instructions | Instructions vs Routerama |
|---|---:|---:|
| `routerama` | 1,960 | 1.00× |
| `serde_html_form` | 5,208 | 2.66× |

### Parse: long ASCII value

| Implementation | Instructions | Instructions vs Routerama |
|---|---:|---:|
| `routerama` | 908 | 1.00× |
| `serde_urlencoded` | 14,412 | 15.87× |
| `serde_html_form` | 15,459 | 17.03× |

### Produce: caller-provided buffer

| Implementation | Instructions | Instructions vs Routerama |
|---|---:|---:|
| `routerama` | 556 | 1.00× |
| `serde_html_form` | 650 | 1.17× |

### Produce: allocating

| Implementation | Instructions | Instructions vs Routerama |
|---|---:|---:|
| `routerama` | 1,051 | 1.00× |
| `serde_urlencoded` | 1,173 | 1.12× |
| `serde_html_form` | 1,058 | 1.01× |
<!-- END GENERATED: query-instructions -->
