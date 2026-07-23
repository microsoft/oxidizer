# Routerama HTTP Handler Plan

## Problem

`#[routerama::resolve::resolver]` is a complete routing primitive: it maps an
HTTP method and path to a typed route value.
`#[routerama::route::router]` originally invoked service methods but was not
yet a complete HTTP application boundary. Phases 0 through 6 closed that gap;
the findings under each phase record what shipped.

Before the narrow Phase 0 HTTP slice, generated routing accepted:

```text
service.route(method, path, context).await
router.route(&service, method, path, context).await
```

That temporary contract has now been replaced for `route` by
`http::Request<B>`, separate shared state, extraction, and direct HTTP
responses, as recorded in the Phase 0 findings below. The original model was
efficient and framework-neutral, but did not directly model:

- request headers, URI metadata, extensions, or protocol version;
- request bodies and the rule that a body can only be consumed once;
- query extraction as part of handler invocation;
- typed extraction failures that become HTTP responses;
- per-handler response types;
- ergonomic status, header, extension, and body composition; or
- middleware that enriches a request before handler extraction.

Applications can place request data in `context` and choose
`http::Response<Body>` as the common return type. That provides raw capability,
but every integration must invent its own conventions and error handling.

## Design goals

1. Preserve the current compiled path matching and direct handler calls.
2. Keep the resolver macro, trie, and query codecs usable in `no_std`.
3. Give HTTP handlers first-class access to request parts and bodies.
4. Guarantee that at most one handler argument consumes the body.
5. Allow every handler to return its own type as long as it can become an HTTP
   response.
6. Make extraction and routing failures produce explicit, customizable
   responses.
7. Support middleware and request-local typed extensions without creating a
   second incompatible ecosystem.
8. Keep path captures cheaper and more strongly checked than a generic
   deserialize-the-whole-path mechanism.
9. Avoid trait objects and per-request handler registries.
10. Produce actionable compile-time diagnostics for invalid handler
    signatures.
11. Reach functional parity with leading Rust web frameworks in what an
    application can express, without copying their public APIs or internal
    abstractions.
12. Prefer the simplest model that preserves that expressiveness and produces
    the fastest measured end-to-end request path.
13. Isolate optional flexibility costs: dynamic dispatch, boxing, type maps,
    and runtime route registration must not penalize requests that stay on the
    generated static path.

## Non-goals

- Mirror the API, trait structure, or composition model of any existing
  framework.
- Add HTTP request and body dependencies to Routerama's routing core.
- Replace typed resolution with an HTTP framework.
- Require Serde for path captures.
- Hide body buffering or permit unbounded buffering by default.
- Make all extractors dynamically dispatched.

## Comparison with leading framework designs

Axum, Actix Web, Rocket, and Warp represent four materially different Rust web
framework designs. They are capability and performance baselines, not API
templates. Routerama's goal is to express the useful application behaviors
found across all four through a simpler model with a faster measured request
path. Only technical expressiveness, flexibility, generated/runtime structure,
and measurable cost are considered.

### Axum's model

Axum routes a request to a value implementing `Handler`. Macro-generated trait
implementations support handlers with up to sixteen arguments:

- every argument except the last implements `FromRequestParts`;
- the last implements `FromRequest` and may consume the body;
- extractor rejections implement `IntoResponse` and short-circuit the handler;
  and
- the handler result implements `IntoResponse`.

Path routing stores captures in request extensions. A later `Path<T>`
extractor deserializes those captures, generally through Serde. Response tuples
compose status, headers, extensions, and a final body-producing value through
`IntoResponseParts` and `IntoResponse`. Tower supplies middleware and service
composition.

Relevant source:

- [handler implementation](https://github.com/tokio-rs/axum/blob/main/axum/src/handler/mod.rs);
- [request extraction](https://github.com/tokio-rs/axum/blob/main/axum-core/src/extract/mod.rs);
- [response conversion](https://github.com/tokio-rs/axum/blob/main/axum-core/src/response/into_response.rs);
- [response parts](https://github.com/tokio-rs/axum/blob/main/axum-core/src/response/into_response_parts.rs);
- [path routing](https://github.com/tokio-rs/axum/blob/main/axum/src/routing/path_router.rs); and
- [middleware](https://github.com/tokio-rs/axum/blob/main/axum/src/middleware/from_fn.rs).

### Capabilities required for functional parity

Routerama must support the useful behaviors represented by Axum's machinery,
but is not committed to the same trait decomposition or syntax:

- inspect request metadata without consuming the body;
- consume or stream the body exactly once;
- short-circuit extraction failures into responses;
- normalize heterogeneous handler results;
- compose response status, headers, extensions, and body;
- pass typed request-local data between middleware and handlers; and
- interoperate with transport and middleware services.

The `FromRequestParts`, `FromRequestBody`, `IntoResponse`, and
`IntoResponseParts` names used later in this plan are provisional mechanisms.
The Phase 0 spike must compare them with simpler generated alternatives. A
framework-compatible shape is not evidence in its favor; expressiveness,
clarity, generated code, and measured cost are.

### What is novel or differentiating in Routerama's model

Routerama combines that HTTP boundary with capabilities Axum does not provide
in the same form:

1. **The route template and handler signature are checked together.** Static
   capture names and types are known by the procedural macro, so missing,
   duplicated, or mismatched capture arguments fail during expansion.
2. **Static captures are direct handler arguments.** They do not need to be
   stored in extensions and then deserialized as an aggregate `Path<T>`.
   Borrowed `&str` captures can remain tied directly to the request path.
3. **Routing and dispatch are generated as one monomorphic path.** The selected
   enum variant is matched directly to an inherent service method without a
   handler registry or a blanket `Handler` implementation over every arity.
4. **Static and startup-configured dynamic routes share one typed model.**
   Static routes retain generated matching while explicitly dynamic handlers
   are registered through persistent builders.
5. **Capabilities are independently feature-gated.** `resolve`, `route`, and
   `query` can be used separately, while the matching and query core remain
   suitable for `no_std`.
6. **Service methods remain the source of truth.** Route declarations,
   captures, and implementation stay together rather than being split between
   a router construction expression and unrelated free functions.
7. **The macro can generate an exact extraction plan per handler.** It does not
   need a universal handler-arity trait, a tuple/HList composition engine, or a
   boxed guard future merely to sequence statically known parameters.
8. **Request-local indirection can be opt-in.** Explicit state and capture
   arguments can avoid a `TypeId` map lookup, while extensions remain available
   when middleware genuinely needs open-ended typed storage.
9. **Response erasure can be specialized per service.** A generated body enum
   can represent the finite set of handler and rejection body types without
   unconditionally boxing every response body. A boxed fallback can remain
   available for genuinely dynamic composition.

The combination is differentiating, but not automatically better. Generated
matching can increase binary size and instruction-cache pressure, service
methods are less freely composable than function handlers, and direct capture
arguments require more macro grammar.

### Where Routerama should be better

These are intended advantages that must be demonstrated:

| Area | Expected Routerama advantage |
|---|---|
| Static matching | Generated matcher with no runtime route-table construction |
| Path captures | Direct, named, compile-time-checked arguments |
| Borrowing | Zero-copy `&str` captures without a Serde-owned intermediate |
| Dispatch | One enum match followed by a direct method call |
| Route/schema drift | Template and handler signature validated together |
| Minimal deployments | `resolve` and `query` remain independent and `no_std` |
| Dynamic registration | Runtime aliases coexist with statically compiled routes |
| API locality | Route declaration and handler implementation occupy one method |

Performance claims must remain hypotheses until measured against equivalent
applications in all comparison frameworks. In particular, an optimizing
compiler may remove more abstraction cost than source-level inspection
suggests.

### Capabilities Axum currently expresses better

Axum has substantial advantages that Routerama should acknowledge rather than
attempt to erase:

| Area | Axum advantage |
|---|---|
| Integrations | Tower layers and extractors for authentication, tracing, cookies, multipart, WebSockets, and more |
| Handler forms | Free functions, closures, values, and arbitrary Tower services |
| Composition | Nested routers, fallbacks, method routers, route merging, and per-handler layers |
| Extraction | Broad built-in catalog plus third-party custom extractors |
| Responses | Comprehensive tuple composition and detailed conversion-failure behavior |
| Diagnostics | `debug_handler`, targeted trait diagnostics, and structured rejection types |
| Transport support | Existing Hyper and Tower integration |
| Flexibility | Runtime route construction without generated-code growth per route |

Axum's `Path<T>` also becomes more convenient when a handler wants a nested
Serde data structure rather than individual named captures. Routerama should
not force direct arguments where aggregate extraction is clearer.

### Actix Web's model

Actix Web constructs its application at runtime. Its router stores resources
in registration order and scans candidate resources, using literal matching
for static resources and regular expressions for dynamic resources. Guards
are additional runtime predicates. Handlers implement a generated `Handler`
trait and are stored behind boxed service factories; request extractors
implement `FromRequest`, and all receive the same mutable payload.

Actix supports:

- runtime route, scope, resource, and guard composition;
- regex-constrained captures and tail captures;
- layered application data selected by type and scope;
- request-local extension data;
- open third-party extractors;
- `Responder`, status/header customization, and multiple body forms; and
- `Transform`/`Service` middleware.

Its single mutable payload permits arbitrary extractor ordering, but the
one-body-consumer rule is a convention enforced at request time rather than by
the handler type. Path extraction deserializes named captures at request time.
The heterogeneous runtime route table and middleware graph require boxed
services, boxed futures, and body normalization at service boundaries.

Routerama should match Actix's expressiveness for custom guards, scoped state,
request-local data, response customization, and middleware without inheriting
linear route scans, regex matching for ordinary captures, name-based capture
lookup, or unconditional handler boxing.

Relevant source:

- [router](https://github.com/actix/actix-web/blob/master/actix-router/src/router.rs);
- [resource matching](https://github.com/actix/actix-web/blob/master/actix-router/src/resource.rs);
- [handlers](https://github.com/actix/actix-web/blob/master/actix-web/src/handler.rs);
- [extractors](https://github.com/actix/actix-web/blob/master/actix-web/src/extract.rs);
- [responders](https://github.com/actix/actix-web/blob/master/actix-web/src/response/responder.rs); and
- [middleware](https://github.com/actix/actix-web/blob/master/actix-web/src/middleware/mod.rs).

### Rocket's model

Rocket is the closest conceptual comparison to Routerama because route
attributes generate handler adapters and validate capture names. The generated
adapter evaluates path/query captures, request guards, and one data guard
before calling the user function and converting its `Responder`.

Rocket additionally supports:

- request guards as typed authorization or validation capabilities;
- one body/data guard;
- route ranking and content-type/accept negotiation;
- managed state and request-local cached values;
- catchers for status-specific failure handling;
- fairings for request/response lifecycle hooks;
- sentinels that validate required application state; and
- typed streaming, SSE, and file responses.

Rocket's dispatch remains runtime-oriented: routes are stored in ranked vectors
and candidates are scanned per method; each route stores a boxed handler;
request and data guards use boxed async-trait futures; request-local caching
uses a type map; and response streams use boxed readers. Route collisions are
detected during application ignition rather than compilation.

Routerama should adopt the expressive ideas of guard transparency, format
negotiation, catchers, required-state validation, and ergonomic streaming. Its
generated route graph can potentially provide those capabilities with
compile-time collision checking, direct guard calls, and no mandatory
handler/guard/body boxing.

Relevant source:

- [route code generation](https://github.com/rwf2/Rocket/blob/master/core/codegen/src/attribute/route/mod.rs);
- [runtime router](https://github.com/rwf2/Rocket/blob/master/core/lib/src/router/router.rs);
- [handler abstraction](https://github.com/rwf2/Rocket/blob/master/core/lib/src/route/handler.rs);
- [request guards](https://github.com/rwf2/Rocket/blob/master/core/lib/src/request/from_request.rs);
- [data guards](https://github.com/rwf2/Rocket/blob/master/core/lib/src/data/from_data.rs);
- [responders](https://github.com/rwf2/Rocket/blob/master/core/lib/src/response/responder.rs); and
- [fairings](https://github.com/rwf2/Rocket/blob/master/core/lib/src/fairing/mod.rs).

### Warp's model

Warp models the entire application as a type-level graph of `Filter`
combinators. Filters extract tuples, reject, recover, transform values, and
compose sequentially with `and` or alternatively with `or`. Closure signatures
are checked against the resulting extraction tuple.

Warp's model is highly expressive for:

- reusable extraction and authorization pipelines;
- arbitrary composition of path, method, header, query, and body filters;
- rejection accumulation and recovery;
- filter-level wrappers;
- streaming bodies and replies; and
- constructing new domain-specific routing combinators as ordinary Rust
  values.

The composed graph is also the runtime router. Alternative branches are tried
in source order and rewind a mutable request path cursor when backtracking.
Large generic filter graphs produce deeply nested types and futures; boxing is
the standard escape hatch, trading type growth for virtual dispatch and a
boxed future.

Routerama should match Warp's ability to define reusable extraction,
authorization, transformation, and recovery components, but express them as a
compile-time handler plan rather than as the route-search data structure. This
separates reusable request processing from efficient indexed route selection.

Relevant source:

- [filter trait and combinators](https://github.com/seanmonstar/warp/blob/master/src/filter/mod.rs);
- [`and` composition](https://github.com/seanmonstar/warp/blob/master/src/filter/and.rs);
- [`or` and backtracking](https://github.com/seanmonstar/warp/blob/master/src/filter/or.rs);
- [boxed filters](https://github.com/seanmonstar/warp/blob/master/src/filter/boxed.rs);
- [rejections](https://github.com/seanmonstar/warp/blob/master/src/reject.rs);
- [replies](https://github.com/seanmonstar/warp/blob/master/src/reply.rs); and
- [body filters](https://github.com/seanmonstar/warp/blob/master/src/filters/body.rs).

### Cross-framework capability target

Functional parity means Routerama can express each behavior, not that it uses
the same syntax or runtime structure:

| Capability | Axum | Actix Web | Rocket | Warp | Routerama target |
|---|---|---|---|---|---|
| Typed path values | `Path<T>` | `Path<T>` | `FromParam` | path filters | Direct captures plus optional aggregate extraction |
| Request metadata | Parts extractors | `FromRequest` | request guards | filters | Generated non-body extraction plan |
| Body ownership | One final extractor | Shared payload convention | One data guard | Take-once body filter | Explicit `#[body]` parameter checked at compile time |
| Custom validation/auth | Extractors/middleware | Extractors/guards | Request guards | filters | Reusable typed guards with direct calls |
| State | `State`/extensions | scoped `Data`/extensions | managed state/cache | injected filters | Explicit typed state plus opt-in extensions |
| Failure handling | rejections | errors/responses | outcomes/catchers | rejections/recover | Typed rejection policy and generated short-circuiting |
| Response conversion | `IntoResponse` | `Responder` | `Responder` | `Reply` | Heterogeneous returns with specialized body normalization |
| Middleware | Tower layers | transforms/services | fairings | wrappers | Generated static layers plus optional Tower boundary |
| Content negotiation | extractors/routing | guards | route formats | filters | Compile-time route predicates for media types |
| Streaming/SSE | body types | body types | stream responders | replies | Generic streaming bodies without mandatory boxing |
| Runtime route additions | router construction | app construction | mounting | filter construction | Explicit dynamic paths plus opt-in mounted dynamic services |
| Fallback/recovery | fallbacks | default services | catchers | recover | Typed fallback and rejection routing |

### Performance opportunities exposed by the comparison

The generated model should be designed to remove work that each comparison
framework performs on its request path:

| Cost to avoid | Seen in | Routerama strategy |
|---|---|---|
| Route-table scans or ordered branch retries | Actix Web, Rocket, Warp | Generated static decision tree or compiled dynamic trie |
| Regex matching for ordinary captures | Actix Web | Segment classification and typed conversion |
| Capture storage followed by name lookup/deserialization | Axum, Actix Web | Direct positional capture binding validated against names at compile time |
| Boxed handler or guard futures | Actix Web, Rocket, boxed Warp filters | Generated direct awaits using concrete future types |
| Universal handler arity machinery | Axum, Actix Web, Warp tuples | Per-handler generated extraction code with no artificial arity ceiling |
| Mandatory type-map state lookup | Actix Web, Rocket; optional in Axum | Explicit typed state; extensions only when requested |
| Unconditional body type erasure | Actix Web service boundary, Rocket | Generated service-specific body enum; explicit boxed escape hatch |
| Runtime route collision discovery | Rocket | Compile-time static validation and fallible dynamic builder validation |
| Runtime enforcement of one body consumer | Actix Web, Warp | Macro-enforced single consuming position |

These are architectural opportunities, not benchmark results. Every claimed
removal must be verified in generated code and measured end to end.

### Existing performance evidence and its limit

Routerama already has evidence that compile-time specialization can pay:

- the current shared routing sweep reports `routerama_static` at 15,284
  instructions versus 27,902 for `matchit`, with both driven to the same typed
  route and converted-capture result; and
- generated query parsing reports between 2.34 and 15.87 times fewer
  instructions than `serde_urlencoded` across the documented parsing
  workloads.

These results are recorded in [`docs/PERF.md`](docs/PERF.md) and are generated
by this repository's benchmark harness. They establish direction, not
full-framework superiority: they do not include handler extraction, guards,
middleware, response conversion, body handling, transport, or direct Actix
Web/Rocket/Warp applications. The `route` design must preserve these savings
through the complete pipeline rather than spending them on a framework layer.

### Simplicity target

Routerama should expose fewer concepts than the union of these frameworks:

1. A route method declares matching, captures, guards/extractors, and handler
   logic in one place.
2. The macro lowers that declaration to a straight-line extraction and
   invocation plan.
3. A reusable extractor or guard is an ordinary typed operation, not part of
   the route-search graph.
4. Handler results convert into responses, with a generated unboxed body union
   when practical.
5. Middleware uses the same typed operations before and after invocation;
   Tower adaptation occurs at the outer boundary rather than defining the
   internal model.

No artificial handler-argument limit should be introduced by tuple-generation
macros. Limits should arise only from explicit resource policies such as body
size or recursion depth.

### Flexibility without a universal hot-path tax

Some parity requirements inherently need runtime openness: plugin-provided
handlers, arbitrary Tower services, type-erased request data, or body types
unknown to the macro. Routerama should support these through explicit boundary
types:

- generated handlers and interceptors remain concrete and directly called;
- runtime-configured paths to statically known handlers use the existing
  dynamic trie without boxing the handler;
- arbitrary runtime handlers or sub-services can be mounted behind an explicit
  dynamic-service adapter;
- unknown response bodies can opt into `BoxBody`; and
- open request-local data can opt into `Extensions`.

The generated route graph should mark these boundaries and enter them only
after static matching determines they are needed. A static request must not
perform a vtable call, allocate a box, or query a type map merely because some
other branch enables a dynamic capability.

### Cost and risk of building this machinery

The largest cost is not the traits themselves. It is maintaining coherent
behavior across:

- body streaming, buffering, limits, trailers, and transport errors;
- response body normalization and error precedence;
- extractor ordering, optional extraction, and custom rejection handling;
- middleware and service readiness;
- compiler diagnostics for invalid async handler signatures;
- feature combinations and generated paths under renamed dependencies; and
- compatibility with the surrounding `http`, `http-body`, Tower, and Hyper
  ecosystem.

Custom extraction traits create an interoperability boundary: extractors,
guards, filters, and responders written for another framework do not
automatically work with Routerama. Routerama must provide a small complete
foundation and low-friction adapters without reproducing every framework's
surface.

### Investment test

Development should proceed as a sequence of evidence-producing milestones,
not as a commitment to the complete extractor catalog.

The Phase 0 vertical slice must compare behaviorally equivalent Routerama,
Axum, Actix Web, Rocket, and Warp handlers for at least:

1. a static literal route;
2. a static route with borrowed and parsed captures;
3. method, header, and query extraction;
4. a bounded body-consuming extractor;
5. a response with a non-default status and headers; and
6. a complete miss and a capture-conversion failure;
7. route sets of 16, 128, and 1,024 routes with first, middle, last, and miss
   lookups;
8. zero, one, and four guards/extractors;
9. zero, one, and four before/after interceptors; and
10. fixed, streaming, and SSE response bodies;
11. host/media predicates, fallback recovery, and required-state checks; and
12. a body-transform interceptor followed by a handler body extractor.

Measure:

- instructions with Callgrind;
- branches, branch misses, and cache behavior where tooling supports them;
- allocations on the successful request path;
- steady-state latency and throughput;
- generated binary size;
- compile time and generated-code size; and
- application code and diagnostic quality for common mistakes.

Before implementation, record final acceptance thresholds and benchmark
controls, including identical transport/runtime configuration, release flags,
LTO settings, workloads, and response payloads. The initial gates are:

- zero framework allocations through handler entry for literal routes and
  borrowed static captures;
- no heap-allocated handler, extractor, guard, or interceptor futures on the
  generated path;
- no mandatory response-body boxing for a statically known service;
- at least 15% fewer instructions, by geometric mean, than the fastest
  comparison implementation across non-I/O routing/extraction scenarios;
- no core routing/extraction scenario more than 5% worse than the fastest
  comparison without a documented capability tradeoff; and
- ~~end-to-end~~ **concurrent in-process** CPU-bound throughput at least equal
  to the fastest comparison, with a target of a measurable improvement outside
  benchmark noise.

**Scope revision to the throughput gate (2026-07-27), recorded with its
justification rather than applied quietly.** The gate originally said
*end-to-end*. Transport equality across the five comparison subjects is not
controllable, so an end-to-end measurement would have compared five different
HTTP server implementations instead of the thing under test: Routerama ships
no server at all (a loopback fixture would pit repository-authored hyper glue
against four upstream servers, with the fixture author controlling one side);
the runtimes cannot be unified, because Actix Web serves on `actix-rt`
workers, Rocket drives its own runtime, and Routerama's generated futures and
Actix Web's handlers are both `!Send` by design; and the transports differ
structurally, so the result would be dominated by parser, buffer-pool, and
syscall behavior. The gate is therefore evaluated at the largest scope in
which all five subjects can be driven identically: complete in-process
dispatch, extraction, an identical deterministic CPU-bound handler, response
conversion, and complete response-body observation, run concurrently on a
share-nothing thread-per-core runtime. **The threshold was not weakened, only
the scope narrowed**, and no result may be reported as transport-level
evidence. `docs/PERF.md` records the fixture, both measurement methods, the
shape sweep, and the host limitations.

The 15% instruction target is intentionally adoption-oriented: a new framework
needs a meaningful advantage, not a statistical tie. It applies to the
geometric mean of routing and extraction scenarios where Routerama's generated
design can remove framework work; it does not require a 15% win in payload
serialization or network-I/O-dominated measurements. The threshold must be
revisited with recorded evidence after the first equivalent fixtures, not
quietly weakened to make the prototype pass.

**Gate status (2026-07-27):** all six gates are met, the sixth as revised
above. `docs/PERF.md` holds the per-gate verdict table, the evidence for each
verdict, and the exact commands and toolchain that produced it. The throughput
gate now has a fixture: Routerama is the highest of the five frameworks in
every measured row, 1.81x Axum's requests per second when the handler's work
is comparable with dispatch cost and 1.23x when it is ten times heavier, with
non-overlapping Criterion confidence intervals in both. The 15% instruction
threshold was **not** revised: every equivalent fixture measured so far clears
it by a wide margin (41% on forms, 45% on bodies, 48% on route-set scaling, 52%
on dispatch), so there is no evidence that would justify moving it. One
measured result got *worse* with scale, and was fixed rather than dropped: a
runtime-registered mount table was matched by a per-node scan of sibling
literals, so a 1,024-entry table cost 18x more instructions on its
last-registered entry than on its first. Nodes with at least sixteen sibling
literals are now sorted at compile time and binary searched, which makes the
same table flat across positions at 2,832/2,829/2,843 instructions; the
before-fix and after-fix matrices are both in `docs/PERF.md`.

Serialization- or network-dominated scenarios may converge, but Routerama must
not erase its routing/extraction advantage before user work begins. A synthetic
route-lookup win that disappears once headers, guards, and response conversion
are included is insufficient.

Proceed beyond the vertical slice only if Routerama meets the performance gates
and demonstrates functional parity for the tested behaviors. Stronger
compile-time validation, simpler service-oriented code, and independent
`no_std` capabilities are additional requirements, not substitutes for the
performance objective.

If the speed advantage disappears end to end, the model is not simpler, or
functional parity requires pervasive boxing/type maps, stop before building the
broad extractor catalog. In that outcome, retain the focused resolver and
provide transport-framework adapters.

## Crate and feature boundary

Keep the cohesive public application API in the `routerama` crate. Organize
the four proven public capabilities into canonical modules with no crate-root
re-exports:

| Feature | Public module | Contents |
|---|---|---|
| `resolve` | `routerama::resolve` | typed resolution, errors, builders, and the `resolver` macro |
| `response` | `routerama::response` | standalone HTTP bodies, response conversion, and metadata composition |
| `route` | `routerama::route` | HTTP handlers, request extraction, generated dispatch, and the `router` macro; implies `response` |
| `query` | `routerama::query` | query codecs and derive macros |

This is a starting boundary, not a rule that every future mechanism must be
absorbed by `route`. Every implementation phase must identify capability
subsets that:

1. are useful without HTTP handler dispatch;
2. can avoid dependencies required by the larger routing stack;
3. have a coherent public contract and tests of their own; and
4. do not force users to understand internal Routerama code generation.

Prefer a feature-gated `routerama` module when the capability shares
Routerama's public vocabulary and release lifecycle. Consider a separate crate
only when the capability is framework-neutral, independently useful, and has a
meaningfully smaller dependency or portability envelope. A crate split must
improve the user-facing dependency graph or reuse boundary; moving private
implementation files into another package is not sufficient.

Candidate boundaries to evaluate during the spike are:

| Capability | Initial home | Independent value and decision |
|---|---|---|
| Path-template parsing | existing `http_path_template` crate | Already framework-neutral and independently useful; keep Routerama-specific policy out of it |
| Matching engine | private shared engine | A possible small `no_std` crate if it can expose compiled and runtime matching without resolver macros, HTTP request types, or Routerama-specific generated symbols |
| Typed resolution | `routerama::resolve` | Useful by itself for framework adapters, dispatch tables, protocol routers, and tests; must remain independent of `route` and `query` |
| Query codecs | `routerama::query` | Useful for URI/form tooling without routing; retain its independent feature and derive surface |
| Response construction | `routerama::response` | Complete standalone capability selected by `response`; route consumes the same canonical API and adds no response re-exports |
| Request extraction | initially `routerama::route::extract` | Keep parts/body ownership planning separate from matching internally; promote it only if custom transports can use it without the router macro |
| Route predicates and policy | initially `routerama::route` submodules | Keep guards, fallbacks, and ranking composable and benchmarkable independently even if their public API remains under `route` |
| Transport and middleware adapters | additive modules/features | Tower, Hyper, and other adapters must not enter the base `route` dependency graph and need not share one feature. Delivered as `routerama::route::tower` behind the `tower` feature |

The strongest potential new-crate candidate is the matching engine currently
split awkwardly between `routerama` runtime files and `routerama_build`'s
framework-neutral trie. During the module migration, document its dependency
and generated-code boundary, but do not extract it until the Phase 0 spike
proves a public API that is useful outside Routerama. The response foundation
passed that test after its body representation and fallible-parts semantics
were settled. It is now a top-level module enabled by `response`, with the
dependency evidence recorded in the Phase 1 findings.

Avoid a crate per trait or per integration. Fine-grained crates increase
version coordination, documentation surface, compile graph complexity, and
generated-path risk. Independence is valuable only when users can obtain a
complete capability without pulling in an unrelated universe.

Use an empty default feature set so that activating no features intentionally
produces only the crate shell. `response`, `resolve`, and `query` are
independently selectable; `route` deliberately layers on `response`, `json`
deliberately layers on `route`, and `form` deliberately combines `route` with
the independently useful `query` codec:

```toml
[features]
default = []
form = ["route", "query"]
json = ["route", "dep:serde", "dep:serde_json"]
mount = ["route"]
query = ["dep:itoa", "dep:routerama_macros", "routerama_macros/query"]
resolve = [
    "dep:http_path_template",
    "dep:routerama_build",
    "dep:routerama_macros",
    "dep:smallvec",
    "routerama_macros/resolve",
]
response = [
    "dep:bytes",
    "dep:http",
    "dep:http-body",
    "dep:pin-project-lite",
]
route = [
    "dep:http_path_template",
    "dep:routerama_build",
    "dep:routerama_macros",
    "dep:smallvec",
    "response",
    "routerama_macros/route",
]
tower = ["dep:tower-service", "route"]
```

The macro crate should expose matching internal features so each public
feature compiles only its own procedural entry points. Dependencies required
only by JSON, forms, Tower, or other integrations remain behind narrower
additive features.

The source layout should be:

```text
src/
  lib.rs
  response.rs
  response/
    body.rs
  resolve.rs
  route.rs
  route/
    extract.rs
    form.rs
    json.rs
  query/
    mod.rs
    ...
```

`lib.rs` should expose only the selected modules:

```rust,ignore
#[cfg(feature = "resolve")]
pub mod resolve;

#[cfg(feature = "response")]
pub mod response;

#[cfg(feature = "route")]
pub mod route;

#[cfg(feature = "query")]
pub mod query;
```

Each module re-exports its own macro rather than relying on the crate root:

```rust,ignore
// routerama::resolve
pub use routerama_macros::resolver;

// routerama::route
pub use routerama_macros::router;

// routerama::query
pub use routerama_macros::{FromQuery, ToQuery};
```

Typical attribute paths are therefore:

```rust,ignore
#[routerama::resolve::resolver]
enum ApiRoute {
    // ...
}

#[routerama::route::router]
impl Api {
    // ...
}
```

There must be no root re-exports of module traits, errors, macros, or helper
types. For example, users import `routerama::resolve::Resolver` and
`routerama::response::IntoResponse`, not `routerama::Resolver` or
`routerama::IntoResponse`. `route` also does not duplicate the canonical
response API through compatibility re-exports.

The existing root API should move as follows:

| Existing symbol | Canonical location |
|---|---|
| `HttpMethod` | `routerama::resolve::HttpMethod` |
| `Resolver` | `routerama::resolve::Resolver` |
| `ResolveError` | `routerama::resolve::ResolveError` |
| `ConfigurationError` | `routerama::resolve::ConfigurationError` |
| `resolver` attribute | `routerama::resolve::resolver` |
| `router` attribute | `routerama::route::router` |
| response bodies and traits | `routerama::response::{Body, Response, IntoResponse, IntoResponseParts, ...}` |
| `FromQuery` and `ToQuery` derives | `routerama::query::{FromQuery, ToQuery}` |

HTTP routing should use `http::Method` at its public boundary rather than
re-exporting `resolve::HttpMethod`. Shared internal method matching can convert
or borrow the HTTP method without coupling the public modules.

Generated-code support must also live under the owning module. Replace root
`codegen_helpers` and `__rt` exports with documented-hidden namespaces such as
`resolve::__private`, `route::__private`, and `query::__private`. The
featureless crate must not expose generated-code support for disabled
capabilities.

The featureless shell contains crate-level documentation and private
configuration scaffolding, but no useful public types. Shared matching
implementation should compile only when either `resolve` or `route`
needs it:

```rust,ignore
#[cfg(any(feature = "resolve", feature = "route"))]
mod engine;
```

This lets `route` reuse the matcher without activating or exposing the
`resolve` module. Likewise, `route` must not activate `query`; it activates
only the canonical `response` capability required by every generated handler
response.
`routerama::route::Query<T>` is available only when both `route` and `query`
are active.

A no-default-feature build must not compile macros, matching code, query code,
`http`, `http-body`, or body utilities. A `response`-only build compiles just
Routerama plus the direct `bytes`, `http`, `http-body`, and
`pin-project-lite` envelope. The crate may remain declared `#![no_std]`;
the `std` support selected for HTTP types is explicit in the feature
documentation and confined to `response` and its `route` dependent.

`routerama_build` continues to own macro lowering. Generated HTTP code should
refer to request and matcher support under `routerama::route` and response
contracts under `routerama::response`; generated resolver code should refer to
`routerama::resolve`, and generated query code should refer to
`routerama::query`. All paths must use the existing renamed-dependency runtime
lookup.

There should be only one `router` macro contract. The current context-only
macro should evolve into the HTTP-aware form and be available only as
`routerama::route::router` with `route`.

## Proposed HTTP boundary

The generated entry point should consume an HTTP request rather than separate
method and path strings:

```rust,ignore
pub async fn route<B, S>(
    &self,
    request: http::Request<B>,
    state: &S,
) -> routerama::response::Response<impl http_body::Body<Data = bytes::Bytes>>;
```

For dynamic routes:

```rust,ignore
pub async fn route<B, S>(
    &self,
    service: &Service,
    request: http::Request<B>,
    state: &S,
) -> routerama::response::Response<impl http_body::Body<Data = bytes::Bytes>>;
```

The generated method should:

1. read the method and path from the request;
2. resolve the route while retaining the complete request;
3. expose captures to the selected handler;
4. split the request into parts and body;
5. run non-body extractors from left to right;
6. run at most one body extractor;
7. call the handler directly; and
8. convert its result with `IntoResponse`.

The URI query must remain attached to the request, while routing continues to
match only the path component.

## Request extraction

The recommended candidate distinguishes metadata extraction from the one
explicit body-consuming parameter:

```rust,ignore
pub trait FromRequestParts<S>: Sized {
    type Rejection: IntoResponse;

    fn from_request_parts(
        parts: &mut http::request::Parts,
        state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send;
}

pub trait FromRequestBody<S, B>: Sized {
    type Rejection: IntoResponse;

    fn from_request_body(
        parts: &mut http::request::Parts,
        body: B,
        state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send;
}
```

`FromRequestParts` cannot consume the body. `FromRequestBody` owns the body and
may consume or stream it while still inspecting request metadata. A full
`Request<B>` extractor reconstructs the request from those inputs.

Unlike Axum, body ownership is explicit rather than positional:

```rust,ignore
async fn create(
    &self,
    headers: HeaderMap,
    #[body] document: Json<Document>,
    state: State<AppState>,
) -> Created<Document>
```

The `#[body]` parameter may appear wherever it reads most clearly in the
handler signature. The macro permits at most one, extracts all request-parts
parameters in declaration order, then transfers the body exactly once.

Handler arguments should be classified as follows:

1. `&self` is the service receiver.
2. Static parameters whose names match template captures remain direct,
   compile-time-validated path captures.
3. Dynamic route captures must be declared explicitly, because an unannotated
   parameter can no longer be assumed to be a capture. A parameter marker such
   as `#[capture] name: String` is preferable to encoding capture names in the
   dynamic route registration API.
4. A parameter marked `#[body]` must implement `FromRequestBody`.
5. Every remaining parameter must implement `FromRequestParts`.

Direct capture parameters should be allowed anywhere in the handler's
parameter list. Capture, parts-extractor, and body-extractor values are placed
back into the handler's original argument order for the direct call.

Initial built-in extractors should cover:

- `Method`;
- `Uri`;
- `Version`;
- `HeaderMap`;
- complete request `Parts`;
- complete `Request<B>`;
- raw body `B`;
- request `Extensions`;
- `State<T>` using a `FromRef<S>`-style projection;
- `Extension<T>`;
- `Query<T: routerama::query::FromQuery>`;
- raw query text;
- typed and raw path-capture views; and
- bounded `Bytes`, `String`, and JSON extractors behind appropriate features.

Body-buffering extractors must have a conservative default limit and an
explicit override mechanism.

## Context and state

The existing context model supports owned, shared, and mutable values. The HTTP
model needs a clearer separation among:

- long-lived application state;
- mutable per-request metadata;
- the request itself; and
- path captures.

The recommended model is:

- immutable application state supplied as `&S`;
- request-local mutable data stored in `http::Extensions`;
- headers, method, URI, and body obtained through extractors; and
- path captures passed directly.

Shared mutable application state should use interior mutability chosen by the
application. This avoids sequential extractors receiving unrestricted mutable
access to global state.

If preserving the current explicit context argument is required, treat it as a
separate handler argument rather than as the extraction mechanism. Extractors
should still receive only a shared state reference. This decision needs an API
spike before stabilizing the handler grammar.

## Responses

Keep response bodies generic through handler conversion:

```rust,ignore
pub type Response<B> = http::Response<B>;

pub trait IntoResponse {
    type Body: http_body::Body;

    fn into_response(self) -> Response<Self::Body>;
}
```

Handlers should no longer be required to return the same concrete type. Each
handler return type only needs to implement `IntoResponse`. The generated router knows the finite set of response sources, so it can
generate a private service-specific body enum:

```rust,ignore
enum ServiceBody<B0, B1, B2> {
    List(B0),
    Create(B1),
    Rejection(B2),
}
```

The enum implements `http_body::Body` by delegating directly to its active
variant. This gives the generated Tower service one concrete response type
without a body trait object or per-response box. Dynamic route paths do not
prevent this optimization because their handler set is still statically known.

The response-source analysis must include:

- handler return types;
- extractor and guard rejections;
- routing failures and fallbacks;
- generated interceptor short-circuits; and
- one boxed/dynamic variant for explicitly mounted runtime services.

Variants should be deduplicated by concrete body type where macro-visible type
identity permits it, not generated blindly per route. Phase 0 must compare
body-type deduplication, per-source variants, and boxed normalization at 16,
128, and 1,024 routes. Phase 1 must not stabilize the body representation until
handlers, rejections, interceptors, fallbacks, and mounted dynamic services are
all represented in the prototype.

Provide an explicit `BoxBody` conversion for applications that need open-ended
runtime composition. Boxing is a chosen interoperability boundary, not the
default internal representation. Phase 0 must compare generated body enums
against a normalized boxed body for instructions, allocations, response size,
binary size, and compile time.

Initial implementations should include:

- `Response<B>`;
- `StatusCode`;
- `()`;
- text strings and byte buffers;
- `Result<T, E>` where both sides implement `IntoResponse`, using a concrete
  two-variant body when their body types differ;
- JSON and form wrappers behind features; and
- `(StatusCode, R)`.

Add an `IntoResponseParts` trait for response metadata:

```rust,ignore
pub trait IntoResponseParts {
    type Error: IntoResponse;

    fn into_response_parts(
        self,
        response: ResponseParts,
    ) -> Result<ResponseParts, Self::Error>;
}
```

Support tuple composition where the final item supplies the body and preceding
items modify status, headers, or extensions:

```rust,ignore
(
    StatusCode::CREATED,
    [(LOCATION, "/books/42")],
    Json(book),
)
```

Header conversion failures must retain their own error status rather than
being accidentally overwritten by a surrounding success status.

## Rejections and routing failures

Every extractor rejection must implement `IntoResponse`. Generated code should
stop at the first failed extractor and return its response without invoking the
handler.

Provide explicit built-in rejection categories for:

- missing or invalid headers;
- missing extensions;
- query decoding;
- body size limits;
- body transport errors;
- UTF-8 decoding;
- JSON or form decoding; and
- path capture conversion.

The HTTP layer must also define the mapping from routing failures:

| Routing result | Default HTTP response |
|---|---|
| no matching route | `404 Not Found` |
| malformed request path | `400 Bad Request` |
| invalid or undecodable capture | `400 Bad Request` |
| internal configuration invariant | `500 Internal Server Error` |

Defaults should be overridable without requiring every handler to repeat the
mapping. Candidate mechanisms are a `RouteRejectionHandler` trait on router
configuration or a middleware layer. This must be decided before exposing the
HTTP `route` signature.

For applications needing programmatic routing errors, consider a lower-level
`try_route` API, but do not add it automatically unless there is a demonstrated
use case: every generated associated method increases the user's symbol
surface.

## Middleware and extensions

Use explicit typed state for statically known application data.
`http::Extensions` remains the open request-local interchange for
authentication, tracing, and transport adapters, but handlers that do not
request extensions should pay no type-map lookup cost.

Support two composition levels:

1. **Generated guards/interceptors** known to the macro. These run before or
   after selected handlers as direct concrete calls, can short-circuit with a
   response, and do not require boxed services or futures.
2. **Transport middleware** outside the generated router. Adapters to and from
   Tower `Service`, behind a `tower` feature, allow arbitrary existing layers
   where runtime-open composition is required.

Phase 0 must determine the smallest generated interceptor contract that can
express authentication, tracing spans, request mutation, response mutation,
and short-circuiting. Extractors should cover parameter production; generated
interceptors should cover cross-cutting before/after behavior. They must have
explicit ordering and route-selection rules.

Body access participates in one ownership plan covering both interceptors and
handler parameters:

- parts-only interceptors cannot observe or consume the body;
- streaming body interceptors may wrap or transform `B` into a replacement
  body passed to the next stage;
- exactly one terminal consumer may buffer or consume the body; and
- a terminal interceptor consumer and a handler `#[body]` parameter are a
  compile-time conflict unless the interceptor produces an explicit
  replacement body.

This must support signature verification, decompression, body-aware logging,
and size enforcement without silently buffering every request. **Delivered in
Phase 5:** `#[transform(stream, ...)]` wraps the transport body generically and
`#[transform(limit = N, ...)]` buffers only the routes that ask for it, so no
universal buffering is imposed.

Runtime-configured paths to statically known handlers retain that handler's
generated interceptors. Arbitrary mounted runtime services receive
router-wide generated interceptors and may supply their own Tower middleware,
but cannot name macro-generated per-handler interceptors for handlers the macro
cannot see.

Expose `Extension<T>` as both an extractor and response-parts modifier for the
open interchange case. Do not force all state or middleware communication
through extensions.

## Macro diagnostics

The macro should diagnose structural errors directly:

- more than one `#[body]` parameter;
- `#[body]` on a type that does not implement `FromRequestBody`;
- a dynamic capture missing `#[capture]`;
- `#[capture]` on a parameter absent from the registered dynamic template;
- duplicate capture names;
- a static capture parameter that does not match the path template;
- unsupported receiver or generic handler forms; and
- generated-name collisions.

Trait failures should be made actionable with generated helper assertions or
diagnostic attributes. Error messages should identify the handler and
parameter and state whether `FromRequestParts`, `FromRequestBody`, or
`IntoResponse` is required.

## Phased implementation

### Phase 0: API spike

- Inventory the matching, response, extraction, policy, and adapter dependency
  boundaries before adding HTTP dependencies.
- Prototype response construction so it can be measured both with and without
  generated handler routing.
- Prototype one static route taking `HeaderMap`, `Query<T>`, and a JSON body.
- Prototype heterogeneous handler response types.
- Validate the request/body ownership model without boxing extractor futures.
- Decide state versus explicit context.
- Build behaviorally equivalent Axum, Actix Web, Rocket, and Warp fixtures for
  the Investment Test scenarios.
- Record acceptance thresholds before comparing results.
- Measure instructions, allocations, latency, binary size, compile time, and
  generated-code size for both implementations.

Exit criterion: the prototype handles headers, query, one body consumer, and
response headers without changing the featureless `routerama` build, and meets
at least one durable value proposition from the Investment Test. If it does
not, stop and retain the focused resolver instead of proceeding to Phase 1.
The phase must also record which candidate subsets have a coherent standalone
API, dependency savings, and benchmarkable value; only those candidates may
become new public modules or crates.

#### Narrow vertical-slice findings (2026-07)

The first HTTP boundary slice is now implemented. It is intentionally smaller
than the complete Phase 0 investment test and establishes the following:

- Generated static and configured-dynamic entry points consume
  `http::Request<B>`, take shared state separately, and return
  `http::Response<routerama::response::Body>` directly.
- Request ownership is split once into parts and body. Matching borrows
  `Parts::uri.path()`, parts extractors borrow `Parts` immutably, and the one
  marked body extractor receives `B` by value. This preserves borrowed static
  captures while allowing the body to move exactly once.
- `Method`, `Uri`, and owned `HeaderMap` extraction is synchronous and
  monomorphic. Requesting an owned `HeaderMap` explicitly pays for its clone;
  handlers that do not request it do not. `State<T>` uses a `FromRef<S>`
  projection, with the blanket same-type projection cloning `T` (normally an
  `Arc` or another cheap shared handle).
- `Query<T>` is compiled only with both `route` and `query`. The prototype
  currently supports owned query schemas (`for<'q> T: FromQuery<'q>`); query
  values that borrow from the URI require a later lifetime-aware extraction
  design.
- `#[body]` is position-independent and a second marker is rejected during
  macro expansion. The only body extractor in this slice is the raw request
  body: its handler parameter type must equal the service request body type.
  Bounded bytes, text, JSON, transport-error handling, and body limits are
  deferred rather than represented by an unbounded or success-shaped API.
- Dynamic handlers use the same HTTP entry and extraction contract. Dynamic
  path values now require explicit owned `#[capture]` parameters, so request
  extractors are not accidentally added to the route shape.
- Every handler result is converted independently through `IntoResponse`.
  The implemented set covers HTTP responses, `String`/`&str`, `bytes::Bytes`,
  `Vec<u8>`, `Body`, `()`, status codes, `Result`, and two- or three-element
  status/header tuples. Routing misses map to 404; malformed paths and capture
  failures map to 400; query rejections map to 400.
- Cargo checks cover the exact feature sets: none, `response`, `resolve`,
  `route`, `query`, `json`, `route+query`, and all features. Dependency-tree
  checks confirm that none, `resolve`, and `query` do not include `http`,
  `http-body`, or `bytes`; those dependencies enter through `response`, which
  `route` enables.

The slice does not box handler or extractor futures. Parts/body extraction is
currently synchronous, and generated code awaits the concrete handler future
directly. It also does not box response bodies. Instead, all supported
responses normalize to a clear prototype `Body` containing zero or one
`bytes::Bytes` frame. This proves the HTTP composition boundary and works with
`http-body`, but it does **not** prove the planned service-specific body enum:
streaming and arbitrary body types are intentionally unavailable. The body
enum versus fixed-buffer normalization decision still requires instruction,
allocation, binary-size, and compile-time measurements before stabilization.
This paragraph records the first slice; the generated prototype described
under Phase 1 below now supersedes its functional limitation, but not the
measurement requirement.
The base generated future also does not impose `Send` on `B` or `Sync` on
`S`; transport adapters may add those bounds, while the core prototype remains
usable on a single-thread executor. This boundary must be revisited with the
transport fixtures.

At this checkpoint response and extraction were separate implementation areas
inside `route`, and response was not yet independently enabled because its
body representation and fallible response-parts semantics still awaited
evidence. The later generated-body, performance, and fallible-parts findings
settled those questions; the independent-response findings below supersede
that temporary placement without changing this slice's historical rationale.

#### Bounded body-extraction findings (2026-07-25)

The next Phase 0 slice adds buffering and decoding without weakening the
one-consumer model:

- `RawBody<B>` is now the only built-in raw-body form. It transfers the
  transport body unchanged and imposes no `http_body::Body` bound. Replacing
  the former same-type blanket implementation makes streaming ownership
  visible in the handler signature and avoids interpreting arbitrary user type
  names as extraction policy.
- `BytesBody<const LIMIT: usize>` and `TextBody<const LIMIT: usize>` accept
  bodies implementing `http_body::Body<Data = bytes::Bytes>`. The const generic
  makes the byte limit part of every buffering handler contract. There is no
  unbounded variant, configuration fallback, or library-selected default.
- `route::json::Json<T, const LIMIT: usize>` is present only with the additive
  `json` feature. It accepts `application/json` and structured
  `application/*+json` media types, including parameters, before collecting
  and deserializing with `serde_json`.
- `FromRequestBody` remains public because it is a coherent custom-extractor
  boundary. Its method now returns a concrete future through RPITIT. Generated
  code awaits that future directly; the contract requires neither boxing nor
  `Send`, so local transport bodies continue to work.

Generated dispatch still splits the request once. It runs every
`FromRequestParts` extraction before body extraction, regardless of the
body marker's declaration position, and passes immutable `Parts` to the body
extractor. JSON can therefore validate `Content-Type` without reconstructing a
request or cloning metadata. The request body moves into exactly one
`FromRequestBody` invocation, and the direct handler call restores the
declared argument order. Compile-time diagnostics identify both markers when a
handler declares two consumers.

Bounded collection polls data frames directly and does not use `SizeHint` as
authority. Every frame is checked against the remaining budget before being
copied, exact-limit input succeeds, trailers do not count as data, and a
multi-frame or dishonest-hint body cannot bypass the limit. The public
diagnostics and default responses are:

| Failure | Diagnostic | Response |
|---|---|---|
| Actual data crosses `LIMIT` | `BodySizeLimitError { limit, received }` through `BodyRejection` | `413 Payload Too Large` |
| Transport fails while yielding a frame | generic `BodyTransportError<E>` retaining `E` | `400 Bad Request` |
| Bounded text is not UTF-8 | `InvalidUtf8Error` with byte position and sequence length | `400 Bad Request` |
| JSON content type is missing, duplicated, or unsupported | `JsonContentTypeError` through `JsonRejection` | `415 Unsupported Media Type` |
| Bounded JSON is malformed | `JsonDecodeError` retaining `serde_json::Error` | `400 Bad Request` |

The feature graph is intentionally asymmetric:

```text
json ──> route ──> response ──> http + http-body + bytes + pin-project-lite
  │         └────> router macro/matcher
  └──────────────> serde + serde_json

query     (does not enable response, route, or json)
resolve   (does not enable response, route, or json)
response  (does not enable route, query, resolve, or json)
route     (does not enable query or json)
```

Placing `Json` under `route::json` gives `json` a clear public boundary, so
`json` logically implies `route`. The reverse edge would make ordinary routing
pay for a codec it did not select. Query extraction remains available only
when the independently selected `query` and `route` features meet; neither
feature reaches JSON transitively.

This slice still does not justify a new crate or root feature for extraction.
The useful contract combines HTTP `Parts`, `http_body::Body`, response policy
for rejections, and generated handler ownership. Moving those pieces to a
package without `route` would not yet create a smaller coherent consumer API.
The buffering helper remains private so custom extractors cannot accidentally
select an unbounded convenience path.

Behavioral coverage now exercises marker positions and direct calls,
exact/over-limit bodies, multiple frames, dishonest size hints, UTF-8 and
transport failures, explicit raw ownership, JSON media types and decoding, a
non-`Send` body, and duplicate-marker compile diagnostics. Exact feature checks
cover none, `response`, `route`, `json`, `query`, `resolve`, and all features.

The next task remains comparative body measurement, not API expansion. It must
measure single- and multi-frame buffering, JSON decoding, allocations,
instructions, response size, generated code, and compile time against the
framework fixtures. Until those results exist, do not change performance
claims, `docs/PERF.md`, or stabilize a generated response-body enum. Also
deferred are service-wide runtime limit policy, form/multipart extractors,
custom transport-error response policy, JSON response serialization, and
whether adapters should require `Send`.

#### Network-free bounded body-extraction evidence fixture (2026-07-25)

The comparative fixture is now implemented in
`benches/routerama_body_extraction.rs`, paired with
`benches/routerama_body_extraction_cg.rs`. The regular
`tests/body_extraction_fixtures.rs` target includes the same shared fixture and
executes every framework/scenario pair before timing is possible. All three
targets require Routerama's additive `json` feature. Axum's and Rocket's JSON
features and every external comparison framework remain dev-dependencies; no
comparison dependency or new production capability was added.

Every Routerama, Axum, Actix Web, Rocket, and Warp application applies the same
explicit 64-byte encoded-body limit. The controlled scenarios are:

- successful bytes extraction of a 10-byte payload from one data frame;
- successful bytes extraction of the identical payload from two frames
  containing 6 and 4 bytes, so frame count is the only changed input;
- successful bytes extraction of exactly 64 bytes, proving that the limit is
  inclusive;
- successful UTF-8 text extraction of an 18-byte payload;
- successful decoding of 24 bytes of `application/json`, followed by the same
  `name`/`count` response formatting;
- a 65-byte bytes body rejected as `413 Payload Too Large`;
- a valid 65-byte UTF-8 text body rejected as `413 Payload Too Large`;
- a valid JSON document with an encoded length of 65 bytes rejected as `413
  Payload Too Large` before decoding;
- a 6-byte text body containing invalid UTF-8 rejected as `400 Bad Request`;
- malformed JSON rejected as `400 Bad Request`; and
- the same valid JSON with unsupported and missing `Content-Type`, each
  rejected as `415 Unsupported Media Type`.

Every request has an exact `Content-Length`; JSON success and malformed JSON
use `application/json`. Successes return `200 OK`, `x-fixture: ready`, and
identical complete bodies. Rejections have no fixture header and an empty
body. The self-check drains every response and compares status, fixture-header
bytes, body length, and body fingerprint. Request bodies are newly constructed
for every `FnOnce` prepared call, so a consumed body can never be reused or
mistaken for an extraction fast path.

The framework limit mechanisms and deliberate adapters are explicit:

- **Routerama** uses `BytesBody<64>`, `TextBody<64>`, and `Json<T, 64>`.
- **Axum** uses its normal `Bytes`, `String`, and `Json<T>` extractors under
  `DefaultBodyLimit::max(64)`.
- **Actix Web** uses its normal bytes, string, and JSON extractors with both
  `PayloadConfig` and `JsonConfig` set to 64. Every scenario uses the same
  `ActixPayloadStream` backend. Request and stream construction occur during
  preparation, outside the sample; stream polling and extractor buffering
  occur during `call_service`, inside the sample. The one-frame/split delta
  therefore changes only the number of yielded frames.
- **Rocket** uses its normal strict `Vec<u8>`, `String`, and `Json<T>` data
  guards with the `bytes`, `string`, and `json` limits each set to 64. Its
  local client accepts one contiguous body buffer, so the split scenario has
  identical encoded bytes but cannot preserve the caller's frame boundary.
  The row remains for response-equivalence coverage but is named
  `rocket_coalesced_client_body` in Criterion and Callgrind output; it is not
  represented as a five-way multi-frame comparison.
  A fixture-only data guard checks JSON `Content-Type` before delegating to
  Rocket's bounded JSON guard because the native guard does not validate the
  media type.
- **Warp** combines its documented `content_length_limit(64)` with normal
  bytes/JSON filters. Because that filter trusts only `Content-Length`, the
  request body is also wrapped in `http_body_util::Limited` so a dishonest
  length cannot make either collector buffer unboundedly. A fixture filter
  rejects missing JSON `Content-Type` because Warp otherwise assumes JSON;
  text performs explicit UTF-8 validation and owned-string construction after
  the bounded bytes filter.

Native rejection bodies and some statuses differ. The comparison applications
normalize only those differences: Actix Web's JSON content-type error becomes
415; Rocket's strict bytes/string limit error becomes 413 and its fixture JSON
guard maps media-type failure to 415; Warp recovery maps its bounded-body,
media-type, UTF-8, and decode rejections to the common empty responses. Axum
already supplies the selected statuses. Routerama's native rejection mapping
is the common contract. This is application policy in the fixtures, not new
Routerama API.

The dispatch controls from the earlier fixture also apply. Calls are direct
and in process, with no socket creation. Each framework uses a current-thread
Tokio runtime and an initialized application retained for process lifetime.
Runtime/application construction, payload/request creation, closure creation,
equivalence checking, and warmup are outside the measured call. The common
`Runtime::block_on`, body consumption, buffering, UTF-8/JSON decoding, handler
and rejection work, complete response drain, and fingerprinting remain
inside. Harness futures are stack-pinned; framework-internal boxing remains
measured. Criterion uses `iter_batched`, and Gungraun receives one prepared
call from setup, so neither request preparation nor final runtime/application
teardown enters a sample.

The allocation sweep measures one pre-created call for every scenario in a
per-framework aggregate. It spans complete extraction, response production,
drain, and observation on the measured thread. It is a diagnostic only and is
explicitly **not** the zero-allocation-through-handler-entry acceptance gate.

Controlled Criterion and Callgrind measurements now cover all twelve
scenarios. Routerama's geometric means are 484 ns and 3,201 instructions. The
fastest comparison geometric means are Warp at 911 ns and Actix Web at 5,773
instructions, leaving Routerama 47% lower in measured time and 45% lower in
instructions. Routerama is the lowest result in every individual row.

The complete extraction/response-observation allocation sweeps are 4,292 bytes
for Routerama, 31,659 for Axum, 89,214 for Actix Web, 86,993 for Rocket, and
10,117 for Warp. This is strong evidence that exact generated extraction does
not sacrifice the routing advantage, but it remains broader than the
zero-allocation-through-handler-entry gate.

`FromRequestBody` and the bounded collectors now demonstrate a coherent custom
extractor capability that does not conceptually require route matching.
However, their rejection contract currently depends on `IntoResponse` and the
prototype fixed `Body`. Do not create a new crate or independently enabled
module until response representation and fallible response-part precedence are
settled; splitting now would freeze the wrong dependency boundary. Larger
payload-size sweeps, response-body alternatives, generated-code size, and clean
compile-time measurements remain deferred.

#### Network-free dispatch evidence fixture (2026-07)

The first five-way HTTP evidence fixture is implemented in
`benches/routerama_http_dispatch.rs`, paired with
`benches/routerama_http_dispatch_cg.rs`. A regular integration test includes
the same shared fixture and executes every framework/scenario pair before any
timing is possible. The fixture deliberately measures only the capability
implemented by the narrow vertical slice: it does not add a Routerama API or
represent bounded bodies, JSON, middleware, streaming, or transport.

Each Routerama, Axum, Actix Web, Rocket, and Warp application has the same
16-route shape. The measured scenarios are:

- literal hits registered first, in the middle, and last;
- one route with a textual `name` capture and a parsed `u32` capture;
- `POST` method, `x-mode` header, and `q`/`page` query extraction;
- a `201 Created` response with `x-fixture: ready`;
- a complete miss producing an empty `404` response; and
- a numeric capture-conversion failure producing an empty `400` response.

The successful responses have identical payloads. The self-check consumes
each response body and compares status, the fixture header, body length, and
body fingerprint. Routerama retains its direct borrowed `&str` plus generated
`u32` capture conversion. The comparison handlers accept the frameworks'
textual path values and perform the same `u32::from_str` conversion explicitly:
Rocket and Warp otherwise treat a failed typed path filter as a route
non-match, which would produce `404` instead of Routerama's implemented `400`.
This makes the compared application behavior and conversion work explicit
instead of hiding that semantic difference.

The controls are:

- invocation is directly in process through Routerama's generated entry,
  Axum's Tower service, Actix Web's initialized test service, Rocket's
  asynchronous local client, and Warp's filtered service; no fixture binds,
  accepts, or connects a socket;
- every framework uses a current-thread Tokio runtime created before
  measurement; a common `Runtime::block_on` and prepared-call indirection are
  intentionally included in every measured sample, while runtime/application
  construction, request creation, equivalence checking, and warmup are
  excluded. Every runtime and initialized application/service is
  retained for the life of the benchmark process. In particular, a temporary
  Callgrind setup fixture cannot leave its prepared closure holding the final
  application or runtime reference, so framework teardown never enters the
  measured `FnOnce` call. Axum and Warp require mutable Tower services, so
  their prepared calls borrow the retained service through `RefCell`; that
  adapter's borrow check remains measured, but no per-call service clone is
  created or destroyed;
- the harness future is stack-pinned without benchmark-side `Box::pin`;
  framework-internal boxing remains part of that framework's measured path.
  Responses are completely drained and fingerprinted so dispatch and response
  work cannot be optimized away. Routerama, Axum, and Warp bodies use the same
  `http_body::BodyExt::collect` adapter. Actix Web and Rocket body types do not
  implement `http_body::Body`, so their native complete-body adapters remain
  in the measured path; the fixture claims equivalent complete observation,
  not identical adapter overhead;
- requests have empty bodies, so this fixture makes no body-extraction claim;
  JSON remains excluded because Routerama has no equivalent bounded JSON
  extractor; and
- `alloc_tracker` wraps the Criterion/test allocator and measures one
  pre-created call for every scenario in a per-framework aggregate. The
  current-thread runtime keeps all polled work in the measured thread;
  request/closure preparation is outside the span. This is a reliable
  allocation-count path for the present synchronous/ready operations, but its
  output is not a checked-in performance result.

The representative route-set size in this capability fixture is exactly 16.
At the time of this fixture, the 128- and 1,024-route shapes were deferred
rather than hand-maintained as thousands of static Routerama and Rocket
declarations. The separate generated scaling fixture described below now
covers those sizes without turning this capability fixture into a generated
application.

#### Initial controlled measurements (2026-07-25)

The corrected fixture was measured with 30 Criterion samples, a one-second
warmup, a two-second measurement window, and the paired Callgrind suite.
Runtime/router teardown was verified absent from the Callgrind profiles before
recording results. The complete raw table and commands are in `docs/PERF.md`.

Across the eight implemented scenarios, Routerama's geometric-mean median
latency is 392 ns, compared with 944 ns for Axum, the fastest comparison
framework. Geometric-mean instruction count is 2,727 for Routerama and 5,676
for Axum. Routerama therefore uses 58% less measured time and 52% fewer
instructions than the fastest competitor on this fixture, exceeding the
initial 15% instruction target. It is also the lowest result in every
individual row, so no implemented scenario triggers the 5% regression gate.

The allocation sweep reports complete response-production and body-observation
bytes across one call to each scenario: Routerama 5,007, Axum 10,554, Actix Web
4,048, Rocket 17,385, and Warp 16,115. Because this span extends beyond handler
entry and includes framework-specific response adapters, it does not test the
zero-allocation-through-handler-entry requirement. That gate remains open.
*Superseded:* later fixtures closed it, and the acceptance-gate tables in
`docs/PERF.md` record the gate as **Met** from 2026-07-26 onward.

This was strong evidence for generated direct dispatch and extraction, but by
itself did not settle the fixed `Body` representation or justify publishing
response construction independently. The later response-body and
fallible-parts evidence below supplies the missing measurements and contract,
so the independent `response` decision supersedes this checkpoint's temporary
placement. Likewise, do not extract the matching engine into a new crate until
the 128/1,024-route generator demonstrates a stable framework-neutral boundary
and dependency win.

The following remain explicit Phase 0 work rather than inferred successes:

- bounded bytes/text/JSON extraction and rejection details;
- borrowed request-part views, extensions, version, and custom asynchronous
  extractors;
- fallible header conversion and response-parts error precedence;
- generated service-specific body enums, streaming, and a boxed opt-in escape
  hatch;
- configurable routing rejection policy;
- controlled results from the generated 16-, 128-, and 1,024-route scaling
  fixtures;
- the remaining guard/interceptor, predicate, body, fallback, and state
  scenarios from the Investment Test; and
- controlled Criterion/Callgrind runs, throughput, binary/compile/generated
  size measurements, recorded results, and acceptance-gate decisions.

Therefore the vertical slice and its first equivalent fixture prove ownership,
direct dispatch, feature isolation, basic response composition, and a
repeatable evidence path. They do not by themselves satisfy the full Phase 0
exit criterion or justify proceeding through the broad extractor catalog.

#### Generated route-set scaling fixture (2026-07-25)

The next scaling milestone is implemented separately from the 16-route
capability application. `scripts/generate_http_dispatch_scaling.rs` emits the
checked-in `benches/generated/http_dispatch_scaling.rs`; the generated header
identifies its source and forbids hand editing. The generator is deterministic,
supports `--check`, and shares its pure generation function with
`tests/http_dispatch_scaling_generated.rs`, so the regular test suite fails
when committed output is stale. Regenerate and verify from the repository root:

```bash
cargo +nightly -Zscript crates/routerama/scripts/generate_http_dispatch_scaling.rs
cargo +nightly -Zscript crates/routerama/scripts/generate_http_dispatch_scaling.rs --check
cargo test -p routerama --test http_dispatch_scaling_generated --locked
```

Each size has exactly 16, 128, or 1,024 non-overlapping `GET` literals in
ascending registration order:
`/scale/routes-NNNN/route-IIII`. First is index zero, middle is `N / 2`, and
last is `N - 1`; `/scale/routes-NNNN/missing` cannot overlap a hit. A hit
returns `200 OK`, `x-route-id: route-IIII`, and the complete
`route-IIII` body. A miss returns an empty `404` with no fixture header. Every
measured response is fully drained and its status, fixture-header bytes, body
length, and body fingerprint are observed. The equivalence test executes all
60 size/framework/scenario combinations before benchmarks can be accepted.

The framework representations remain explicit:

- **Routerama:** the generator emits one static `#[router]` implementation per
  size and every literal `#[route]` method. There is no dynamic registration.
- **Axum:** the normal runtime `Router` receives the generated table through
  ordered `.route(...)` calls.
- **Actix Web:** the normal `App` builder receives the same table through
  ordered `.route(...)` calls.
- **Rocket:** the generator emits its normal attributed route functions and
  ordered `routes!` lists. Lists are produced in 64-route helper batches and
  mounted in order; this avoids a single multi-megabyte macro stack frame
  without changing Rocket's route representation or matcher.
- **Warp:** the normal `Filter::or` representation is retained. Each leaf and
  branch is a documented `BoxedFilter`; the ordered branches form a balanced
  tree so last/miss dispatch does not require a 1,024-frame Rust call stack.
  `or` still evaluates leaves left-to-right, and no alternate matcher is
  substituted.

All five idiomatic representations compile and pass equivalence checks at
1,024 routes, so no common size was dropped. The paired benchmark files are
`routerama_http_dispatch_scaling.rs` and
`routerama_http_dispatch_scaling_cg.rs`. Criterion groups use
`routerama_http_dispatch_scaling/routes_N_scenario/framework`; matching
Callgrind functions use `routes_N_scenario_framework`.

The original dispatch controls also apply here. Each framework runs directly
in process on a current-thread Tokio runtime with no socket I/O. Runtime,
application, and service state are retained for the process lifetime; prepared
closures contain only requests and references to that retained state, never a
final owning reference. Application/runtime construction, request creation,
equivalence checking, and one scenario-specific warmup are outside the
measured call. The common `Runtime::block_on`, response drain/fingerprint, and
framework adapter remain in-region; benchmark futures are stack-pinned.
Criterion uses `iter_batched`, while Gungraun receives a prepared call from its
setup function, so Callgrind cannot measure application or runtime teardown.

Comparison frameworks remain dev-dependencies, and the scaling targets require
only Routerama's `route` feature (which transitively selects `response`). The
default and independent feature sets therefore do not acquire a comparison
framework or a new production dependency.

The controlled Criterion and Callgrind runs are now recorded in
`docs/PERF.md`. At 1,024 routes, Routerama's four-scenario geometric means are
429 ns and 2,685 instructions. Axum is the fastest comparison framework at
749 ns and 5,137 instructions, leaving Routerama 43% faster and 48% lower in
instructions. Routerama has the lowest result in every measured scaling row.

From 16 to 1,024 routes, Routerama's geometric-mean latency increases 29% and
its instruction count increases 9%. This is evidence of generated-code or
cache pressure and must remain a tracked design cost, even though the lead is
preserved. It argues for keeping generated dispatch surgical; it does not
justify replacing it with runtime indirection.

The combined generated source for all frameworks and sizes is 540,156 bytes.
After cleaning Routerama's package artifacts, the combined debuginfo-enabled
release Criterion binary took 85.3 seconds to build, peaked at 1,287,312 KiB
resident memory, and was 56,960,800 bytes. These are fixture-wide engineering
costs rather than per-framework application measurements. Separate minimal
application binaries are still required before drawing a framework binary-size
or compile-time conclusion.

The scaling evidence strengthens the case for a reusable matching core but
still does not prove a public standalone crate: generated Routerama dispatch
and private runtime matching share implementation, while no framework-neutral
consumer API has been demonstrated. Keep the engine private until such a
consumer exists. The following command sequence remains the reproducible path
for collecting compile and binary-size controls in a fresh repository-local
target directory:

```bash
mkdir -p target/routerama-http-dispatch-scaling-measurement
rustc -Vv > target/routerama-http-dispatch-scaling-measurement/environment.txt
cargo -V >> target/routerama-http-dispatch-scaling-measurement/environment.txt
uname -a >> target/routerama-http-dispatch-scaling-measurement/environment.txt
git rev-parse HEAD >> target/routerama-http-dispatch-scaling-measurement/environment.txt
wc -c crates/routerama/benches/generated/http_dispatch_scaling.rs \
  > target/routerama-http-dispatch-scaling-measurement/generated-source-size.txt
rm -rf target/routerama-http-dispatch-scaling-clean
mkdir -p target/routerama-http-dispatch-scaling-clean
/usr/bin/time -f '%e seconds' \
  -o target/routerama-http-dispatch-scaling-measurement/clean-compile-time.txt \
  env CARGO_TARGET_DIR=target/routerama-http-dispatch-scaling-clean \
  cargo bench -p routerama --features route \
    --bench routerama_http_dispatch_scaling --no-run --locked \
    --message-format=json \
  > target/routerama-http-dispatch-scaling-measurement/cargo-messages.json
jq -r 'select(.reason == "compiler-artifact"
  and .target.name == "routerama_http_dispatch_scaling")
  | .executable // empty' \
  target/routerama-http-dispatch-scaling-measurement/cargo-messages.json \
  | xargs -r stat --printf='%n %s bytes\n' \
  > target/routerama-http-dispatch-scaling-measurement/benchmark-binary-size.txt
```

The paired suites can be compile-checked without running a long measurement:

```bash
cargo bench -p routerama --features route \
  --bench routerama_http_dispatch_scaling --no-run --locked
cargo bench -p routerama --features route \
  --bench routerama_http_dispatch_scaling_cg --no-run --locked
```

### Phase 1: response foundation

- Add `Response`, `Body`, `IntoResponse`, and `IntoResponseParts`.
- Determine whether the response foundation can be an independently enabled
  `routerama` module without matching, extraction, or router macros.
- Implement status/header/body tuple composition.
- Generate a service-specific response-body enum and an explicit boxed-body
  fallback.
- Add response-conversion and failure-precedence tests.
- Change generated handlers to accept heterogeneous return types.

This phase immediately solves response status and header control.

#### Generated response-body prototype findings (2026-07-25)

The next Phase 0/1 vertical slice removes fixed-body normalization from
generated services without claiming that the representation is stable:

- `IntoResponse` now has `type Body:
  http_body::Body<Data = bytes::Bytes>` and returns
  `http::Response<Self::Body>`. Built-in `()`, string, byte, status, and
  rejection conversions still use the zero-or-one-frame `response::Body`.
  `http::Response<B>` retains a real concrete `B` unchanged. A response whose
  body is only a value, such as `http::Response<String>`, must instead use
  `http::Response<Body>` or a status/header tuple ending in the value. Stable
  coherence cannot provide both a blanket `B: http_body::Body` response impl
  and the old blanket `T: Into<Body>` response impl: an upstream crate may add
  `Body` for one of those value types. The prototype chooses the ecosystem
  meaning of `http::Response<B>` rather than silently normalizing a real body.
- `Result<T, E>` uses the public concrete
  `EitherBody<T::Body, E::Body>` and `EitherBodyError` rather than selecting a
  boxed common body. Tuple response-parts composition is generic over the
  retained final body.
- `#[router]` collects handler results, parts/body extraction rejections, and
  routing failures into one finite sum. Static and configured-dynamic
  handlers share it. Sources with the same macro-visible category and
  syntactic type are deduplicated; semantic associated-type equality is not
  attempted.
- The generated private names are
  `__routerama_<Service>::<Service>ResponseBody`,
  `<Service>ResponseBodyProjection`, and `<Service>ResponseBodyError`, with
  private `SourceN` variants. They remain under the existing private generated
  module policy and are not exported. The public entry point is:

  ```text
  Response<impl http_body::Body<
      Data = bytes::Bytes,
      Error = impl core::error::Error + use<RequestBody, State>,
  > + use<RequestBody, State>>
  ```

  This avoids both an unnameable private type in a public signature and an
  additional collision-prone public generated symbol. A public service whose
  handlers return private body types compiles with `private_interfaces`
  denied. Rust 2024 precise captures exclude borrowed service and state values,
  preventing an accidental non-`'static` opaque response. Existing
  handler-contract bounds retain their scoped `private_bounds` allowance
  because private extractor/response source types are intentional.
- Polling uses safe `pin-project-lite` projection and delegates directly to the
  selected concrete body. It forwards data and trailer frames unchanged and
  delegates `is_end_stream` and `size_hint`. There is no body allocation,
  boxed future, body vtable call, per-frame dynamic dispatch, or authored
  unsafe code on an ordinary generated branch; behavioral coverage compiles
  generated sums under `forbid(unsafe_code)`.
- Each generated body has a concrete heterogeneous error sum. It retains the
  original error value and maps only when an error frame occurs. The opaque
  error implements `Debug`, `Display`, and `core::error::Error` without
  requiring source errors to implement `Error`, `Send`, `Sync`, or `'static`;
  consequently it cannot expose a typed variant or `Error::source` yet.
  `Display` identifies the deduplicated response source. This is the deliberate
  Phase 0 tradeoff for avoiding error boxing and convenience bounds.
- Auto traits are structural across the complete service sum. An all-`Send`
  service satisfies an adapter requiring body `Send + 'static` and error
  `Error + Send + Sync + 'static`; a service containing one local variant does
  not, even when a different route is selected. The core route future and body
  remain usable on a current-thread executor. Transport adapters state their
  stronger bounds separately.
- Public `BoxBody::new` is the explicit open-set boundary. It allocates once,
  polls through a body trait object, requires a `'static` body/error, and does
  not require `Send` or `Sync`. `BoxBodyError` boxes a concrete error only when
  one occurs and exposes `as_error`/`into_inner`. No blanket conversion boxes
  ordinary bodies, and generated code mentions `BoxBody` only when a handler
  explicitly returns it.

Behavioral coverage combines fixed bytes with multi-frame data and trailers in
one service; heterogeneous handlers and `Result` branches; a streaming custom
rejection; configured-dynamic streaming; explicit `BoxBody`; body-error
propagation; local handler futures and response bodies; adapter bounds; and
generated privacy/symbol hygiene. Compile-fail coverage reports that response
body data must be `bytes::Bytes`.

The `bytes::Bytes` data contract is intentionally narrower than arbitrary
`http_body::Body::Data`: generating a second data-buffer sum would add a
per-frame representation and transport questions that this slice has not
measured. The opaque return also means callers cannot match the generated
error variants. Fallible response-parts conversion, runtime-mounted unknown
services, and a named transport adapter remain future work. Response
machinery remained under `route` at this prototype checkpoint. The subsequent
response-body and fallible-parts evidence completed the contract and justified
the top-level `response` feature described below; it did not justify a new
crate.

Paired Criterion/Callgrind fixtures are deferred to the existing
`response-body-evidence` task because the representation and opaque boundary
are not stable enough to establish a benchmark contract. That task must use
the public route API and compare at least fixed-only, fixed-plus-streaming,
multi-frame-with-trailer, error-frame, and explicit-`BoxBody` cases. It must
pair equivalent direct/generated/boxed representations, drain every frame and
trailer, keep construction outside the measured region, stack-pin measured
futures, and cover 16/128/1,024 source-set scaling. Record instructions,
latency, allocations, `size_of` response/body values, generated source size,
clean compile time, and binary size. Callgrind cases must have matching
Criterion cases and must isolate polling from allocation. Do not update
`docs/PERF.md` until those controlled results exist.

The existing 1,024-route generated scaling fixture now crosses Clippy's
conservative `large_stack_frames` threshold after response mapping is added;
the generated fixture records that expectation only at 1,024 routes. This is
not treated as a measured regression or hidden in ordinary generated
services. The response-body evidence task must measure future size and actual
stack/compile effects before representation changes or performance claims.

#### Response-body evidence fixture (2026-07-25)

The response representations now have a dedicated internal comparison rather
than a synthetic five-framework table. There is no honest framework-equivalent
row for Routerama's private generated sum, its direct concrete body, and its
explicit `BoxBody` boundary. The paired harnesses are
`benches/routerama_response_body.rs` and
`benches/routerama_response_body_cg.rs`; both include
`benches/common/response_body_scenarios.rs`. The regular
`tests/response_body_fixtures.rs` target executes the same scenarios and
checks their behavior, allocation boundaries, runtime sizes, and transport
bounds.

The elementary measured cases are:

- `direct_observation/fixed_body`: poll a prepared fixed `Body`;
- `direct_observation/concrete_stream`: poll a prepared concrete body with two
  distinct data frames and one populated trailer frame;
- `direct_observation/box_body_wrap_and_observe`: construct `BoxBody` from the
  prepared stream and then poll it, intentionally including its one allocation
  and dynamic dispatch;
- `generated_route/fixed_body`: route a prepared request and poll the fixed
  response through the one-concrete-body service sum;
- `generated_route/concrete_stream`: route a concrete stream supplied during
  preparation through `RawBody<ResponseInput>`, then poll it through the
  generated sum;
- `generated_route/explicit_box_body`: perform that same prepared-body route
  while the handler explicitly constructs and returns `BoxBody`;
- `error_propagation/generated_concrete`: route and poll a prepared failing
  body through the generated concrete error sum; and
- `error_propagation/boxed`: route the same prepared failure through a generated
  handler that constructs `BoxBody`, then poll the routed response and include
  both the body-erasure allocation and the error-box allocation.

Criterion uses `iter_batched`; each Callgrind function receives the same
`PreparedScenario` from Gungraun setup. Request creation, fixed/concrete body
creation, populated trailer-map creation, and per-instance failure identity
assignment are outside the measured function. Route future creation/polling,
generated response mapping, intentional `BoxBody` construction, frame polling,
fingerprints, and body teardown are inside. In particular, the generated stream
is passed through the request solely to keep its construction out of the route
sample; the directly generated `RawBody` extraction and variant check remain
measured and must not be mistaken for an ordinary application handler cost.
The Callgrind boxed rows describe wrapper/dynamic-dispatch instructions around
an allocator call; they do not claim that Callgrind models allocator latency.

Every in-memory future and body is stack-pinned and must be immediately ready,
so no executor, socket, or benchmark-side future allocation is involved.
Observation fingerprints every data payload with frame boundaries, every
trailer name/value, and frame order. It also records the initial size hint and
initial/final end-stream state. Every prepared failure receives a distinct
thread-local identity and a per-instance drop flag. Its concrete error
publishes that identity to a thread-local drop observation, so batched
Criterion preparation and parallel libtest threads cannot reset or overwrite
another thread's evidence. Both generated concrete and boxed paths verify the
exact instance flag is still clear before the routed opaque error is observed,
then verify that dropping that error sets the flag and publishes the same
identity. Error fingerprinting performs the same fixed marker-and-eight-byte
identity work for both representations; it does not compare their intentionally
different `Display` text.

Allocation diagnostics have separate setup and measured-thread operations for
every scenario, rather than one aggregate sweep. The regular test currently
enforces measured allocation counts, in the scenario order above, of
`[0, 0, 1, 0, 0, 1, 0, 2]`. It separately proves that populated trailer-map
setup allocates before both direct and generated stream samples. Byte counts
are printed per setup/measured span and remain allocator/host diagnostics, not
portable constants.

Runtime `size_of_val` diagnostics avoid naming private opaque types. They
cover public `Body`, the concrete stream, `EitherBody<Body, StreamBody>`,
`BoxBody`, `BoxBodyError`, route futures for a one-body and a multi-body
service, each returned opaque response/body, and a produced generated error
sum. On the current 64-bit x86-64 Linux host they report, in bytes:

| Value | Bytes |
|---|---:|
| `Body` | 32 |
| concrete stream | 168 |
| `EitherBody<Body, StreamBody>` | 168 |
| `BoxBody` / `BoxBodyError` | 16 / 16 |
| one-body route future / response / opaque body | 488 / 152 / 40 |
| multi-body route future / response / opaque body | 488 / 280 / 168 |
| generated body-error sum | 24 |

These are host/toolchain-specific layout observations, not stable API limits;
the test checks only structural relationships and prints the current values.
The transport check separately routes an all-`Send` fixed-plus-stream service
through a generic adapter requiring
`Body<Data = Bytes> + Send + 'static` and
`Error + Send + Sync + 'static`. A service containing `Rc` in one body variant
continues through the generic core adapter without `Send`, demonstrating that
the transport bound is additive rather than imposed by routing.

Distinct-body growth is isolated from route-count growth by three deterministic
compile controls. `scripts/generate_response_body_variants.rs` produces the
checked-in `routerama_response_body_variants_{1,4,16}` bench targets. Every
target has exactly 16 identical routes and response payloads; only the number
of distinct `VariantBody<const ID: usize>` handler body types changes. The
freshness test regenerates each source twice and compares the committed files.
This measures the real extra enum arms and monomorphizations without using
route count as a proxy. Measure variant-dependent target compilation only after
warming one shared dependency graph. Rebuild each benchmark target repeatedly
by touching only its generated source, alternate target order to reduce
systematic thermal/cache bias, and record medians from the elapsed-time rows:

```bash
target_dir="target/routerama-response-body-variants-warm"
evidence_dir="target/routerama-response-body-variants-evidence"
mkdir -p "${target_dir}" "${evidence_dir}"
: > "${evidence_dir}/target-compile-seconds.txt"

# Warm dependencies and all three target baselines before timing.
for variants in 1 4 16; do
  env CARGO_TARGET_DIR="${target_dir}" \
    cargo bench -p routerama --features route \
      --bench "routerama_response_body_variants_${variants}" \
      --no-run --locked --quiet
done

for order in "1 4 16" "16 4 1" "4 16 1" "1 16 4" "4 1 16"; do
  for variants in ${order}; do
    touch "crates/routerama/benches/routerama_response_body_variants_${variants}.rs"
    /usr/bin/time -a -f "${variants} %e" \
      -o "${evidence_dir}/target-compile-seconds.txt" \
      env CARGO_TARGET_DIR="${target_dir}" \
      cargo bench -p routerama --features route \
        --bench "routerama_response_body_variants_${variants}" \
        --no-run --locked --quiet
  done
done

# Record loadable code/read-only-data sections, not debuginfo-inflated file size.
for variants in 1 4 16; do
  target_name="routerama_response_body_variants_${variants}"
  messages="${evidence_dir}/variants-${variants}-cargo-messages.json"
  env CARGO_TARGET_DIR="${target_dir}" \
    cargo bench -p routerama --features route \
      --bench "${target_name}" --no-run --locked --message-format=json \
    > "${messages}"
  executable="$(
    python3 -c '
import json, sys
name = sys.argv[1]
paths = [
    message["executable"]
    for line in sys.stdin
    if (message := json.loads(line)).get("reason") == "compiler-artifact"
    and message["target"]["name"] == name
    and message.get("executable")
]
print(paths[-1])
' "${target_name}" < "${messages}"
  )"
  size -A -d "${executable}" \
    | awk '$1 == ".text" || $1 == ".rodata" { print }' \
    > "${evidence_dir}/variants-${variants}-sections.txt"
done
```

Elapsed compile rows remain machine-, filesystem-cache-, scheduler-, and
toolchain-sensitive, while `.text`/`.rodata` include shared benchmark harness
and library code in addition to variant-dependent code. Compare repeated
within-host deltas, not these values as portable absolutes. Controlled Criterion and Callgrind measurements now establish the runtime
tradeoff. Generated concrete streaming takes 397 ns and 1,978 instructions,
with no measured allocation. The same routed stream through explicit
`BoxBody` takes 472 ns and 2,275 instructions, with one 168-byte allocation.
Concrete generated error propagation takes 244 ns and 1,266 instructions;
boxed propagation takes 324 ns and 1,565 instructions, with one body box plus
one 24-byte error box. The generated sum is therefore the correct default:
boxing remains valuable as an explicit open-set boundary, but imposing it on
closed services would measurably spend time, instructions, and allocations.

Runtime layout shows the cost on the other axis. On the measured 64-bit
x86-64 Linux host, a fixed-only generated response is 152 bytes with a 40-byte
opaque body; a heterogeneous generated response is 280 bytes with a 168-byte
opaque body. Both route futures are 488 bytes. These are observations, not API
layout guarantees.

The 1/4/16 distinct-body controls produced median warm target compile times of
4.57, 4.81, and 5.10 seconds. Their `.text` sizes were 249,226, 249,434, and
249,466 bytes; `.rodata` remained 22,800 bytes. Sixteen body types therefore
cost about 12% target compile time in this sample but only 240 bytes of
loadable code over one type. This supports body-type deduplication and generated
sums while making compile-time growth a tracked budget. Larger source sets and
transport-specific binaries remain future controls.

#### Fallible response-part composition findings (2026-07-25)

The response-parts contract is now complete enough to evaluate separately from
the generated router:

- `IntoResponseParts` has an associated `Error: IntoResponse` and consumes a
  public body-free `ResponseParts`. Implementations can inspect and update
  status, version, headers, and extensions, then return the parts or a typed
  rejection. `StatusCode`, `HeaderMap`, and header arrays use `Infallible`, so
  their call sites remain ordinary tuples without wrappers or error handling.
- Tuple application preserves the original leftmost-wins behavior. The final
  value is converted first. `(part, value)` then applies `part`;
  `(first, second, value)` applies `second` and then `first`. A later
  application replaces a duplicate status/header, so `first` wins over
  `second` and both win over the final response. Header-array entries are
  inserted left to right, making the last duplicate inside one array win.
- Failure follows the same deterministic order. A `second` failure prevents
  `first` from running. If `second` succeeds and `first` fails, the successful
  body and metadata already changed by `second` are discarded. No surrounding
  success status or header is applied to the rejection response, so rejection
  policy cannot be accidentally overwritten.
- A two-item tuple has body
  `EitherBody<R::Body, <P::Error as IntoResponse>::Body>`. A three-item tuple
  has body
  `EitherBody<R::Body, EitherBody<<P2::Error as IntoResponse>::Body,
  <P1::Error as IntoResponse>::Body>>`.
  These branches retain data frames, trailers, body errors, and auto traits.
  Tuple conversion never selects `BoxBody`; explicit `BoxBody` handlers retain
  the existing one-allocation interoperability boundary.
- The generated service body needs no part-specific source analysis. A handler
  tuple's complete nested body and body-error sum enter the existing handler
  response variant through `IntoResponse::Body`, then the private generated sum
  delegates to it normally. A public service returning a custom fallible part
  compiles with private-interface lints denied and keeps the generated outer
  body/error names behind the opaque route return.

Behavioral coverage uses a public integration-test `CheckedHeader` custom part,
not a permanent Routerama helper. Invalid metadata returns a concrete streaming
rejection body rather than `Body`; tests observe its data frames, trailers, and
typed stream error through both direct tuple conversion and generated routing.
Separate first-part, second-part, and both-fail cases prove right-to-left
short-circuiting and show that modified success metadata is discarded.

The response evidence allocation fixture now measures direct fallible-part
success and rejection through complete body observation. Both paths report zero
allocations. No Criterion or Callgrind row was added: the existing controlled
response-body fixture already measures concrete generated sums against explicit
boxing, while the new question was whether tuple branching hid an allocation.
The direct allocation assertions answer that question without inventing a new
performance number.

This settles the response contract that previously blocked a
capability-boundary decision. The following decision records the separate
dependency and consumer evidence.

#### Independent response capability findings (2026-07-25)

Response construction is now a top-level `routerama::response` capability
enabled by the matching `response` Cargo feature. The complete foundation moved
from `src/route/body.rs` and `src/route/response.rs` to
`src/response/body.rs` and `src/response.rs`: `Body`, `NeverBody`,
`EitherBody`/`EitherBodyError`, `BoxBody`/`BoxBodyError`, `Response`,
`ResponseParts`, `IntoResponse`, and `IntoResponseParts` have one canonical
public path. `route` depends on `response` and has no compatibility re-exports.
Generated handlers resolve renamed dependencies as before but now emit
canonical `<crate>::response::Response` and
`<crate>::response::IntoResponse` paths for return types, bounds, and
conversion.

The normal-dependency evidence is concrete:

```text
$ cargo tree -p routerama --no-default-features --features response --edges normal --depth 1
routerama
├── bytes
├── http
├── http-body
└── pin-project-lite

$ cargo tree -p routerama --no-default-features --edges normal --depth 1
routerama
```

`http` retains its own small transitive dependencies, but Routerama's
`response` edge has exactly those four direct dependencies. The response-only
tree contains no `routerama_macros`, `routerama_build`,
`http_path_template`, `smallvec`, matching engine, extraction, query, JSON, or
resolve code. Enabling `route` adds the macro/matcher dependencies and enables
`response` transitively; it does not alter the response API. The exact feature
matrix is now:

```text
default = []
response = bytes + http + http-body + pin-project-lite
route = response + router macro/matcher
json = route + serde + serde_json
query = independent
resolve = independent
```

Standalone integration tests use only `routerama::response` and cover built-in
composition, typed fallible parts, data frames, trailers, concrete stream
errors, explicit `BoxBody`, and a body containing `Rc` to prove that no
mandatory `Send` bound entered the API. A response-only compile-fail fixture
also proves that `routerama::route` is not exposed. Existing route behavior,
fixtures, generated controls, examples, and benchmarks consume the canonical
response paths.

This is a module rather than a new crate because feature gating already
produces the complete dependency saving: the response-only Cargo tree is the
four-dependency envelope above, with none of Routerama's macro or matching
packages. Response composition shares Routerama's vocabulary, documentation,
version, and release lifecycle, and `route` consumes the exact same public
traits. A separate package would not remove another dependency from
response-only consumers; it would add version coordination, publishing
surface, and a generated-code dependency edge. There is no independently
versioned cross-framework contract that outweighs those costs, so a module is
the smaller public architecture.

### Phase 2: request parts

- Change generated `route` to accept `http::Request<B>`.
- Add method, URI, version, headers, parts, state, extension, and query
  extractors.
- Add compile-time route predicates for host, content type, and accepted
  response media types.
- Preserve direct static captures.
- Add extractor ordering and short-circuit behavior.

This phase solves header access and request metadata without introducing body
buffering.

#### Lifetime-aware request-parts findings (2026-07-26)

The request-metadata portion of Phase 2 is now implemented for both generated
static dispatch and startup-configured dynamic dispatch:

- The public synchronous contract is
  `FromRequestParts<'request, S>`. Its input is
  `&'request http::request::Parts`, so an implementation may return a value
  containing that same lifetime. The generated route owns the parts while it
  runs every parts extractor, awaits the optional body extractor, and awaits
  the handler. The compiler therefore keeps metadata borrows live through the
  handler future but prevents them from escaping dispatch. No extractor
  future, allocation, box, unsafe code, or `Send` bound was added.
- Handler source uses normal `&T` elision and `'_` in named types such as
  `UserAgent<'_>` or `Outer<UserAgent<'_>>`. For each unique parts-extractor
  type, the macro recursively substitutes every elided reference lifetime and
  anonymous lifetime argument that belongs to the extractor with one generated
  higher-ranked request lifetime. Callable parameter signatures retain their
  own independently bound elided lifetimes. The extractor bound is the
  equivalent of
  `for<'request> Extractor<'request>:
  FromRequestParts<'request, S, Rejection = R>`. `R` is an inferred generated
  type parameter shared across the higher-ranked family. This both supports
  arbitrary nested custom extractors and proves that the early rejection type
  does not vary with the borrow. The generated method additionally requires
  `R` and its response body to be `'static`, because a short-circuit response
  must not retain request metadata.
- Explicit named lifetimes in a parts-extractor handler type are rejected with
  a focused macro diagnostic. There is no sound general way for the macro to
  decide whether such a name denotes the parts borrow, some outer owner, or an
  independently bound nested lifetime. Application code never writes the
  generated higher-ranked lifetime.
- `routerama::route::RequestParts` is the canonical public re-export of
  `http::request::Parts`. The module also re-exports `Version` and
  `Extensions`. Direct `&Method`, `&Uri`, `&Version`, `&HeaderMap`,
  `&Extensions`, and `&RequestParts` extraction borrows the ecosystem values
  without wrappers or clones. Owned `Method` retains its explicit `clone`,
  owned `Uri` and `HeaderMap` retain their explicit clones, and owned
  `Version` copies the protocol value. `State<T>` retains its `FromRef<S>`
  projection.
- `Query<T>` now requires `T: FromQuery<'request>` rather than an owned-only
  higher-ranked bound. Existing owned query schemas continue to work, while a
  handler may use `Query<Borrowed<'_>>` to borrow fields directly from the URI
  query string when decoding does not require ownership.
- Typed extensions use names that expose ownership:
  `ExtensionRef<'request, T>` performs one `Extensions` lookup and returns the
  stored `&T` without cloning; `ClonedExtension<T>` performs the lookup and
  explicitly calls `T::clone`. Both use `MissingExtension<T>`, a zero-sized
  typed error implementing `Error` and `IntoResponse`. A missing extension is
  deliberately `500 Internal Server Error`, because required request-local
  application data was not installed. Direct `&Extensions` remains available
  when typed lookup policy belongs in the handler.
- Behavioral coverage compares header-value, URI-buffer, extension-value, and
  request-parts field addresses; checks owned and borrowed method/version
  values; keeps borrows live across body extraction and a handler yield; and
  proves static and configured-dynamic custom `UserAgent<'_>` extraction and
  rejection short-circuiting. The allocation tracker records zero allocated
  bytes for a prepared static dispatch using borrowed headers, URI,
  extensions, and full parts.

This is intentionally a synchronous metadata boundary. It is closest to
Axum's parts extractors in capability, but generated direct calls permit
borrowing the HTTP types themselves and avoid an async-trait future. The
position-independent explicit body marker remains a Routerama difference.
Mutable parts extraction, asynchronous authentication, middleware insertion,
and responses that borrow request metadata remain out of scope.
`http::Extensions` itself requires typed values to be `Send + Sync + 'static`;
Routerama adds no stronger bound. The existing service-wide request-body type
unification also remains unchanged.

#### Compile-time route-predicate findings (2026-07-26)

The request-metadata predicate portion of Phase 2 now uses the selected
attribute grammar:

```rust
#[route(
    POST,
    "/items",
    host = "api.example",
    consumes = "application/json",
    produces = "application/json",
)]
```

Configured-dynamic handlers use the same keys after their marker:

```rust
#[route(dynamic, host = "plugins.example", produces = "application/json")]
```

Keys are optional, comma-separated, order-independent string literals with an
optional trailing comma. The parser now classifies `dynamic` structurally
rather than comparing rendered token strings. Duplicate and unknown keys,
malformed literals, invalid authorities, and non-concrete media types fail
during macro expansion. `consumes` and `produces` declarations are exactly one
RFC token, `/`, and one RFC token; wildcards and parameters are deliberately
excluded from the declaration so generated matching and the produced response
header have one unambiguous representation. Direct `#[resolver]` enums reject
all three arguments with an HTTP-specific diagnostic: resolver entries still
receive only method and path. The private resolver enum generated by
`#[router]` safely carries and ignores this metadata while preserving the
existing matcher.

Host matching prefers `Uri::authority()` and consults `Host` only when no URI
authority exists. It validates exactly one complete request authority and
compares it ASCII-case-insensitively, including any explicit port. There is no
default-port or percent-decoding normalization. Registered names, bracketed
IPv6, URI `%25` IPv6 zone identifiers, and IPvFuture literals are supported.
Unbracketed IPv6, userinfo, schemes, paths, whitespace, empty hosts, duplicate
`Host` fields, and invalid ports reject with 404. Routerama refines URI's
syntactic `port = *DIGIT` production to a nonempty decimal `u16` when a colon
is present; this rejects empty and unusable out-of-range ports consistently in
configured and request values.

Consumes requires exactly one `Content-Type` field. Type/subtype matching is
ASCII-case-insensitive; legal OWS, token or quoted parameters, and quoted
escapes are validated without allocation and do not affect the match.
Missing, duplicate, malformed, or nonmatching values reject with 415, without
reading the body.

Produces treats a missing `Accept` as acceptable and linearly parses every
field line and comma-separated member. Exact, type-wildcard, and global ranges,
representation parameters, quoted values, post-weight extensions, and strict
three-decimal `q` values are supported without temporary strings or vectors.
The most specific matching range supplies quality, so an exact `q=0` overrides
an acceptable broader wildcard. A declared produced type has no parameters;
therefore a range carrying representation parameters does not match it,
whereas extensions after `q` do not constrain it. Any malformed line or the
absence of a nonzero matching range rejects with 406.

The generated arm order is fixed: matcher/capture conversion, host, consumes,
produces, all parts extraction, optional body extraction, then the direct
handler call. Predicate rejections use one truthful, fixed-`Body` response-sum
source and do not run extractors or handlers. A produces success converts the
handler result first and then replaces `Content-Type` with a static
`HeaderValue`; concrete streaming bodies, trailers, and errors are only moved
into the existing generated sum. Arbitrary `IntoResponse` conversion erases
whether an application value originated from an `Ok` or `Err`, so the safest
deterministic rule is to apply the declared type to every response from an
invoked handler, regardless of status. Routing, predicate, and extraction
rejections return earlier and never receive that metadata.

Static aliases can share one generated variant only when all three configured
predicate values are identical (key order is irrelevant). Differing aliases
now fail expansion rather than accidentally applying one alias's policy to
another. This does not add overlapping candidates, negotiation-based handler
selection, fallback recovery, or ranking; those remain Phase 4 work.
Configured-dynamic aliases already identify one handler and therefore share
its one predicate set.

Generated services with no predicates contain no predicate response variant,
helper calls, header lookups, predicate branches, or response-header mutation.
Annotated arms call three small private linear scanners as needed. Unit and
behavioral coverage exercises authority precedence/case/ports/IPv6,
Content-Type parameters and malformed/duplicate fields, Accept wildcard and
quality specificity across multiple lines, static and configured-dynamic
handlers, identical/differing aliases, extraction short-circuiting, body and
borrowed metadata/state/query coexistence, and streaming response metadata.
`alloc_tracker` observes zero allocated bytes for both successful and rejected
prepared dispatches carrying all three predicates. A handler response whose
header map has no spare entry can still allocate inside `HeaderMap::insert`
when the required produced header is added; the predicate scanners themselves
allocate nothing, and replacing a prepared entry is allocation-free.

#### Bounded form-extraction findings (2026-07-26)

The first form completion milestone adds
`route::form::Form<T, const LIMIT: usize>` behind an additive `form` feature.
The selected feature graph is:

```text
form ─────> route ─────> response ─────> http + http-body + bytes
  └───────> query ─────> query derive + itoa

json       (not enabled by form)
resolve    (not enabled by form)
```

This preserves `query` as an independent `no_std` codec, keeps ordinary
`route` free of query/form machinery, and avoids a second decoder or any new
Serde dependency. `form` is useful only at the HTTP handler boundary, so its
canonical API remains under `routerama::route::form`; it does not justify
another crate or root module.

Every form body has an explicit const-generic encoded-byte limit. The shared
bounded collector rejects a lower size-hint bound above that limit before
polling, still checks every yielded data frame, accepts an exact-limit and
multi-frame body, preserves the concrete transport error, ignores non-data
frames, and imposes no `Send` requirement. No unbounded form or library default
exists.

Extraction requires exactly one `Content-Type`. Form matching reuses the
private route-predicate media parser, including full token, OWS, quoted
parameter, and escape validation. Type and subtype compare
ASCII-case-insensitively against
`application/x-www-form-urlencoded`; parameters are legal and ignored.
Missing, duplicate, malformed, and valid-but-unsupported values remain distinct
`FormContentTypeError` variants and all convert to 415. The same private parser
now validates JSON parameters as well; no predicate internals became public.

After bounded collection, extraction validates UTF-8 and invokes the existing
`FromQuery` implementation. The implementation bound is
`for<'form> T: FromQuery<'form>`: one fixed output type must decode from every
possible temporary input lifetime. Owned strings, parsed scalars, optional
fields, and repeated vectors satisfy that contract, while a type containing a
reference into the buffered form cannot escape. A dedicated compile-fail case
records that boundary. Query-codec limits and structured errors apply
unchanged. `FormRejection` preserves content-type, body/transport, UTF-8, and
query-codec diagnostics; the latter three decode classes map to 400 except
size overflow, which remains 413.

Behavioral coverage includes empty forms, scalar/optional/repeated fields,
plus and percent decoding, exact and exceeded limits, split frames, early
size-hint rejection without polling, malformed UTF-8/encoding/scalars, all
content-type failure classes, transport failure, a local non-`Send` body,
every body-marker position, direct handler calls, response conversion, and a
route that combines the transitively available query extractor with a form.
A dedicated runnable `form` example covers the public path without a transport
server.

The existing five-framework body fixture is a hand-maintained JSON/bytes/text
comparison, not a generator that can absorb form decoding without adding new
framework-specific schema adapters and changing its implemented-milestone
scope. It is intentionally unchanged. The final performance milestone must add
a separate equivalent form fixture covering a successful owned scalar,
optional/repeated and escaped values, split input, malformed encoding/value,
and limit rejection before making form performance claims.

Milestone validation uses Rust 1.93 for the exact feature sets none,
`response`, `resolve`, `route`, `query`, `json`, `form`, `route+query`, and all
features. The form-only rustc configuration contains exactly `form`, `query`,
`response`, and `route`, proving that it adds neither `json` nor `resolve`.
All Routerama library/integration tests and both all-feature and featureless
doctests pass on 1.93. Strict all-target/all-feature Clippy, docs.rs-style
rustdoc, external-type checking, spelling, Cargo sorting, generated-fixture
freshness, README freshness, formatting, and diff whitespace checks pass.
The existing docs.rs `all-features` metadata includes form automatically, and
the public errors add no external type outside the existing `http::*`
allowance, so neither metadata table needs a new entry.
Release-set packaging of Routerama with its local build/macro crates succeeds;
the package list contains the form source, example, behavioral test, and UI
evidence and contains no LFS-tracked binary asset.

### Phase 3: request bodies

- Add `FromRequestBody` and the `#[body]` parameter marker.
- Enforce exactly zero or one body-consuming parameter independently of its
  position in the handler signature.
- Add raw body, bounded bytes, text, JSON, and form extractors.
- Add configurable body limits and transport-error propagation.

### Phase 4: route policy and dynamic routes

- Add typed fallbacks/catchers for routing and extraction failures.
- Add compile/build-time required-state validation.
- Define explicit priority/ranking only for intentional overlaps that cannot be
  resolved by normal static specificity.
- Add explicit dynamic `#[capture]` parameters.
- Preserve alias registration and capture validation.
- Verify that request extractors do not become dynamic path fields.
- Keep the persistent router/builder model.
- Add an explicit adapter for mounted runtime services whose handlers or body
  types are not statically known.
- Verify that enabling a mounted dynamic service adds no allocations or
  indirect calls to requests resolved entirely by static routes.

#### Route-policy completion findings (2026-07-26)

The typed fallback, extractor-catcher, and intentional-overlap portion of Phase
4 is implemented. Required-state validation is recorded in the following
finding; mounted runtime services remain a separate future milestone.

The selected source grammar is:

```rust,ignore
#[route(
    GET,
    "/items/{id}",
    host = "api.example",
    produces = "application/json",
    priority = 20,
)]

#[fallback]
async fn fallback(&self, failure: RouteFailure<'_>) -> MyResponse

#[catch(QueryRejection)]
async fn catch_query(&self, rejection: QueryRejection) -> MyResponse

#[catch(AuthRejection, from = AuthExtractor)]
async fn catch_auth(&self, rejection: AuthRejection) -> MyResponse
```

`priority` is a signed `i32`; higher values are evaluated first. Zero remains
the effective priority for an ordinary declaration, but every declaration in
an overlapping method/template shape must state an explicit distinct value.
Overlap grouping uses the shared routing trie, so differently spelled
templates that terminate in the same method/verb bucket cannot bypass the
check. One conversion plan serves the group; capture source names, positions,
and concrete Rust types must therefore be identical. A predicate-free
candidate is permitted only at the lowest priority, and duplicate predicate
sets are rejected because they make a lower candidate unreachable.

The generated private resolver receives one representative leaf per overlap
group. Its existing compiled segment/method dispatch is unchanged. The matched
group arm emits a straight-line, priority-ordered predicate chain: host, then
consumes, then produces for each candidate. A mismatch advances to the next
candidate, and parts/body extraction appears only inside the selected branch.
There is no request-time candidate vector, allocation, indirect call, or
handler registry. Non-overlapping declarations retain the previous direct
match arm and do not emit candidate state or policy calls.

When no candidate matches, generated code records the deepest failed stage
reached by any candidate, independently of declaration order. Host-only
failure is 404, any consumes-stage failure takes precedence at 415, and any
produces-stage failure takes precedence at 406. The same typed
`RouteFailure<'request>` covers those classes plus not found, malformed path,
missing capture, invalid capture conversion, and undecodable capture. It
borrows path text and carries static capture names, so no diagnostic is
allocated or erased. `RouteFailure::status` and `IntoResponse` implement the
defaults. One `#[fallback]` may instead receive it by value and return any
concrete `IntoResponse` type asynchronously. HTTP `Uri::path()` is already a
well-formed path, so malformed-path fallback is normally an invariant branch
at this boundary; the public typed class remains complete for matcher
diagnostics.

Catchers are keyed by an exact concrete rejection and called directly at the
generated extraction site. The build macro can recognize Routerama's built-in
extension, query, bounded-body, JSON, and form families. Rust procedural macros
cannot semantically resolve an arbitrary trait associated type, so a custom
extractor supplies `from = ExtractorType`; the generated trait bound then
proves that extractor's associated rejection is the catcher's by-value
argument. Duplicate, ambiguous, unused, generic, borrowed, mismatched, and
recursive/body-marked catchers fail with focused diagnostics. Uncaught sites
retain their prior `IntoResponse` conversion. A concrete bounded-body, JSON,
or form catcher necessarily fixes the transport-error parameter in its
rejection and therefore narrows the generated request-body bound; this is the
sound exact-type consequence of a non-generic policy method, not error
erasure.

Fallback and catcher results are response sources in the existing private
service body and error sums. Their frames, trailers, errors, size hints, and
auto traits are delegated exactly like handler and ordinary rejection bodies.
No body or future is boxed, and policy futures have no `Send` requirement.
Behavioral coverage includes streaming success/trailer and error frames plus a
future/body containing `Rc`.

Static aliases are grouped per declaration shape. An alias participates in
candidate selection only where it collides; another alias on the same handler
can remain a direct route. Aliases still require identical predicate values
because they share one handler policy, while their priorities may differ by
shape. Configured dynamic handlers support the same fallback and catcher calls
and retain their declaration predicates, but `priority` is rejected: dynamic
registrations may not overlap each other or any generated static route.
Builder-time collision validation avoids adding runtime candidate indirection
to static requests.

Tests cover priority ordering, all deterministic predicate failures,
compatible captures, independent aliases, extraction short-circuit side
effects, default and customized routing failures, parts/body/query/JSON/form
catchers, uncaught behavior, static/dynamic/mixed services, streaming/local
policy responses, and zero measured allocations for prepared plain, overlap,
fallback, and catcher dispatch. UI coverage pins malformed/duplicate priority,
missing priority, capture incompatibility, dynamic priority, duplicate
fallback/catcher, unused catcher, mismatched/borrowed/generic signatures, and
catcher extraction recursion. The dedicated runnable example is
`examples/route_policy.rs`.

This milestone deliberately does not implement mounted runtime services,
middleware/interceptors, or Tower integration. It also does not add
status-specific catchers or wildcard rejection matching: policy matching
remains exact and compile-time checked.

#### Required-state validation findings (2026-07-26)

Required-state validation is now an optional specialization of the existing
explicit shared-state model:

```rust,ignore
#[router]                    // route<B, S>(request, &S)
#[router(state = AppState)] // route<B>(request, &AppState)
```

The configured dynamic router uses the same distinction on
`router.route(&service, request, state)`. Bare routers remain generic for
reusable services and retain their previous generated route body, bounds, and
precise opaque `use<B, S, ...>` captures. A fixed router has no state type
parameter and its opaque response captures only request-body and inferred
rejection type parameters. The public state parameter, every extractor bound,
and every generated extraction call use the concrete annotated type.

The attribute accepts exactly optional `state = Type`, with one optional
trailing comma. Qualified, `self`, `super`, associated, and generic concrete
paths are retained in public signatures and the impl-local validation method;
existing capture types are still rebased correctly when copied into the
private generated child module. Explicit `&'static T`, `str`, slices, and
`dyn Trait + 'static` are sound because state is always borrowed and extractor
traits accept `S: ?Sized`. Unknown or duplicate keys, trailing junk,
`impl Trait`, inferred type/const `_`, type macros, `!`, references without an
explicit `'static`, trait objects without `dyn` and `+ 'static`, and explicit
anonymous `'_` are rejected during parsing. `Self` is also rejected because it
would denote the service in one generated impl and the configured router in
another; the named service or a fully qualified associated type is stable
across both contexts. A private concrete type alias
additionally makes omitted lifetime arguments and Rust-invalid unsized forms
fail at the annotation rather than becoming a locally inferred state family.
No context-forwarding grammar was restored.

Simply replacing generic `S` with `AppState` in the existing higher-ranked
where-clause was not sufficient definition-time evidence. Rust accepts an
otherwise unsatisfied
`for<'request> Extractor<'request>: FromRequestParts<'request, AppState>`
where-clause as an assumption on a generic function. The annotated service impl
therefore contains a dead private validation function that calls a generic
assertion once per distinct extractor. The assertion retains the complete
higher-ranked request lifetime and asks rustc to infer one common rejection
type, which forces the compiler to solve the obligation immediately. Missing
same-type `Clone`, missing `FromRef<AppState>`, custom parts extractors
implemented for another state, and catcher-associated extractor mismatches now
fail even if no application calls `route`.

Body extraction has an additional independent request-body type `B`. A generic
`Extractor: FromRequestBody<AppState, B>` bound also cannot prove that any
such implementation exists at definition time, and selecting an arbitrary
probe `B` would reject valid extractors or validate the wrong contract.
Fixed-state custom body extractors therefore implement the zero-method
`BodyStateWitness<AppState, Rejection>` trait and name one concrete
`RequestBody` for which they implement
`FromRequestBody<AppState, RequestBody, Rejection = Rejection>`. The private
assertion checks that exact witness. Built-in raw, bounded bytes/text, JSON,
and form extractors supply compile-only bodies parameterized by the transport
error, so catcher rejection bounds are checked without choosing a real
transport. Actual route calls continue to prove the real request-body type;
the witness neither narrows the route signature nor performs work. Bare
routers do not require this additional trait, preserving their existing
custom-extractor API.

Trait absence is semantic information available only to rustc, not the
procedural macro. The generated assertion names
`FromRequestParts<'static, AppState>` or
`FromRequestBody<AppState, WitnessBody>` directly, so the compiler diagnostic
points to the handler parameter, the generated assertion bound, and the
missing or wrong implementation. Macro-level diagnostics remain responsible
for malformed attribute syntax and ambiguous lifetime forms.

Behavioral coverage includes same-type cloned `State<AppState>`, multiple
`FromRef` projections, a state-dependent borrowed parts extractor, a
state-dependent body extractor and witness, no-state handlers, static-only,
dynamic-only, and mixed fixed services, a non-`Send` `Rc`-containing state,
overlap selection, fallback/catcher policy, and qualified `self`/`super`
generic paths. Existing query, JSON, and form service tests now use fixed
state. A separate bare service is called with unrelated state types. UI
coverage fixes the missing projection, incompatible parts/body state,
malformed arguments, omitted lifetime, and wrong-state call diagnostics.

Generated-source tests show that bare services still contain
`__RouteramaState: ?Sized` and `use<B, S, ...>`, while fixed route functions
contain neither. The fixed-only additions are a private type alias and dead
assertion function. They are never referenced by request dispatch. A paired
layout/allocation test gives otherwise identical fixed and generic services
equal route-future sizes and observes zero allocated bytes while polling both.
There is no runtime type map, validation registry, lookup, allocation, branch,
or per-request witness call. The core still imposes no `Send` bound.

This milestone does not implement runtime mounts, interceptors, middleware, or
Tower integration. `BodyStateWitness` proves one compatible
state/rejection/body triple; each call's existing bound remains the authority
for that call's body type. It is deliberately not a runtime sentinel and
cannot validate state values or application invariants.

#### Explicit runtime-mounted service findings (2026-07-26)

The remaining Phase 4 mount milestone is implemented behind a separate
`mount` feature. It implies `route` but adds no third-party dependency,
procedural-macro feature, or runtime. The canonical API is
`routerama::route::mount`; code generation is selected explicitly in source
with `#[router(state = S, erased_mounts)]`, so Cargo feature unification cannot
silently change an ordinary generated service.

The explicit boundary consists of:

- `MountedService<B, S>` for named concrete implementations;
- `ErasedMountService<B, S>::new` and `from_async_fn` as the visible erasure
  points for named services and async closures;
- `ErasedMountRouterBuilder<B, S>::mount`, where cloning one erased service
  handle registers deterministic method/template aliases; and
- immutable `ErasedMountRouter<B, S>` dispatch.

The builder reuses the existing runtime trie and accumulates invalid RFC 9110
methods, invalid affix-enabled path templates, and all internal method/shape
conflicts in `ConfigurationError`. Registration allocates the service object
and route table at startup. Aliases share the stored service. Mounted
templates deliberately do not declare Rust capture types:
`MountedRequest::raw_capture` and `captures` slice through precomputed byte
ranges without reparsing or allocating, `decoded_capture` borrows when no
percent decoding is needed, and `capture<T>` decodes and parses once on
explicit request. Missing, undecodable, and invalid typed captures form a
compact `MountedCaptureError` with deterministic 400 conversion.

The request body type and shared state are fixed by
`ErasedMountRouter<B, S>`. Matching retains capture offsets, then transfers the
original `Request<B>` into `MountedRequest`; services can consume
`into_request().into_parts()` without body boxing, cloning, or copying.
Services are startup-owned and therefore `'static`. Their call futures may
borrow the service/request/state and, like concrete response bodies, need not
be `Send` or `Sync`. Response bodies and errors must be `'static` because
`BoxBody` owns them.

Every successful mounted call crosses one service vtable, allocates and
dynamically polls one future, and converts the concrete response body exactly
once through `BoxBody`. A body error adds its existing conditional error box.
The shared matcher keeps paths up to its 16-segment scratch boundary inline,
and mounted capture ranges keep up to four captures inline; larger path or
capture sets may add the same documented spill allocations as runtime
resolution. A prepared immediate mounted call with no captures measures
exactly two allocations (future and body). A standalone complete miss invokes
no service and measures exactly one allocation for its boxed fixed 404 body.

Generated integration is intentionally a second named entry rather than a
change to ordinary `route`. With `mount`, fixed-state
`#[router(state = S, erased_mounts)]` services and their configured
service-router types emit `route_with_erased_mounts`. `erased_mounts` without
fixed state is a focused macro error. The existing generated resolver runs
first.
Because borrowed generated captures require stable request parts, a miss
reassembles `Request::from_parts(parts, body)` by move before delegation; it
does not clone, copy, or box either component.
Static handlers and configured `#[route(dynamic)]` handlers remain direct;
only `ResolveError::NotFound` delegates to the mount table. Generated capture
conversion, predicates, extraction, catchers, fallbacks other than a complete
miss, and handler responses are final. This defines cross-table conflicts
without a second validation registry: every generated static or configured
dynamic route deterministically wins over an overlapping mount, while mount
versus mount conflicts fail construction.

The integration response is structurally
`EitherBody<GeneratedBody, BoxBody>`. The generated branch is the existing
private concrete body/error sum and performs no body or future boxing.
Generated-source checks pin that shape. An allocation and call-counter test
polls a prepared static request through a populated mount wrapper and observes
zero allocated bytes, zero allocations, and zero mounted-service calls. The
paired mounted fallback observes exactly the two documented allocations.
Ordinary `route` code generation remains unchanged; fixed-state services
without the marker and all generic-state routers acquire no mount entry or
body-erasure reference.

Behavioral coverage includes standalone calls, zero-copy/decoded/typed
captures, misses, invalid captures, accumulated startup errors, conflicts,
aliases, complete request parts/body/state transfer, custom statuses and
headers, async closures, named services, multi-frame streams, trailers,
stream errors, and local `Rc` futures/bodies. Generated static, dynamic-only,
and mixed routers cover static-first precedence, configured-dynamic
precedence, mounted fallback, borrowed and parsed generated captures,
generated capture failure, and complete miss.
Route-only compile-fail coverage proves an `erased_mounts` source opt-in cannot
name its runtime adapter without `mount`; the mount-only feature selection is
validated separately. Generated runtime
paths retain the existing dependency-alias lookup, and an external fixture
using `rr = { package = "routerama", ... }` compiles the explicitly annotated
fixed-state mounted entry through that renamed dependency.

This milestone does not implement middleware, interceptors, Tower readiness,
or a `Send` transport flavor. A later Tower adapter can impose its own
readiness and auto-trait bounds without changing the local mounted-service
contract.

### Phase 5: middleware integration

- Add request-extension helpers.
- Add generated before/after interceptors with direct concrete calls.
- Include body transforms and terminal consumers in the compile-time ownership
  plan.
- Add optional Tower `Service` adapters. **Delivered:** the separately gated
  `routerama::route::tower` adapter.
- Demonstrate authentication and tracing middleware. **Delivered:** the
  `auth_tracing` example and its structured-field test (see the authentication
  and tracing findings below).
- Validate router-wide and per-handler interceptor ordering.

#### Phase 5 findings

The generated interceptor slice is implemented for the `route` feature with no
new dependency, no crate, no boxed future or service, and no per-request
allocation. Three method attributes on a `#[router]` impl carry it:

- `#[before]` methods return `Before<R>` (`Next` or `Respond(R)`). A bare
  `#[before]` is *router-wide*: it takes `&mut BeforeContext<'_>` (the whole
  mutable request head) and is emitted at every generated entry (`route`,
  `route_with_erased_mounts`, and the configured-dynamic entry) **before** route
  resolution, so it may rewrite the method/URI and also enriches and
  short-circuits mounted delegation. `#[before(handler, ...)]` is *per-handler*:
  it takes `&mut SelectedContext<'_>` and is emitted inside the selected
  dispatch arm, after predicate selection and before extraction.
- `#[after]` methods take `&mut AfterContext<'_>` (immutable request head,
  mutable response head) and return `()`. A bare `#[after]` observes **every
  generated response**; `#[after(handler, ...)]` observes only its named
  handlers' responses and runs first.
- `#[transform(limit = N, handler, ...)]` methods take `&RequestParts` and one
  buffered `bytes::Bytes`; `#[transform(stream, handler, ...)]` methods are
  generic over the transport body and take it by value. Both return
  `BodyTransform<B, R>` (`Replace(B)` / `Respond(R)`) or `BodyConsumed<R>`
  (`Consumed` / `Respond(R)`). They are the terminal request-body owner for
  their named handlers.

Ordering is deterministic and documented: router-wide `#[before]` (declaration
order) → per-handler `#[before]` (declaration order) → `#[transform]` →
extraction → handler → per-handler `#[after]` (declaration order) →
generated-wide `#[after]` (declaration order). A `#[before]`/`#[transform]`
short-circuit skips the handler, extraction, and every per-handler `#[after]`,
and is still observed by a bare `#[after]`.

**Body ownership is genuinely codegen-solvable in both directions.** Bounded
buffering collects the generic transport body through `collect_body` (bounded by
the explicit `limit`) and returns a *concrete* replacement `B2`. Streaming keeps
the generic: the interceptor declares exactly one generic parameter, takes the
transport body by value, and returns a replacement expressed in that parameter
(`BodyTransform<Wrapper<B>, R>`); the macro substitutes the entry's
`__RouteramaBody` for `B`, so the handler's `#[body]` bound becomes
`FromRequestBody<S, Wrapper<__RouteramaBody>>` and the direct call stays
monomorphized, unboxed, and `Send`-free. Nothing is buffered unless a route
explicitly asks for it, which is what decompression, signature verification, and
metering middleware need. A consuming transform (`BodyConsumed`) that names a
handler with `#[body]` is a compile error in both modes; each handler is
transformed at most once; parts-only `#[before]`/`#[after]` cannot observe or
consume a body at all.

Strict signature grammar backs this: the streaming mode requires exactly one
generic type parameter (no lifetimes or consts) used as the body argument type,
rejects a short-circuit response that depends on that parameter, and rejects
`limit` and `stream` together; the buffered mode rejects generics and requires a
`bytes::Bytes` body argument. Both require `&RequestParts` first, and each
diagnostic names the fix.

**Request-extension helpers** are `BeforeContext`/`SelectedContext`/
`AfterContext`, which wrap the mutable request head (respectively the split
request head, and the immutable request head plus mutable response head) and
expose ergonomic typed `insert_extension`/`get_extension`/`remove_extension`
plus metadata accessors, with `http::Extensions`' exact bounds. Extractors on
unused routes still pay no type-map lookup.

**Borrow-checker interaction with zero-copy captures is solved by a field
split, not by a restriction.** A router-wide `#[before]` runs before any capture
exists, so it owns the whole head. After selection, the request URI backs the
route's zero-copy captures, so `SelectedContext` borrows the head by field:
`&method`, `&uri`, `version`, `&mut headers`, `&mut extensions`. A per-handler
guard therefore authenticates, enriches extensions, and normalizes headers while
its handler still receives borrowed `&str` captures and `ExtensionRef`
parameters; only method/URI mutation is unavailable there, and after selection
that could not change routing anyway. Transforms and `#[after]` read the request
head immutably and compose with borrowed captures. `parts` is bound `mut` only
when a `#[before]` exists.

**`#[after]` scope is exact, and named for what it delivers.** With a bare
`#[after]`, the entry lowers its resolution and dispatch into one labeled block
whose value is the generated response; every stage that used to `return` now
breaks to it. The interceptors then observe handler responses,
`#[before]`/`#[transform]` short-circuits, request-parts and request-body
extractor rejections, `#[catch]` responses, predicate rejections, and routing
failures or `#[fallback]` responses — decomposing the response into parts and
moving the original body back unchanged, so streaming frames, trailers, and
error types survive. It is documented as *generated-response-wide* rather than
router-wide because one case is genuinely excluded: a mounted service's response
(and the mount table's own `404`), since `route_with_erased_mounts` moves the
request head into that service and `AfterContext` borrows it. Redesigning the
mount interface to borrow the head instead would remove `MountedRequest::
into_request`, the ownership transfer mounts exist for, so the boundary is kept
and stated wherever `#[after]` is described. A service without a bare `#[after]`
keeps the previous `return`-based lowering byte for byte.

**Mounts.** Router-wide `#[before]` guards and enriches mounted delegation
because it runs before resolution. Per-handler `#[before]`, `#[transform]`, and
every `#[after]` are generated-handler concerns only.

**Zero-cost preservation.** A service with no interceptor annotations emits
byte-for-byte the previous dispatch (verified by codegen assertions plus the
unchanged snapshot tests and the full existing router test suites). The
interceptor sources only join the response body sum when a
`#[before]`/`#[transform]` exists, a streaming transform adds no buffering
rejection source at all, and allocation counters prove that a passive
`#[before]`/`#[after]` pair and a guarded, streaming, observed route both add
zero bytes on the generated static path.

**Not done in this milestone:** the optional Tower `Service` adapters were
delivered separately (see the Tower transport adapter findings below), and no
interceptor benchmark is checked in yet. A tracing `#[before]`/`#[after]` pair
is demonstrated behaviorally here (span-like enrichment through extensions and
response-header stamping) rather than pulling in a `tracing` dependency; a real
`tracing` demonstration followed separately as a dev-only example and test (see
the authentication and tracing findings below), and the library still emits no
telemetry and depends on no telemetry crate.

#### Tower transport adapter findings (2026-07-26)

Phase 5's last bullet is implemented behind a separate `tower` feature
(`tower = ["dep:tower-service", "route"]`). It implies `route`, adds only the
`tower-service` trait crate, and lives at `routerama::route::tower` with no
crate-root re-export. **No code generation changed.** The adapter never names a
generated type, so there is no new macro attribute, no `__private` runtime
entry, and no renamed-dependency lookup to get wrong; a renamed dependency
works because application code simply names `rr::route::tower::RouteService`.

The surface is one service, one future, and one boundary trait:

- `RouteService<Service, State, Call, Boundary = ExactBody>` stores the
  concrete router, concrete state, and concrete callable. It adds no `Arc`, no
  trait object, no type map, and no per-call vtable of its own;
- `RouteFuture<Fut, Boundary>` is a named, pin-projected wrapper holding the
  callable's own future **inline**. Nothing is boxed; and
- `NormalizeResponse<R>` with the `ExactBody`, `SendBoxedBody`, and
  `LocalBoxedBody` markers selects the response boundary. The trait is open, so
  an application can normalize into another transport's body type directly.

**A closure-backed adapter is genuinely sufficient, and lending futures are not
needed.** `tower_service::Service::call` takes `&mut self` but its `Future` is
an associated type with no lifetime parameter, so a returned future can never
borrow the service. Expressing "callable takes `&Service`/`&State` and returns
a future borrowing them" would therefore be useless even if `AsyncFn`'s lending
`CallRefFuture` could express it: the borrow could not escape `call`. The
adapter instead hands the callable **owned clones** — `Fn(Service, State,
http::Request<B>) -> Fut` — so the future owns everything it needs and plain
`Fn` bounds suffice on Rust 1.93. Applications choose the sharing strategy: a
zero-sized router and a `Copy` state clone for free, and an `Arc` clone is one
atomic increment. No RPITIT/GAT trait was required, and adding one would have
forced a boxed `Service::Future` because an `async fn` future cannot be named
by an explicit associated type.

`RouteService::new` carries the full `Fn` bound so closure parameter types are
inferred from the expected type. The request-body type `B` appears only in that
bound, so the closure's request parameter is annotated (or the transport that
consumes the service fixes it); this is documented on the constructor.

**The adapter covers every routing entry** because it wraps a call rather than
a type: generated static `route`, a configured dynamic/mixed
`Router::route(&service, request, &state)` (one `Arc<(Router, Service)>` keeps
that a single clone), `route_with_erased_mounts`, and a standalone
`ErasedMountRouter::route`.

**Readiness is accurate rather than decorative.** Generated routing owns no
permit, queue, or connection, so `poll_ready` is always `Poll::Ready(Ok(()))`
and never errors. It is not a lie about a deferred resource. A test drives a
`tower::limit::ConcurrencyLimitLayer` above the adapter and observes the
layer's own `Poll::Pending`, proving the adapter neither masks nor duplicates
real readiness. `Clone` is implemented whenever the three stored values are,
which is what Hyper's per-connection and Axum's per-request cloning need;
cloned adapters share exactly what the application shared.

**Errors.** `Service::Error` is `Infallible`: every routing failure, predicate
rejection, extractor rejection, and mounted miss is already an HTTP response.
Body errors stay body errors and surface while the body is polled.

**Auto traits are inherited, not imposed.** The impl adds no `Send`, `Sync`, or
`'static` bound, so `RouteService` is `Send`/`Sync` exactly when its three
stored values are and `RouteFuture` is `Send` exactly when the routing future
is. The core route contract therefore stays local and `Send`-free, and the
transport flavor's requirements are paid at one explicit place: the response
boundary.

**Response erasure is the one measured cost.** A generated router's response
body is a private concrete sum behind an opaque `impl Body`, so it cannot be
*named* even when it is already `Send`. `ExactBody` keeps it and adds nothing —
that is enough wherever the response type is never written down. Naming it
requires an erasure, so `response` gained `SendBoxBody`/`SendBoxBodyError`
alongside the existing local `BoxBody`: a `Send + 'static` body whose error is
`Send + Sync + 'static`, and therefore convertible into the
`Box<dyn Error + Send + Sync>` Hyper and Axum expect. `LocalBoxedBody` reuses
`BoxBody` so a mount integration's structural `EitherBody<Generated, BoxBody>`
becomes nameable without acquiring a `Send` bound the mount core deliberately
does not have. Normalization runs exactly once, after the routing future
resolves, and never touches the request. Data frames, trailers, size hints, and
end-of-stream state are forwarded unchanged.

Allocation counters pin the numbers: the same generated static request measures
**zero** allocations through `ExactBody` and **exactly one** through
`SendBoxedBody`. The one allocation is the boxed body; a body error boxes only
if it occurs. One limitation is recorded honestly: through a generated router
the erased error is the generated response-body error sum, which implements
`Error` with the default `source()`, so the concrete body error's *type* is not
recoverable after erasure even though its identity is named in the message.
Erasing a concrete body directly does keep it downcastable.

Behavioral coverage includes generated static routing and misses, configured
dynamic plus mixed routing through one shared handle, standalone erased mounts
and the static-first mount integration through the local boxing boundary,
always-ready readiness under a real readiness-bearing layer, 16 concurrent
requests through cloned services on a multi-threaded runtime, `Send`/`Sync`/
`'static` compile assertions for the transport flavor, multi-frame streams with
trailers and a mid-stream error through the erasure, a Tower `MapRequestLayer`
feeding a generated router-wide `#[before]` guard that promotes an
authentication extension a handler then extracts (and the `401` short-circuit
without the layer), and the allocation counts above. Compile-fail coverage
proves the module cannot be named without the feature and that the local
`BoxBody` is rejected by the `Send` transport boundary. The runnable example is
`examples/tower_service.rs`, which composes `ServiceBuilder` concurrency-limit,
map-request, and map-response layers and serves the same value over Axum.

This milestone deliberately does not add a `tower::Layer` implementation, a
`Send` erased-mount boundary, or Tower-side benchmarks. A `Send` mount would
require a parallel `SendErasedMountService` in the mount core; the local
contract is unchanged and erased mounts compose through `ExactBody` or
`LocalBoxedBody` today.

#### Authentication and tracing findings (2026-07-26)

Phase 5's demonstration bullet is now closed with **no library change at all**:
`tracing` and `tracing-subscriber` are dev-dependencies used by
`examples/auth_tracing.rs` and `tests/router_auth_tracing.rs`, so the published
crate still emits no telemetry, depends on no telemetry crate, and keeps its
`route` feature surface unchanged. No helper was added to the interceptor
contexts; the demonstration is expressible with the shipped API.

**The composition is performance-first.** A Tower `MapRequestLayer` assigns a
`CorrelationId` extension at the transport edge (from an `x-request-id` header
or a counter), because the id must exist before anything else runs and may come
from a proxy. The router-wide `#[before]` then opens one `tracing` span, stores
it in the request extensions, authenticates a bearer credential, and either
inserts a typed `Principal` or short-circuits with `401`. The handler declares
`ExtensionRef<'_, Principal>` and `ExtensionRef<'_, RequestSpan>` plus a
borrowed `&str` capture, so it observes the principal and the span by reference:
no clone, and no type-map lookup beyond the extraction it requested. The
response body is boxed once through `send_boxed_body()` only because the
example's `stack()` signature must name the response type.

**A span cannot be transparently held across the handler await, and the example
does not pretend otherwise.** Interceptors are ordinary `async` methods: a
`#[before]` returns before dispatch, so an `Entered` guard taken there would be
dropped immediately, and holding such a guard across an `await` is unsound
practice regardless. The workable pattern, and the one demonstrated, is to carry
the `Span` handle (`Clone + Send + Sync + 'static`, which is exactly what
`http::Extensions` requires) as a typed request extension and enter it
explicitly at each site: `Span::in_scope` for synchronous emission in the
`#[before]`, the handler, and the `#[after]`, and `Instrument::instrument` for
the handler's own future, which enters on every poll and exits on every yield.
No Tower tracing layer is needed for correlation, and none is used.

**`#[after]` is where the response record belongs, and its scope is exactly what
the demonstration needs.** Because the span is inserted *before* the credential
check, the bare `#[after]` re-enters it through the immutable request head it
borrows and emits one `http.response` event for every generated response:
handler `200`s, the `401` short-circuit, a `404` routing miss, and a `400`
capture-extraction rejection are all correlated to the same span, and all carry
the correlation header. A mounted service's response remains outside that scope,
which the example documents and cross-references rather than papering over.

**Testing asserts structured fields, not formatted lines.** The test binary
follows `docs/tracing-tests.md`: it calls `testing_aids::init_tracing!()` (with
the `ctor` dev-dependency, ignored by `cargo-machete`) so trace-event lines
count as covered, and installs a thread-local `tracing_subscriber` registry via
`set_default` — no global subscriber and no `#[serial]`. A small recording
`Layer` captures each event's metadata name, level, and fields together with its
enclosing span's fields, so assertions read `http.status`, `principal.name`, and
the span's `correlation` directly. That also proves correlation structurally:
every event, including the one emitted from the `instrument`ed handler future,
reports the same `http.request` span. Coverage spans authenticated success,
missing and unknown credentials, the short-circuit, a routing miss, a capture
rejection, an authentication-exempt public route, and per-request id uniqueness.

### Phase 6: hardening and documentation

- Add compile-fail coverage for handler grammar and trait requirements.
  **Delivered:** `tests/ui` grew grammar, route-policy, fixed-state, and
  interceptor/policy cases, and `routerama_build` gained message assertions for
  the diagnostics whose spans are not snapshot-stable.
- Add examples for headers, cookies, JSON, streaming bodies, custom
  rejections, and response headers. **Delivered** as the runnable
  `request_metadata`, `request_predicates`, `json_api`,
  `streaming_responses`, and `response_composition` examples, alongside the
  existing `route_policy` custom-rejection example. **Withdrawn:** cookies.
  Routerama has no cookie capability (open decision 6), so an example would
  have to demonstrate a hand-rolled header parse that Routerama does not own.
- Benchmark routing alone and end-to-end extraction separately. **Delivered:**
  `docs/PERF.md` separates routing, dispatch, body extraction, form extraction,
  response bodies, route policy, mounts, interceptors, and the Tower boundary,
  each with a paired Criterion and Callgrind workload, plus the concurrent
  CPU-bound throughput fixture. No promised performance workload remains
  unmeasured.
- Add a separate five-framework form-extraction fixture with equivalent
  schemas, encoded bodies, limits, responses, and malformed-input policy.
  **Delivered:** `benches/common/form_extraction_scenarios.rs` with the paired
  `routerama_form_extraction`/`routerama_form_extraction_cg` benchmarks and the
  `tests/form_extraction_fixtures.rs` equivalence test. Its one deliberate
  exclusion, invalid UTF-8, is documented in both `docs/PERF.md` and
  `docs/TODO.md`.
- Document allocation behavior and body limits. **Delivered:** limits are part
  of every body extractor's type, `docs/PERF.md` records the measured
  allocation counters for bodies, mounts, interceptors, and the Tower boundary,
  and the module docs state where each allocation occurs.
- Review generated symbol hygiene and feature combinations. **Delivered:** the
  full feature powerset checks clean, per-feature Clippy is clean, and
  `cargo package --list` contains no unpackaged or LFS-tracked path.
- Document and test every independently enabled capability without relying on
  examples from a larger feature. **Delivered:** `tests/feature_gates.rs`
  asserts both directions of the `route`, `mount`, and `tower` boundaries, and
  `response_composition` is a runnable example that needs only `response`.

#### Phase 6 findings

**Compile-fail coverage is split by span stability, not by convenience.**
`syn` joins a multi-token span only when the compiler *running* the proc macro
exposes `proc_macro::Span::join`. A diagnostic that points at a whole type,
parameter, or generics list therefore renders one caret under the stable MSRV
toolchain and a full underline under the nightly toolchain used by the
`careful` job, so its `.stderr` snapshot cannot be correct for both. Every
`tests/ui` case now pins a single-token span; the multi-token grammar rules
(handler generics, generic impl blocks, `#[body]` plus `#[capture]`, and
`impl Trait` responses) are asserted by message in `routerama_build`'s
expansion tests, which are identical on every toolchain. Both layers are kept
deliberately: the snapshot proves the rendered diagnostic in a real
compilation, the unit test proves the rule.

**Feature-off diagnostics need their own target.** The workspace test command
is `--all-features`, which silently compiles out any `#[cfg(not(feature =
...))]` `compile_fail` case. `tests/feature_gates.rs` collects all three
boundary cases (`route`, `mount`, `tower`) and pairs each with a positive test
that names the module's public types when the feature *is* enabled, so every
feature selection asserts one direction. The `route`-off case additionally
required `required-features = ["response"]` on the target, and the `mount`-off
and `tower`-off cases are gated on `route` because their pinned diagnostic
assumes `routerama::route` exists.

**Intra-doc links must be gated as carefully as the items they name.**
Building the crate docs under partial feature selections exposed four links
that resolved only when an optional feature happened to be on:
`route::RouteFailure` in the crate docs, and `mount`, `tower::RouteService`,
and `tower_service::Service` in the `route` module docs. They now use the same
absolute `docs.rs` form the rest of those documents already used for
cross-feature references, and `RUSTDOCFLAGS="-D warnings" cargo doc` is clean
for every individual feature as well as for `--all-features` under the
`docsrs` cfg.

**The flagship macro pages carry runnable examples again.** A procedural-macro
crate cannot depend on the crate whose runtime its expansion names, and the
workspace forbids dependency cycles, so every example in `routerama_macros`
was ` ```ignore `. Two of them were worse than untested: the `FromQuery`
`compile_fail` cases "passed" because `routerama` was not a dependency at all,
not because the derive rejected them. Rustdoc merges a doc comment written on
a `pub use` with the re-exported item's own documentation, and doctests
written there run as part of the re-exporting crate's doctest suite. The
runnable examples therefore moved onto `routerama::route`'s and
`routerama::query`'s re-exports, where they compile, run, and reject for the
right reason; the macro crate keeps the prose plus its intentional ` ```text `
grammar illustrations and points at the tested page.

**Examples cover one capability each and prove it by assertion.** The five new
examples exit non-zero on any behavioral change: `request_metadata` asserts
pointer identity between a borrowed capture, a borrowed header slice, a
borrowed extension, and the request head; `request_predicates` walks the
404/415/406 ladder and shows that acceptability is a per-route predicate while
`priority` chooses between candidates; `json_api` pins all four `Json`
outcomes through a `#[catch]`; `streaming_responses` drains frames, trailers,
and a mid-stream body error; and `response_composition` needs only the
`response` feature, which keeps that capability honest about being
independently selectable.

**Two facts surfaced while writing them, and both are documented rather than
papered over.** `Accept` quality values do not reorder overlapping candidates —
a higher-priority route that is merely acceptable still wins, so a client
steers selection by refusing a representation with `q=0`. And the generated
response-body error sum reports *which* response failed instead of forwarding
the handler's own message, because it implements `Error` with the default
`source()`; whether it should forward `source()` is still the open item in
`docs/TODO.md`.

#### Performance-evidence findings (2026-07-26)

**The form fixture reaches equivalence without weakening any framework.** All
five applications decode the same three-field schema from the same encoded
bytes under the same 64-byte limit and the same required media type, and each
maps its own typed error values onto one shared 400/413/415 policy. Two needed
explicit mapping because their defaults differ: Axum reports a form decode
failure as 422 and Rocket as 422, so both are mapped to the fixture's 400 in
application code, which is the technique the JSON fixture already uses for
Actix Web and Rocket. Exactly one scenario had to be excluded rather than
reconciled: Routerama rejects a non-UTF-8 encoded form with 400 while
`form_urlencoded` and Rocket's `url_decode_lossy` substitute U+FFFD and
succeed, and no fixture-level policy fixes that without replacing the
framework's decoder.

**Overlap and media-type negotiation are the only route-policy costs worth
naming.** A typed `#[fallback]` costs 82 instructions over the generated default
404 and a typed `#[catch]` costs 140 over letting the rejection answer itself;
neither allocates. Declaring `consumes`/`produces` costs an order of magnitude
more, and the measurement shows why: rejecting an unacceptable `Content-Type`
costs 1,247 instructions with no allocation, while *accepting* and writing the
negotiated `Content-Type` costs 3,331 and 2 allocations, of which the two
allocations are `http::HeaderMap`'s own first-insert storage. The advice that
falls out is to declare predicates only where they are needed.

**Interceptor overhead is linear and small, and only buffering costs memory.**
The first `#[before]` costs 30 instructions and later ones about 7 each; the
first `#[after]` costs 58, which includes the entry's single
`into_parts`/`from_parts` round trip, and later ones about 10 each. None of the
six rows allocates. A streaming `#[transform]` costs 66 instructions (5%) over
unwrapped bounded extraction with no extra allocation, while a buffering
`#[transform]` costs 542 (44%) and one extra allocation because the fixture
deliberately double-buffers: the transform collects the body and the handler's
`#[body]` parameter then extracts the replacement.

**Erasure is the only expensive boundary, and it is always opt-in.**
Configuring erased mounts costs a generated static request 69 instructions
(8.8%) and no allocation. An erased mounted hit costs 2.4x the generated
configured-dynamic hit that answers through the same entry, with two
allocations. The Tower adapter's identity boundary costs 22 instructions (3.2%)
over calling `route` directly and allocates nothing; `SendBoxBody` costs 141 in
total, or 21% over direct routing, and exactly one allocation. Nothing forces a
caller into any of those.

**Generated code is bounded and mostly fixed.** One generated route expands to
19,778 bytes of macro output and adds 16,320 bytes of `.text`; each additional
route adds about 1,657 expanded bytes and 1,136 bytes of loadable code. Compile
time behaves the same way: against a roughly 6.3-second floor that is dominated
by rebuilding the library itself, one route costs 0.39 s and four cost 1.36 s.

**Throughput is measured, in-process and concurrently, and the gate's scope is
revised in the open.** Every other fixture is a single in-process request per
iteration; `benches/routerama_throughput.rs` adds a share-nothing
thread-per-core driver in which five frameworks and one no-framework control
run the identical deterministic CPU-bound handler under the identical
concurrency and request counts. Routerama leads every row: 4.80 M req/s
against Axum's 2.66 M when the handler's work is comparable with dispatch
cost, and 1.242 M against 1.012 M when it is ten times heavier. There is no
transport in the measurement and none is claimed; the reason transport
equality is not controllable across these five subjects, and the resulting
scope revision, are recorded with the gate list above and in `docs/PERF.md`.

**The runtime matcher no longer depends on registration position.** The wider
mount matrix measured 16-, 128-, and 1,024-entry mount tables and found the one
result that got worse with scale: a hit on the last-registered entry of a
1,024-entry table cost 43,264 instructions against 2,386 on the first, because
the descent scanned a node's sibling literal edges. The matcher now chooses per
node: below sixteen sibling literals it keeps the weight-ordered scan it always
had, and at or above sixteen `RtNode::compile` sorts that node's keys once and
`Walk::descend_iterative` binary searches them. Keys are unique within a node,
so ordering is a lookup heuristic and precedence, captures, and backtracking
are unchanged. The same table now costs 2,832/2,829/2,843/1,618 instructions at
its first, middle, last, and missing entry — a 93.4% cut on the worst row, no
new allocation, no new dependency, no extra per-node state, and generated
routers untouched. Sixteen is the measured crossover, not a guess: the matrix
was extended to eight widths and run with each strategy forced everywhere, and
`docs/PERF.md` records the full decision table together with the before-fix
evidence.

#### Validation matrix coverage

Every row of the validation matrix in the next section is proven by a test, an
example, or a doctest in the tree. The comparison-benchmark rows are proven by
checked-in fixtures whose equivalence is asserted by a test and whose measured
results are recorded in `docs/PERF.md`.

| Matrix row | Where it is proven |
| --- | --- |
| static/dynamic/mixed routers, captures, extractor counts | `tests/router.rs`, `tests/router_borrowed.rs`, `tests/dynamic_parity.rs`, `routing`/`dynamic_routing`/`hybrid_routing` examples, `route::router` doctest |
| body markers, ownership, size limits, streaming bodies | `tests/router_body.rs`, `tests/ui/duplicate_body_markers.rs`, `routerama_build` grammar tests, `json_api`/`form` examples |
| short-circuiting, heterogeneous responses, status/header composition, failed parts conversion | `tests/response.rs`, `tests/router_response_body.rs`, `response_composition`/`streaming_responses` examples |
| query extraction with and without `query` | `tests/query.rs`, `tests/router_query.rs`, `query_strings`/`web_app` examples, `feature_gates.rs` |
| extensions, custom rejections, catchers, fallbacks | `tests/router_policy.rs`, `request_metadata`/`route_policy` examples |
| host, content-type, accepted-media predicates | `tests/router_predicates.rs`, `request_predicates` example |
| overlap priorities and exact policy diagnostics | `tests/router_policy.rs`, `tests/ui/overlap_*.rs`, `tests/ui/predicate_free_overlap_priority.rs`, `tests/ui/identical_overlap_predicates.rs`, `tests/ui/duplicate_route_alias.rs` |
| bare and fixed-state services, witnesses, malformed-state diagnostics | `tests/router_required_state.rs`, `tests/ui/missing_state_projection.rs`, `tests/ui/incompatible_*_state.rs`, `tests/ui/wrong_fixed_state_call.rs`, `tests/ui/erased_mounts_without_state.rs`, `required_state` example |
| body-transform ownership and conflicts, interceptor scope and ordering | `tests/router_interceptors.rs`, `tests/ui/transform_*.rs`, `tests/ui/interceptor_on_policy.rs`, `interceptors` example |
| authentication guard plus request-span correlation | `tests/router_auth_tracing.rs`, `auth_tracing` example |
| mounted services, startup validation, transfer, streams, trailers, errors | `tests/mount.rs`, `mounted_services` example |
| Tower adapter over every routing flavor, auto traits, allocation counters | `tests/tower.rs`, `tests/ui/tower_non_send_state.rs`, `tower_service` example |
| renamed dependencies in generated code | `routerama_build`'s `renamed_dependency_paths_include_the_owning_module` test over every runtime capability |
| no-default-feature, response-only, and per-feature builds | `tests/feature_gates.rs`, `cargo hack --feature-powerset`, per-feature Clippy and rustdoc |
| doctests and generated README content | `cargo test --doc` under all and default features, `cargo doc2readme --check` |
| comparison benchmarks against Axum, Actix Web, Rocket, and Warp | `tests/http_dispatch_fixtures.rs`, `tests/http_dispatch_scaling_fixtures.rs`, `tests/body_extraction_fixtures.rs`, `tests/form_extraction_fixtures.rs`, `tests/throughput_fixtures.rs`, with results in `docs/PERF.md` |
| concurrent CPU-bound throughput against the same four frameworks | `tests/throughput_fixtures.rs` and the `routerama_throughput` benchmark, with results, both measurement methods, the shape sweep, and the in-process scope statement in `docs/PERF.md` |
| five-framework form-extraction fixture | `benches/common/form_extraction_scenarios.rs`, `tests/form_extraction_fixtures.rs`, `routerama_form_extraction`/`_cg` benchmarks, results in `docs/PERF.md` |
| route policy, mount, interceptor, and Tower performance evidence | `tests/route_policy_fixtures.rs`, `tests/mount_fixtures.rs`, `tests/interceptor_fixtures.rs`, `tests/tower_fixtures.rs`, with paired `routerama_route_policy`, `routerama_mount`, `routerama_interceptors`, and `routerama_tower` benchmarks and results in `docs/PERF.md` |
| mounted literals, captures across the inline boundary, misses, streams and stream errors, 16/128/1,024-entry tables, and both sides of the 16-segment scratch boundary | `tests/mount_fixtures.rs` (equivalence, mounted-call counts, the named future/body/error/scratch decomposition, and the five differential pairs that pin it) and the paired `routerama_mount`/`routerama_mount_cg` benchmarks, with results in `docs/PERF.md` |
| sibling-literal lookup across the sorted/scanned fanout boundary: wide tables probed first, middle, last, and missing; wide nodes mixed with affix, single-wildcard, and rest siblings; backtracking out of a wide node; keys that prefix each other; nodes whose subtree weights disagree with key order | `src/literal_edge.rs` and `src/rt_node.rs` unit tests (hybrid lookup against a linear reference, and the ordering each fanout selects), `src/walk.rs` descent tests, the `wide_literal_fanout_matches_the_static_router` Bolero differential in `tests/bolero_routerama_resolver.rs`, and the wide-table alias and conflict tests in `tests/mount.rs` |

#### Cross-toolchain lint portability (2026-07-27)

Routerama requires `async fn` handlers, so every generated service carries a
handler-level suppression for clippy's unused-`async` diagnostic. That
diagnostic is *not* one lint across the toolchains this repository pins: clippy
0.1.96 (`RUST_LATEST`) reports it as `clippy::unused_async`, while clippy 0.1.98
(`RUST_NIGHTLY`) split the trait-impl case out into
`clippy::unused_async_trait_impl` — and the `#[router]` macro relocates handlers
into a generated trait impl, so it is the *split* lint that fires on the newer
toolchain. Naming only the new lint makes `RUST_LATEST` fail
`unknown_lints`; naming only the old one makes the nightly toolchain fail the
split lint; and `#[expect]` cannot bridge the two because an expectation that no
longer matches is itself a warning.

Every such suppression is therefore an `#[allow]` — not an `#[expect]` — that
names `unknown_lints` alongside both clippy lints and `clippy::allow_attributes`
(the workspace idiom for a deliberate `allow`, as in `fetch_tls`). The same form
is emitted by the two fixture generators under `tests/support/`, so the
checked-in generated benches stay byte-identical to their generators. This is
the general rule for any future suppression here: a lint whose name differs
across the pinned and nightly toolchains must be suppressed with `allow` plus
`unknown_lints`, never with `expect`.

## Final status

**Phases 0 through 6 are complete.** Every phase deliverable listed above is
implemented in the tree, every row of the validation matrix in the next section
is proven by the coverage table above, and every performance workload the plan
asked for is measured and recorded in `docs/PERF.md`. All six acceptance gates
are **Met** as of the 2026-07-27 evaluation in `docs/PERF.md`, with one gate —
throughput — explicitly *rescoped* from end-to-end to concurrent in-process
CPU-bound work before it was evaluated, for the reason stated there and in
`docs/TODO.md`. The threshold was not weakened, and no number in this repository
may be described as transport-level.

**The dated `#### ... findings` sections above are historical snapshots, not
current status.** Each records what was true at the checkpoint named in its
heading, including work that was open *then*. Where such a section says a gate
"remains open" or that a fixture is "deferred" — for example the 2026-07-25
initial controlled measurements on the zero-allocation gate, and the paired
Criterion/Callgrind fixtures deferred by the Phase 1 prototype checkpoint —
later phases closed it; the acceptance-gate tables in `docs/PERF.md` are
authoritative for the current verdict.

**What is genuinely still open is not phase work.** It is recorded in
[`docs/TODO.md`](docs/TODO.md) and is unchanged by this status: open design
questions (response interceptors for mounted services, a `tower_layer::Layer`
impl, a `Send` boundary for erased mounts, `source()` forwarding on the
generated body-error sum, and whether that sum should become an exported named
type), the deliberate exclusions (cookies, multipart, typed headers,
WebSockets, a loopback-transport throughput benchmark, the invalid-UTF-8 form
row, and `routerama_build`'s partial-feature dev targets), the query-codec
follow-ups, and path canonicalization. Open decisions 2 and 9 below remain
**partially resolved** on purpose: each names the measurement or the production
feedback that would settle it, and neither is a missing deliverable.

## Validation matrix

The implementation is not complete until it covers:

- static-only, dynamic-only, and mixed routers;
- owned, borrowed, decoded, and parsed captures;
- zero, one, and many request-parts extractors;
- zero or one explicitly marked body extractor and compile failures for
  duplicate `#[body]` parameters;
- extractor short-circuiting without handler invocation;
- heterogeneous handler responses;
- status and header composition;
- failed response-parts conversion;
- query extraction with and without the `query` feature;
- request extension insertion and extraction;
- custom rejection conversion;
- host, content-type, and accepted-media route predicates;
- typed fallbacks/catchers and intentional overlap priorities;
- exact policy diagnostics for duplicate/unused catchers, malformed or
  duplicate priorities, and incompatible overlap capture schemas;
- generic bare and concrete fixed-state services, including definition-time
  `FromRef`, custom parts/body extractor witness, policy, wrong-call, and
  malformed-state diagnostics;
- body-size enforcement;
- streaming bodies;
- body-transform interceptor ownership and conflicts with handler `#[body]`,
  in both the bounded-buffering and streaming-wrapper modes;
- per-handler request guards composed with zero-copy borrowed path captures;
- the exact response-interceptor scope, including handler responses,
  short-circuits, extractor rejections, predicate rejections, routing failures,
  typed fallbacks, and the excluded mounted-service response;
- an authentication guard plus request-span correlation, including the
  authenticated, unauthenticated, unknown-credential, routing-miss, extraction-
  rejection, and authentication-exempt outcomes, asserted on structured
  `tracing` event fields and their enclosing span rather than formatted output;
- generated interceptors on static and runtime-configured dynamic paths;
- standalone erased mounts, startup validation, aliases, captures, complete
  miss/invalid-capture policy, request body/state transfer, streams, trailers,
  errors, and local futures/bodies;
- fixed-state generated static, configured-dynamic, and mixed integration with
  generated-first precedence and structural `EitherBody`;
- allocation and mounted-call counters proving a populated mount table costs a
  generated static request neither allocation nor erased-service invocation,
  plus exact mounted future/body allocation accounting;
- the Tower transport adapter over generated static, configured-dynamic,
  mixed, and erased-mount routing, including always-ready readiness under a
  readiness-bearing layer, cloned concurrent dispatch, transport-flavor
  `Send`/`Sync`/`'static` assertions, streams/trailers/body errors through the
  `Send` erasure, a Tower layer feeding a generated interceptor guard, and
  allocation counters showing zero adapter overhead and exactly one
  response-erasure allocation;
- renamed dependencies in generated code;
- no-default-feature builds of the core `routerama` crate;
- response-only builds and tests with no route or macro/matcher symbols;
- exact checks for none, `response`, `resolve`, `route`, `mount`, `query`,
  `json`, `form`, `tower`, `route+query`, and all features, including proof that
  mount-only adds route/response but not resolve/query/JSON/form, form-only
  adds route/query/response but not JSON, resolve, or mount, and tower-only adds
  route/response and `tower-service` but not resolve/query/JSON/form/mount;
- doctests and generated README content; and
- comparison benchmarks against the existing context-only path and equivalent
  Axum, Actix Web, Rocket, and Warp handlers.

## Performance constraints

- Path matching must remain unchanged and independently benchmarkable.
- Static captures must retain their current zero-copy behavior.
- Request-parts extraction should be monomorphized and sequential.
- No handler registry, artificial arity limit, boxed framework future, or
  mandatory type-map lookup should be introduced on the generated request
  path.
- Ordinary static captures must not invoke a regex engine.
- Response bodies should use a generated concrete sum type by default.
- Response body erasure, when explicitly requested for interoperability,
  should happen once after handler return.
- Merely configuring erased mounts must not allocate, box, or invoke a trait
  object on a request selected by generated static routing.
- Header maps and extensions should not be cloned unless an extractor
  explicitly requests ownership.
- Body buffering must be opt-in and bounded.

## Open decisions

1. **Resolved for the prototype:** the context argument is removed. Bare
   routers take generic `&S`; `#[router(state = AppState)]` takes
   `&AppState` and validates required extractor state at definition time.
   Handlers request explicit `State<T>` projections in either form.
2. **Partially resolved for the prototype:** a private generated concrete sum
   behind an opaque `Body<Data = Bytes, Error = impl Error>` return is coherent
   and transport bounds are structural. The 2026-07-25 and 2026-07-26
   measurements in `docs/PERF.md` now support keeping it: the concrete sum
   allocates nothing on the generated path, the explicit `BoxBody` opt-in costs
   19% more time and 15% more instructions on the generated streaming path, the
   Tower identity boundary costs 22 instructions with no allocation while
   `SendBoxBody` costs 119 more and exactly one allocation, and holding route
   count fixed while varying distinct body types from 1 to 16 grew loadable code
   by 240 bytes. What is still unmeasured is the *alternative* boundary: no
   fixture builds an exported named sum, so the comparison that would justify
   changing the representation has not been run. Until it is, this decision
   stays partially resolved rather than closed.
3. **Resolved for the prototype:** `route` always returns a response. A raw
   routing-error API has not demonstrated enough value to add.
4. **Resolved:** one generated-service `#[fallback]` receives the public
   borrowed `route::RouteFailure<'_>` value. The same type implements the
   deterministic default 404/400/415/406 mapping and also represents overlap
   predicate exhaustion.
5. **Resolved for the prototype:** `#[capture]` stays the dynamic-capture
   marker. A configured dynamic route has no compile-time template, so the
   marker is what lets the macro tell a capture from a request-parts
   extractor, name the generated `add_<handler>` argument checks, and reject a
   borrowed capture or a name the registered template does not contain. Those
   diagnostics are asserted in `routerama_build`'s expansion tests. Declaring
   captures elsewhere would move the same information further from the
   parameter it describes; revisit only if a second dynamic declaration form
   appears.
6. **Resolved for the prototype:** explicit raw, bounded bytes, and bounded
   text extraction belong to `route`; JSON is an additive feature that implies
   `route`. Bounded forms are an additive feature that implies `route` and
   `query`, lives at `route::form`, and accepts only output types satisfying
   the owned higher-ranked `FromQuery` contract. Cookies, multipart, and typed
   headers remain **deliberately unimplemented**: none is required by this
   plan, each would add a parsing surface and a dependency question of its
   own, and a handler can already read the raw header or body it needs. Phase
   6 therefore withdrew its cookie example rather than shipping a
   demonstration of machinery Routerama does not own.
7. **Resolved:** `FromRequestBody` and generated routing impose no `Send`
   bound, and the Tower adapter imposes none either: auto traits are inherited
   from the stored router, state, callable, and routing future. A transport's
   `Send`/`Sync`/`'static` requirements are paid only at the explicitly
   selected response boundary (`SendBoxBody` for `Send` transports, the local
   `BoxBody` otherwise).
8. **Resolved:** Tower integration is a separate `tower` feature in the same
   crate, exposed as `routerama::route::tower`. It implies `route`, adds only
   the `tower-service` trait crate, requires no code generation, and imposes
   a response body's `Send + 'static` requirements at an explicit boundary
   rather than on core routing; service-level auto traits remain structural.
9. **Partially resolved:** every bounded extractor states its limit as a const
   generic on the handler parameter, and an application can name an alias
   (`type SmallJson<T> = Json<T, 4_096>`) to keep one number in one place.
   There is deliberately no default limit and no ambient service policy,
   because either would let an unbounded buffer appear without a source
   change. Whether a service-level opt-in policy is worth adding on top of the
   alias is still open, and needs production feedback rather than another
   prototype.
10. **Resolved:** response construction is the independently selectable
    `routerama::response` module. Feature gating already removes macro and
    matcher dependencies, so a separate crate would add coordination without a
    smaller consumer graph.
11. **Resolved:** required state is a compile-time specialization, not a
    runtime sentinel. Parts extractors use a generated, eagerly called
    higher-ranked assertion; custom body extractors name one compile-only
    rejection/request-body witness through `BodyStateWitness`. Bare routers
    retain the previous generic contract and require no witness.
12. **Resolved for the prototype:** genuinely open runtime handlers use the
    separately gated `route::mount` API and explicit `ErasedMountService` /
    `ErasedMountRouter` names. Fixed-state generated services opt in with
    `#[router(state = S, erased_mounts)]` and call
    `route_with_erased_mounts`; they return structural
    `EitherBody<GeneratedBody, BoxBody>`, and give every generated static or
    configured-dynamic route precedence over mounts. Generic-state routers
    retain only their ordinary route entry.

## Recommended first milestone

Build a narrow `routerama::route` prototype with `route` enabled, plus `query`
for the query-extraction scenario, with:

- `http::Request<B>` input;
- direct static path captures;
- `Method`, `Uri`, `HeaderMap`, `Query<T>`, and `Extension<T>` extractors;
- one raw body extractor;
- `IntoResponse` for response, text, bytes, status, `Result`, and header
  tuples; and
- generated `route` methods returning a service-specific concrete response
  body enum.

Do not begin with middleware macros, multipart, WebSockets, cookies, or a large
catalog of convenience extractors. The first milestone should prove the
request ownership, body consumption, rejection, and response composition
model while preserving Routerama's current routing performance and `no_std`
core.

## Exhaustive performance-audit findings (2026-07-27)

The implemented milestone meets its measured non-I/O goals after correcting
fixture asymmetries: competitor numeric captures now use native typed
extraction, query schemas have equivalent ownership and measured regions, and
single-frame request bodies retain their original `Bytes`. Generated static
dispatch, typed captures, passive interceptors, exact Tower adaptation, and
borrowed query parsing retain allocation-free paths. Corrected throughput
measurements still place Routerama ahead of Axum for both CPU workloads in both
measurement methods.

The audit also narrows what the evidence proves. Routerama is a routing library,
not a transport, and no current result covers sockets, parsing, TLS, HTTP/2,
backpressure, scheduler migration, or connection lifecycle. Route-count
scaling covers one GET-only, literal-only, depth-three topology; affix fanout,
deep/disjoint tries, method fanout, many captures, and many unique extractor or
body types require separate fixtures. Generated code shows an instruction-cache
signal at 1,024 routes and a code-size step between 16 and 128 routes. Mixed
static/dynamic fallback rescans paths, deep paths spill scratch storage, Tower
clone costs are underrepresented, and realistic fragmented/pending/trailer
bodies remain open measurements.

Accordingly, adoption claims must remain workload-specific: Routerama has the
lowest deterministic instruction count in every equivalent routing,
bounded-body, and form row currently measured, and leads the concurrent
in-process CPU fixture, but the plan does not claim "fastest web server."
Future performance work should prioritize isolated route-shape scaling,
instruction-cache counters, realistic body/backpressure workloads, and a
transport integration owned and measured separately from the core router.
