# g5-network findings

Scope: `crates/fetch`, `crates/fetch_hyper`, `crates/fetch_tls`, `crates/fetch_options`,
`crates/fetch_azure`, `crates/fetch_winhttp`, `crates/rest_over_grpc`,
`crates/rest_over_grpc_tests`, `crates/rest_over_grpc_examples`.

Method note: the container has no egress to `index.crates.io` / `static.crates.io` and no
cargo registry cache, so `cargo build`, `cargo bench`, `cargo clippy` and every `just`
recipe fail at dependency resolution (`cargo metadata --offline` exits 101;
`cargo build -p fetch --offline` reports `no matching package named 'tokio' found`). All
findings are therefore labelled `inferred from code reading`, and each names the specific
benchmark that would confirm it. The exceptions are the type-layout claims, which were
verified by compiling a throwaway dependency-free replica with plain `rustc` and printing
`size_of`/`align_of` (the temporary program was deleted afterwards and never added to the
repo).

Cross-group context used below: `[profile.bench]` sets `lto = "fat"` and
`codegen-units = 1` while `[profile.release]` sets neither, so repo benchmark numbers come
from a build no consumer gets and are structurally blind to missing `#[inline]`;
benchmarks are also built with `--all-features`.

Inline census across the six implementation crates in scope: **244 public functions, zero
`#[inline]` attributes** (fetch 62/0, fetch_hyper 17/0, fetch_tls 21/0, fetch_options 25/0,
fetch_azure 1/0, rest_over_grpc 118/0).

## Crate: fetch

### Summary

`fetch` is the front door of the HTTP stack: it owns the client, the builder, the
layered pipeline assembly and the standard handler set (logging, metrics, retry,
dispatch, transport). Consequently almost everything here is genuinely on the
per-request hot path, and it is where the highest-value findings in this group live.

Three themes dominate. First, **unconditional work for conditional output**: the logging
handler redacts the URL on every request even though the result is normally discarded by
the subscriber filter, and the client wraps every request in a timeout future even when
no timeout was requested. Second, **type erasure and shared locking**: the default Tokio
client erases the pipeline twice into `layered::DynamicService`, each layer of which takes
a `Mutex<Pool>` per request, and the default `Isolation::Shared` means all worker threads
contend on the same two mutexes. Third, **allocation in telemetry**: the standard pipeline
instantiates two `Metrics` layers, each of which allocates a `String` for the host per
request.

The crate also has zero `#[inline]` across 62 public functions, which matters more than
usual because `[profile.release]` enables no LTO — consumers get no cross-crate inlining
at all.

There is genuinely good work here too, called out in "Considered and ruled out": the
dispatch handler deliberately selects the transport *before* entering the async block to
keep the request future small, with a comment explaining why. That is exactly the style
`docs/performance.md` asks for.

### Findings

#### F1. `Logging::execute` redacts the URL on every request before the await

- **Location:** `crates/fetch/src/handlers/logging.rs:102-107` (the eager call is line
  107), consumed at `:122` and `:139`; helper `redacted_path_and_query` at `:151-159`.
- **Issue:** `execute` computes
  `let redacted_path_and_query = redacted_path_and_query(&input, &self.redaction_engine);`
  *before* the `async move` block that starts on line 109. The helper runs the redaction
  engine over the request's path and query and materialises the result into a fresh
  `String` (`to_redacted_string`). The value is consumed in exactly two places: a DEBUG
  event `http.response.complete` (line 122) and a WARN event `http.response.error`
  (line 139). In a normal production configuration the DEBUG event is filtered out by the
  subscriber and the WARN event only fires on failure, so on the overwhelmingly common
  path — a successful request with DEBUG disabled — the allocation and the whole redaction
  pass are pure waste. This is the clearest violation in the group of the
  `docs/performance.md` "no allocation on the hot path" rule: it is one allocation plus a
  full scan of the URL per request, on by default, for output nobody reads.
- **Impact:** High — every request through the standard pipeline pays it; it is on by
  default; and the cost scales with URL length, so query-heavy APIs pay most.
- **Remediation:** Gate the computation on whether anything will consume it. The naive
  `tracing::enabled!(Level::DEBUG)` gate is *not* sufficient here for two reasons: the
  value is also needed on the WARN error path, and `input` is moved into the async block
  so the redaction cannot simply be deferred to the same binding. The surgical shape is to
  capture the cheap ingredients before the move (the templated `PathAndQuery`, or a clone
  of the URL — clone the `Uri`, which is refcounted, not the redacted `String`) and call
  `redacted_path_and_query` lazily inside each match arm that actually emits an event,
  with the DEBUG arm additionally guarded by `tracing::enabled!`. That preserves both
  events byte-for-byte and removes all work from the success path.
- **Evidence:** inferred from code reading. Confirmable with a Criterion benchmark
  `crates/fetch/benches/handlers.rs`, group `handlers/logging`, comparing a request driven
  through `Logging::execute` with a no-op subscriber against one with DEBUG enabled, and
  with `alloc_tracker` asserting zero allocations in the DEBUG-disabled case (the crate
  already depends on `alloc_tracker` in its existing benches, so this is cheap to add).
  A Callgrind pair `handlers_cg.rs` would show the instruction delta directly.

#### F2. Default Tokio client erases the pipeline twice and shares one mutex across all threads

- **Location:** `crates/fetch/src/handlers/transport.rs:15,19`;
  `crates/fetch/src/pipeline/builder.rs:124,146`; `crates/fetch/src/tokio.rs:77`;
  `crates/fetch/src/client_builder.rs:445-447`; `crates/fetch/src/client.rs:373-377`;
  cross-crate evidence in `crates/layered/src/dynamic.rs:70-92`.
- **Issue:** There are **two** independent `layered::DynamicService` type erasures on the
  per-request path. The inner one is `TransportHandler`, which is declared as a newtype
  around `DynamicService<...>` (`transport.rs:15`, constructed at `:19`). The outer one
  erases the entire assembled pipeline (`pipeline/builder.rs:124` and `:146`). Each
  `DynamicService` owns a `Mutex<Pool>` that is locked on every `execute` to rent a boxed
  future slot, and additionally performs an `Arc::clone(&service)` (`dynamic.rs:78`) — so
  the steady-state per-request cost of the erasure alone is two mutex lock/unlock pairs
  and two atomic read-modify-writes, before any actual HTTP work.

  The design comment at `dynamic.rs:70-72` explicitly justifies the mutex by appealing to
  "the thread-isolated common case", where the mutex is uncontended and therefore nearly
  free. That assumption does not hold for the default client: `tokio.rs:77` selects
  `Isolation::Shared`, and `client_builder.rs:445-447` turns that into
  `HttpClientPipeline::Shared(Arc::new(...))` — a single pipeline instance shared by every
  Tokio worker thread. So on a multi-core runtime under load, every worker thread contends
  on the same two mutexes for every request. The consumer-side scenario
  `docs/performance.md` asks for is concrete and mainstream: a service built on the
  default `fetch` Tokio client, running on an N-core machine, making concurrent outbound
  calls — the standard shape of an API gateway or a service-to-service caller. As core
  count rises, these two locks become a serialisation point in front of work that is
  otherwise fully parallel.
- **Impact:** High — it is a contention point, not a constant cost, so its severity grows
  with core count and concurrency, exactly where an HTTP client is expected to scale. It is
  also on the default configuration, so no user opts into it.
- **Remediation:** Two independent, separately shippable steps. (a) Surgical: the
  `Isolated` variant already exists and routes through `thread_aware::Arc::new_with`
  (per-core instances), so re-examine whether `Isolation::Shared` must be the Tokio default
  — switching the default, or auto-selecting `Isolated` on multi-threaded runtimes, removes
  the contention without touching `layered`. (b) Structural, and therefore requiring the
  scenario above as justification: remove one of the two erasure layers. `TransportHandler`
  erasing separately from the outer pipeline erasure looks redundant — if the outer
  pipeline is already dynamic, the inner one buys nothing but a second mutex. This overlaps
  with the sibling group's finding against `crates/layered/src/dynamic.rs:73-87`; the
  pool-free redesign belongs there, the double-erasure and the `Shared` default belong here.
- **Evidence:** inferred from code reading, corroborated by the sibling group's independent
  discovery of the same mutex from the `layered` side. Confirmable with a Criterion
  benchmark `crates/fetch/benches/pipelines.rs` extended with a **multi-threaded**
  group — the existing benches all run under `block_on`, so they are structurally incapable
  of showing contention (see "Benchmark coverage" below). Drive N concurrent requests from
  N Tokio worker threads through a no-op transport and sweep N over 1, 2, 4, 8, 16; a
  contention-free implementation is flat in per-request latency, the current one is not.

#### F3. Two `Metrics` layers each allocate a `String` for the host per request

- **Location:** `crates/fetch/src/handlers/metrics.rs:312` (and `fill_error_attributes` at
  `:335`); `crates/fetch/src/pipeline/standard.rs:95,111`.
- **Issue:** `metrics.rs:312` does `val.host().to_string()` to populate the
  `server.address` telemetry attribute. `Uri::host` returns a `&str` borrowed from the
  refcounted `Uri`, so this is a fresh heap allocation and copy per request purely to
  satisfy the attribute type. Worse, `pipeline/standard.rs` installs **two** `Metrics`
  layers — `total_metrics` (line 95) and `attempt_metrics` (line 111) — so the standard
  pipeline pays this twice per request (and once more per retry attempt, since the attempt
  layer sits inside the retry loop).
- **Impact:** Medium — two to N allocations per request, unconditional, on the default
  pipeline. Not as severe as F1 because the value is genuinely consumed (metrics are not
  filtered out the way DEBUG spans are), but the allocation itself is avoidable.
- **Remediation:** Attribute values in the OpenTelemetry-style `KeyValue` model can be
  backed by a `Cow<'static, str>` or an `Arc<str>`. Cache the host string once per
  `Metrics` layer instance keyed on the destination, or hoist it into the connection/route
  metadata that is already per-destination rather than per-request. If the attribute type
  cannot avoid owning, at minimum share one computed value between the two layers instead
  of computing it independently in each.
- **Evidence:** inferred from code reading. Confirmable with
  `crates/fetch/benches/handlers.rs`, group `handlers/metrics`, using `alloc_tracker` to
  count allocations per request through the standard pipeline with and without the change.

#### F4. Every request is wrapped in a timeout future even when no timeout was requested

- **Location:** `crates/fetch/src/client.rs:345-370` (the wrap is line 363; the default is
  established at `:346-349`); `crates/tick/src/future_ext.rs:33-38`;
  `crates/tick/src/delay.rs:72-78` and `:99-105`; `crates/tick/src/clock.rs:176-180`.
- **Issue:** `Client::execute` reads the `ResponseTimeout` extension and falls back to
  `Duration::MAX` when it is absent (lines 346-349) — i.e. "no timeout". It then calls
  `.timeout(&self.clock, timeout)` on line 363 unconditionally. `tick::FutureExt::timeout`
  constructs a `Delay` (`delay.rs:72-78`), and `Delay::new` performs `clock.clone()`
  unconditionally — an atomic refcount increment (and a matching decrement on drop) on a
  `Clock` whose state is `{ state: ClockState, time: SimpleClock, affinity: Option<Affinity> }`.

  An important correction to the obvious over-claim: `Delay::poll` (`delay.rs:99-105`)
  short-circuits with `None if duration == Duration::MAX => Poll::Pending` **first**, so
  `Instant::now()` is *not* called and no timer is registered. The real residual cost is
  therefore modest but genuine and unconditional: one `Clock` clone plus drop (two atomic
  RMWs) per request, `size_of::<Delay>()` added to the size of the request future (which
  is itself moved around and, in the erased path of F2, boxed), and one extra branch per
  poll of the composed future. Note also that the standard pipeline already installs its
  own timeout layer, so for the default configuration this is a *second*, redundant timeout
  wrapper.
- **Impact:** Medium — small per-request constant, but unconditional, on the default path,
  and it inflates the future that F2's boxing then has to allocate for.
- **Remediation:** Branch on `timeout == Duration::MAX` and select between the wrapped and
  unwrapped future with `futures::future::Either` (or a small hand-rolled two-variant
  enum future). This is a few lines, preserves behaviour exactly, and keeps the defensive
  timeout intact wherever one was actually configured — squarely within the "surgical over
  architectural" guidance.
- **Evidence:** inferred from code reading. Confirmable with a Callgrind benchmark
  `crates/fetch/benches/client_cg.rs` paired with a Criterion `client.rs`, comparing
  `execute` with and without a `ResponseTimeout` extension; the instruction delta on the
  no-timeout case is the whole finding. A `size_of` assertion on the request future would
  show the future-size component.

#### F5. Telemetry attribute SmallVec inflates the pipeline future

- **Location:** `crates/fetch/src/telemetry.rs:44`;
  `crates/fetch/src/handlers/metrics.rs:340-385`.
- **Issue:** `TelemetryAttributes` is `smallvec::SmallVec<[KeyValue; 9]>`. `KeyValue` is
  roughly 56–64 bytes (a `Key` plus a `Value` enum with a string variant), so nine inline
  slots is on the order of 600 bytes carried inline. `MetricsDropGuard` holds this across
  the `.await` (lines 340-385), which means it is part of the generated future's state, and
  since the standard pipeline has two `Metrics` layers (F3) the pipeline future carries
  roughly 1.2 KB of inline attribute storage. Large futures are not free: they are moved on
  every combinator boundary and, in the erased path, memcpy'd into a boxed slot.
- **Impact:** Low — this is a size/copy cost rather than an allocation, and the SmallVec
  choice deliberately trades size for avoided allocations, which is defensible. Recorded
  for completeness and because it compounds with F2 and F4.
- **Remediation:** Consider whether nine inline slots is the right inflection point; if the
  common case is four or five attributes, `[KeyValue; 4]` halves the inline footprint at
  the cost of an allocation only in the rarer wide case. Measure before changing — the
  current choice may well be correct.
- **Evidence:** inferred from code reading; the `KeyValue` size is **estimated**, not
  verified (it lives in a third-party crate that could not be fetched in this container).
  Confirmable with a `size_of` assertion test plus a Criterion benchmark that measures the
  cost of moving the composed pipeline future.

#### F6. Zero `#[inline]` across 62 public functions

- **Location:** crate-wide; census of `crates/fetch/src`.
- **Issue:** `fetch` exports 62 public functions and carries **zero** `#[inline]`
  attributes. Many are small accessors and wrappers on the per-request path. Because
  `[profile.release]` sets neither `lto` nor `codegen-units = 1`, a downstream consumer
  compiling in release mode gets **no cross-crate inlining of non-generic functions at
  all** — every one of these is a real call through the crate boundary. `docs/performance.md`
  asks for `#[inline]` precisely in this situation, while also asking us to be judicious
  and not blanket-annotate.
- **Impact:** Medium — diffuse rather than concentrated, but it applies to every consumer
  of the crate in the configuration consumers actually ship.
- **Remediation:** Annotate the small, hot, non-generic public functions: the accessors on
  `Client`, the handler constructors, and anything that is a one-line delegation. Do not
  blanket-annotate; generic functions are already available for inlining and large
  functions should not be.
- **Evidence:** inferred from code reading (attribute census). Confirmable with a Callgrind
  benchmark built under `[profile.release]` settings rather than `[profile.bench]` — note
  that under the current `[profile.bench]` (fat LTO, 1 CGU) this finding is **invisible**,
  which is itself the point.

#### F7. TLS feature flags: enabling both backends silently links a dead one

- **Location:** `crates/fetch/Cargo.toml:54-79`; `crates/fetch/src/tokio.rs:135-165`.
- **Issue:** `default = []`, `tls = ["rustls"]`, and the `tokio` feature force-enables
  `rustls?/aws-lc-rs`. In `build_tls_backend`, the rustls branch is
  `#[cfg(any(feature = "rustls", test))]` and the native-tls branch is
  `#[cfg(all(feature = "native-tls", not(any(feature = "rustls", test))))]`. Because Cargo
  features are additive and unify across a dependency graph, any workspace where one crate
  wants `native-tls` and another wants `rustls` ends up with both enabled — at which point
  **rustls silently wins and the entire native-tls stack (including OpenSSL or the platform
  TLS library) is compiled and linked but never executed**. That is binary size, build time
  and a wholly unnecessary transitive dependency surface, with no diagnostic. It also means
  the benchmark configuration, which builds `--all-features`, links both TLS stacks into
  every benchmark binary.
- **Impact:** Medium — build-time and binary-size cost rather than runtime, but it is
  silent, and TLS backend selection is exactly the kind of feature choice that should be
  loud.
- **Remediation:** Emit a `compile_error!` (or at minimum a `#[deprecated]`-style build
  warning) when both `rustls` and `native-tls` are enabled, stating which one wins. That is
  the ecosystem-standard way to handle mutually exclusive backend features and costs
  nothing at runtime. Separately, consider whether benchmarks should build
  `--all-features`, given that no consumer does.
- **Evidence:** inferred from code reading of the `cfg` predicates and the feature table.
  Confirmable by `cargo tree -e features` with both features enabled (blocked in this
  container) or by inspecting binary size with each combination.

#### F8. Router clone inserted into extensions per request when alternatives exist

- **Location:** `crates/fetch/src/client.rs:354-356`.
- **Issue:** When `has_alternatives()` is true, `execute` does
  `input.extensions_mut().insert(self.router.clone())`. `http::Extensions::insert` boxes
  the inserted value (it is a `HashMap<TypeId, Box<dyn Any + Send + Sync>>`), so this is a
  router clone plus a heap allocation plus a hash-map insert per request, on the
  multi-endpoint path.
- **Impact:** Low — only on clients configured with alternative endpoints, and the clone is
  plausibly refcounted. Recorded because it is unconditional within that configuration.
- **Remediation:** If the router is already `Arc`-backed the clone is cheap and only the
  extensions boxing remains; consider passing the router through the handler's own state
  (it is constructed per client, not per request) rather than through the dynamic
  extensions map, which exists for caller-supplied data.
- **Evidence:** inferred from code reading. Confirmable with `alloc_tracker` in
  `crates/fetch/benches/pipelines.rs` comparing a single-endpoint client against a
  multi-endpoint one.

#### F9. Convenience API re-parses the URI on every call

- **Location:** `crates/fetch/src/client.rs:98-104` (and the same pattern at `:135`,
  `:168`, `:195`, `:223`, `:256`, `:287`); documented at `:68-71` and `:110-113`.
- **Issue:** The request-construction methods take `impl TryInto<Uri>` and
  `impl TryInto<Method>`. The ergonomic call `client.get("https://example.com/v1/items")`
  therefore re-parses and re-validates the URI on **every** call, including in a tight
  request loop against a fixed endpoint. The crate is aware of this — there are
  "Performance tip" doc sections at lines 68-71 and 110-113 telling callers to pre-build a
  `Uri` — but the shape of the API means the slow path is the convenient one and the fast
  path is the one you have to know about.
- **Impact:** Low — per-request URI parsing is real but modest, and the escape hatch exists
  and is documented. Flagged under the brief's "public API forcing callers into slow paths"
  heading.
- **Remediation:** No code change proposed; this is a deliberate ergonomics/performance
  trade-off that the crate documents. If it is ever revisited, a `Uri`-taking method with
  the short name and a `&str`-taking one with a longer name would invert the default.
- **Evidence:** inferred from code reading. Confirmable with a Criterion benchmark
  `crates/fetch/benches/client.rs`, group `client/request_construction`, comparing `&str`
  and pre-parsed `Uri` inputs.

#### F10. Pooled dispatch: boxed selector closure plus a globally shared atomic and two divisions per request

- **Location:** `crates/fetch/src/handlers/dispatch.rs:65,71,92,116-130`;
  `crates/fetch_options/src/pooling.rs:407,425-447`.
- **Issue:** In `DispatchMode::Pooled`, the pool selector is a `Box<dyn Fn>`
  (`PoolSelector`, `dispatch.rs:65,71`) invoked through dynamic dispatch on every request
  (`:92`, `:116-130`). The selector calls `PoolSelectionStrategy::select`
  (`pooling.rs:425-447`), which performs a `fetch_add` on a **globally shared** `AtomicU32`
  counter (`pooling.rs:407`) for every request. That is a cache-line ping-pong across all
  worker threads — and it is ironic, because the entire point of having multiple pools is
  to *reduce* cross-thread contention, yet the mechanism that distributes requests across
  them reintroduces a single shared cache line. In `Saturating` mode the same function also
  performs **two runtime integer divisions** (`counter / requests_per_client`, then
  `% clients.len()`) on values not known at compile time, so neither is strength-reduced.
- **Impact:** Medium — dynamic dispatch plus a contended atomic plus two divisions, per
  request, in the multi-pool configuration that exists specifically for high-throughput
  workloads.
- **Remediation:** Replace the shared counter with a per-thread counter (the workspace
  already has `thread_aware` for exactly this) — round-robin does not require global
  ordering, only reasonable distribution. Replace the modulo with a mask when
  `clients.len()` is a power of two, or cache a reciprocal. Consider making the selector a
  small enum rather than `Box<dyn Fn>` so the common strategies dispatch statically.
- **Evidence:** inferred from code reading. Confirmable with a multi-threaded Criterion
  benchmark `crates/fetch/benches/dispatch.rs` sweeping thread count against a no-op
  transport in `Pooled` mode; a shared-counter implementation degrades with thread count
  while a per-thread one is flat.

### Benchmark coverage

`fetch` has two Criterion benchmark files, `benches/http_crate.rs` and
`benches/pipelines.rs`, both of which use `alloc_tracker` — good, and unusual for this
workspace. There is **no Callgrind coverage at all** for the crate; `docs/naming.md`
permits Criterion without Callgrind, so this is a gap rather than a violation, but the
per-request pipeline is exactly the kind of hot path `docs/callgrind-benchmarks.md`
describes as warranting instruction-count coverage.

Two concrete problems:

1. **Naming convention violation.** `benches/pipelines.rs:39` uses
   `c.benchmark_group("http_client_pipelines")`. `docs/naming.md` requires the group name
   to be prefixed by the file basename, i.e. `pipelines/...`. `benches/http_crate.rs:25`
   uses `"http_crate"` and is correct. This matters beyond tidiness: it is what keeps
   Criterion and Callgrind identifiers aligned when a paired `_cg.rs` file is later added.
2. **The existing benches cannot see the most important finding.** Every benchmark in the
   crate runs under `block_on`, i.e. single-threaded. F2's shared-mutex contention and
   F10's shared-atomic ping-pong are *by construction* invisible to a single-threaded
   harness — they are zero-cost when uncontended, which is precisely the case the benches
   measure. The suite therefore gives a clean bill of health to the crate's most serious
   scaling problem.

Missing, in priority order: a multi-threaded pipeline benchmark (F2, F10); a
`benches/handlers.rs` covering logging and metrics with `alloc_tracker` (F1, F3); a
`benches/client.rs` covering request construction and the timeout wrap (F4, F9); and
Callgrind pairs for whichever of these prove hot.

### Considered and ruled out

- **`dispatch.rs:109-131` — transport selected before the async block.** This is not a
  problem, it is exemplary, and worth recording as a positive. The comment at lines 109-113
  explains that the transport is chosen eagerly specifically so that the selection state
  does not become part of the request future, keeping the future small. This is the pattern
  the rest of the crate should follow (and which F1 and F4 violate).
- **`DispatchMode::Single` (`dispatch.rs:115`)** correctly bypasses the selector, the
  atomic and the divisions entirely. No finding.
- **Builder-time and client-construction allocations** throughout `client_builder.rs`.
  `docs/performance.md` explicitly deprioritises first-insert and setup costs; a client is
  built once and used for the lifetime of the process.
- **Retry handler state.** Examined; the retry bookkeeping is proportional to attempts
  actually made and does not allocate on the zero-retry success path.
- **`Extensions` lookups for `ResponseTimeout`.** A `TypeId`-keyed hash lookup per request
  is real but is the ecosystem-standard `http` crate mechanism; `docs/performance.md` asks
  us to justify deviations from ecosystem patterns, and there is no justification here.

## Crate: fetch_hyper

### Summary

`fetch_hyper` is the hyper-backed transport. The good news dominates: the actual
per-request path, `HyperHandler::execute`, is close to ideal — allocation-free, unboxed,
combinator-based, with a small future. The findings here are all on the **per-connection**
path (connect, TLS handshake, connection telemetry), which `docs/performance.md`
explicitly deprioritises relative to per-request work. They are recorded for completeness
and each is flagged accordingly; one carries an explicit philosophy note.

The one finding with genuine per-request reach is the absence of `#[inline]` across the
crate's 17 public functions, for the same release-profile reason as F6.

### Findings

#### F11. Two full clones of the connect input per connection, plus histogram and clock clones

- **Location:** `crates/fetch_hyper/src/connection/client_connector.rs:106-107` (the two
  `input.clone()` calls), `:110` and `:121` (`Histogram::clone()`), `:131`
  (`clock.clone()`).
- **Issue:** `client_connector` clones the connect input twice in adjacent statements
  (lines 106-107), then clones the histogram handle twice more (110, 121) and the clock
  once (131). Whether the input clone is cheap depends on whether the contained
  `templated_uri::BaseUri` is refcounted or owns its string data — this could not be
  confirmed in this container, so the severity is hedged accordingly. If it owns, each
  clone is a string allocation per connection.
- **Impact:** Low — per-connection, not per-request, and connection establishment is
  already dominated by DNS, TCP and TLS handshake costs that dwarf a handful of clones.
- **Remediation:** Restructure to clone once and share, or borrow where the consumer's
  lifetime permits. Low priority.
- **Evidence:** inferred from code reading. Confirmable with `alloc_tracker` around a
  connect against a loopback listener.
- **Philosophy note:** **Conflicting.** `docs/performance.md` explicitly deprioritises
  first-insert and setup costs, and connection establishment is setup. I am reporting this
  because the brief asks for exhaustive coverage of clones and copies, but I would not
  recommend acting on it ahead of any per-request finding in this document, and a reviewer
  would be within the house philosophy to close it as won't-fix.

#### F12. Connection telemetry allocates a heap Vec and two Strings per connect

- **Location:** `crates/fetch_hyper/src/telemetry.rs:23-28` and `:35-40`.
- **Issue:** Both attribute-building functions construct a heap `Vec<KeyValue>` (not a
  `SmallVec`, unlike `fetch`'s `TelemetryAttributes`) and populate it with
  `host().to_string()` and `scheme().to_string()`. The scheme in particular is almost
  always one of two `&'static str` values, so allocating for it is avoidable in essentially
  every case.
- **Impact:** Low — per-connection. Recorded because the scheme allocation is trivially
  avoidable and because it is inconsistent with `fetch`'s use of a `SmallVec` for the same
  job.
- **Remediation:** Use `SmallVec` for consistency with `fetch::TelemetryAttributes`, and
  map the scheme to a `&'static str` (`"https"` / `"http"`) rather than allocating.
- **Evidence:** inferred from code reading. Confirmable with `alloc_tracker` around a
  connect.

#### F13. Zero `#[inline]` across 17 public functions

- **Location:** crate-wide; census of `crates/fetch_hyper/src`.
- **Issue:** Same structural issue as F6: no `#[inline]` anywhere, and `[profile.release]`
  provides no LTO, so consumers get no cross-crate inlining. The transport handler's small
  wrappers sit directly on the per-request path.
- **Impact:** Medium — this is the one finding in this crate that touches per-request cost.
- **Remediation:** Annotate the small delegating functions on the transport path; be
  judicious, as `docs/performance.md` asks.
- **Evidence:** inferred from code reading (attribute census). Confirmable only under
  release-profile codegen settings, not under the current `[profile.bench]`.

#### F14. TLS connector cloned wholesale and boxed per connection

- **Location:** `crates/fetch_hyper/src/tls/connector.rs:168`, `:177` (`let mut c = c.clone();`),
  `:171` and `:180` (`Box::pin(s) as Pin<Box<dyn HyperIo>>`).
- **Issue:** Each connection attempt clones the entire `HttpsConnector` and then boxes and
  type-erases the resulting stream into `Pin<Box<dyn HyperIo>>`. The clone is required by
  the connector API's ownership model; the boxing is required to unify the TLS and plaintext
  stream types behind one return type. Both are one-per-connection.
- **Impact:** Low — per-connection, and the erasure cost is then amortised over every byte
  the connection carries. Note however that the `dyn HyperIo` indirection persists for the
  life of the connection, so every read and write goes through a vtable — that part is
  per-I/O, not per-connection.
- **Remediation:** The stream erasure could be replaced by a two-variant enum implementing
  `HyperIo` (plaintext vs TLS), which removes both the box and the vtable dispatch on every
  read/write for the life of the connection. That is a contained, surgical change to a
  private type.
- **Evidence:** inferred from code reading. Confirmable with a throughput benchmark over a
  loopback connection comparing the boxed and enum forms — the difference should show on
  large-body transfers where read/write call counts are high.

### Benchmark coverage

**Zero benchmarks.** No `benches/` directory, no Criterion, no Callgrind. This is the
crate that performs the actual HTTP transport work, so the absence is notable. The most
valuable additions would be a loopback-server throughput benchmark (to catch F14's
per-I/O vtable cost) and a connect-path benchmark with `alloc_tracker` (F11, F12).

### Considered and ruled out

- **`HyperHandler::execute` (`crates/fetch_hyper/src/connection/hyper_handler.rs:70-93`)
  — exemplary, no finding.** This is the crate's per-request hot path and it is written
  the way the house philosophy asks: no allocation, no boxing, a small future, and body
  adaptation done with `map_frame`/`map_err` combinators rather than by collecting or
  re-wrapping. I searched hard for a problem here and there isn't one. Worth holding up as
  the reference implementation for the rest of the group.
- **Body streaming adaptation.** Zero-copy throughout; frames pass through without being
  collected.
- **Connection pool interaction.** Delegated to hyper's own pool, which is the ecosystem
  standard; no deviation to justify.

## Crate: fetch_tls

### Summary

`fetch_tls` is a configuration crate: it builds TLS backends, maps ALPN protocols and
loads client identities. Essentially all of its work happens once per client, not per
request, so `docs/performance.md`'s deprioritisation of setup costs applies to nearly
everything in it. The only finding with per-request reach is the `#[inline]` gap on its
accessors, and even that is marginal.

Notably, this crate contains one of the better-reasoned pieces of in-tree performance
documentation in the group — see "Considered and ruled out".

### Findings

#### F15. Zero `#[inline]` across 21 public functions

- **Location:** crate-wide; census of `crates/fetch_tls/src`. Representative:
  `crates/fetch_tls/src/alpn.rs:12-24` (`map_to_alpn`).
- **Issue:** No `#[inline]` anywhere. `map_to_alpn` is a tiny match called per client, so
  annotating it has low value; the accessors on the backend and identity types are the ones
  that could plausibly appear in a per-connection path and benefit.
- **Impact:** Low — most of this crate's surface is genuinely per-client.
- **Remediation:** Annotate only the small accessors that are reachable per connection.
  Explicitly *do not* blanket-annotate this crate; it would be noise.
- **Evidence:** inferred from code reading (attribute census).

#### F16. `write_pem_block` base64-encodes into a temporary String then copies into the output buffer

- **Location:** `crates/fetch_tls/src/client_identity.rs:138-143`.
- **Issue:** The function allocates a fresh `String` for the base64 encoding of each PEM
  block and then copies the bytes into the caller-supplied `out: &mut Vec<u8>`. The
  intermediate `String` is pure overhead — base64 encoders in the ecosystem support
  encoding directly into a `&mut Vec<u8>` or a slice.
- **Impact:** Low — this runs once at client-identity load time, i.e. once per process in
  the normal case. Recorded only because the brief asks for exhaustive coverage of avoidable
  copies.
- **Remediation:** Encode directly into `out`. One-line change if the base64 crate in use
  exposes a slice/`Vec` encoding entry point.
- **Evidence:** inferred from code reading.
- **Philosophy note:** **Conflicting.** This is squarely a setup cost, which
  `docs/performance.md` tells us to deprioritise. Reported for completeness; not
  recommended for action.

### Benchmark coverage

**Zero benchmarks.** Given that the crate is configuration-only and its work is per-client,
this is defensible and I would not recommend adding Criterion coverage here ahead of any
other crate in the group. If anything is worth measuring it is certificate-chain parsing
time for large chains, and even that is a startup metric rather than a throughput one.

### Considered and ruled out

- **`TlsBackend` large enum variant (`crates/fetch_tls/src/backend.rs:19-34`).** This looks
  at first glance like a classic large-enum-variant finding, and `clippy::large_enum_variant`
  is indeed allowed on it — but with a documented justification stating that it is a
  configuration object and that boxing "would clutter the public API without performance
  benefit". That reasoning is correct: the enum is constructed once per client and never
  moved on a hot path, so the usual cost of a large variant (memcpy on every move) does not
  apply. **No finding.** This is a good example of an in-tree lint allowance carrying its
  own rationale, exactly as the house philosophy asks.
- **No crypto provider feature enabled by default (`crates/fetch_tls/Cargo.toml:38-41,51-56`).**
  Deliberate: the caller supplies the provider through `TlsBackendBuilder::configure_rustls`.
  This avoids forcing `aws-lc-rs` or `ring` on consumers who have already chosen one, and it
  is documented. **Positive note, no finding.**
- **ALPN protocol vector construction.** Per client, tiny, and the allocation is genuinely
  needed by the rustls API.

## Crate: fetch_options

### Summary

`fetch_options` is a pure configuration crate (about 1,470 lines of `src`) holding the
option types for pooling, timeouts and connection metadata. It performs no per-request work
of its own. Its one performance-relevant contribution is that its *accessors* are called
per connection and per request from `fetch` and `fetch_hyper` across a crate boundary, and
none of them are `#[inline]`. The pool-selection algorithm also lives here, though the
per-request cost it imposes is attributed to `fetch` under F10 and cross-referenced below.

### Findings

#### F17. Trivial `Copy` accessors on the per-request path are not `#[inline]`

- **Location:** `crates/fetch_options/src/connection_info.rs:71` (`age`), `:77`
  (`pool_index`), `:83` (`is_poisoned`), `:101` (`max_age`), `:111` (`is_expired`);
  `crates/fetch_options/src/pooling.rs:278` (`resolve`), `:325` (`index`). Census: 25
  public functions, zero `#[inline]`.
- **Issue:** These are one-line accessors returning `Copy` values, called from `fetch` and
  `fetch_hyper` per connection and, for the pooling ones, per request. Because they are
  non-generic and `[profile.release]` enables no LTO, each is a real function call across
  the crate boundary in every build a consumer actually ships — a call, a return, and a lost
  optimisation opportunity, for a function whose body is a field read. This is the textbook
  case `docs/performance.md` gives for `#[inline]`, and it is also the least controversial
  place to apply it, since these functions are trivially small and cannot cause code bloat.
- **Impact:** Medium — individually tiny, but `is_expired` and `is_poisoned` are checked on
  every connection reuse and `resolve`/`index` on every pooled request.
- **Remediation:** Add `#[inline]` to the listed accessors. This is the highest
  confidence-to-risk `#[inline]` change in the whole group.
- **Evidence:** inferred from code reading (attribute census). Confirmable with a Callgrind
  benchmark built under release-profile codegen settings; invisible under the current
  fat-LTO `[profile.bench]`.

#### F18. Pool selection performs a shared atomic RMW and two runtime divisions per request

- **Location:** `crates/fetch_options/src/pooling.rs:407` (the shared `AtomicU32`),
  `:425-447` (`PoolSelectionStrategy::select`).
- **Issue:** See F10 for the full analysis — this is the same defect viewed from the crate
  that owns the code rather than the crate that calls it. The counter at line 407 is shared
  by all threads, so `fetch_add` bounces a cache line across cores on every request; and the
  `Saturating` arm performs `counter / requests_per_client` followed by `% clients.len()`,
  two divisions on values unknown at compile time.
- **Impact:** Medium — cross-referenced with F10; counted once for prioritisation purposes.
- **Remediation:** Per-thread counters via the workspace's `thread_aware`; mask instead of
  modulo for power-of-two pool counts, or a cached reciprocal.
- **Evidence:** inferred from code reading. Confirmable with the multi-threaded dispatch
  benchmark described under F10.

### Benchmark coverage

**Zero benchmarks.** Mostly appropriate for a configuration crate, with one exception:
`PoolSelectionStrategy::select` is a genuine per-request algorithm and deserves its own
microbenchmark — a Criterion `benches/pooling.rs` with a matching Callgrind
`benches/pooling_cg.rs`, both sweeping thread count, would make F17 and F18 measurable and
would be a small, self-contained addition.

### Considered and ruled out

- **Option struct sizes.** Several option structs are large, but they are constructed once
  per client and stored behind a shared reference; size is irrelevant here.
- **`Duration` arithmetic in `is_expired`/`age`.** Cheap integer work; the only cost is the
  un-inlined call, covered by F17.
- **Builder-style setters.** Setup only.

## Crate: fetch_azure

### Summary

`fetch_azure` adapts `fetch` to the `azure_core::HttpClient` trait. It is a small crate,
but it is a genuine per-request adapter and it contains the densest concentration of
avoidable per-response allocation in the group: converting response headers costs one
`HashMap` plus two `String`s *per header*, and the body conversion routes through a
`bytesbuf` API whose own documentation says to avoid it in performance-sensitive code.

Two of its costs (the `#[async_trait]` boxing and the `Box::pin` on the response body) are
imposed by the `azure_core` trait signatures and are genuinely unavoidable; they are
recorded as Low with that noted.

### Findings

#### F19. Response header conversion allocates a HashMap and two Strings per header

- **Location:** `crates/fetch_azure/src/client.rs:138-155`.
- **Issue:** `to_headers` builds a `HashMap`, then for each response header inserts
  `name.as_str().to_owned()` and `value.to_owned()` — two heap allocations per header — and
  finally constructs `Headers::from(map)`. A typical HTTP response carries around ten
  headers, so this is one `HashMap` allocation (plus its growth reallocations) and roughly
  twenty `String` allocations **per response**. Header names in particular are almost always
  well-known static strings, and header values are already backed by refcounted `Bytes` in
  the underlying `http` types, so essentially none of this copying is fundamental.
- **Impact:** Medium (arguably High for header-heavy responses) — it is per response, it is
  unconditional, and it scales with header count, which the caller does not control.
- **Remediation:** Check whether `azure_core::Headers` can be constructed from an iterator
  of borrowed or refcounted values rather than a fully owned `HashMap`; if it can, avoid the
  intermediate map entirely. If the owned map is unavoidable, at minimum pre-size it with
  `HashMap::with_capacity(headers.len())` to eliminate the growth reallocations, and use the
  static-string fast path for well-known header names.
- **Evidence:** inferred from code reading. Confirmable with a Criterion benchmark
  `crates/fetch_azure/benches/adapter.rs`, group `adapter/to_headers`, using `alloc_tracker`
  to count allocations against header count (2, 10, 30 headers).

#### F20. Response body converted with `BytesView::to_bytes`, which the source crate documents as slow

- **Location:** `crates/fetch_azure/src/client.rs:~131` (`.map_ok(|view| view.to_bytes())`);
  the implementation and its warning at `crates/bytesbuf/src/bytes_compat/to_bytes.rs:34-45`
  (doc) and `:55-82` (code).
- **Issue:** Every body chunk is passed through `BytesView::to_bytes()`. Reading the
  implementation: the conversion is zero-copy **only** when the view consists of a single
  contiguous span; for a multi-span view — which is the normal shape for a chunked or
  scattered read — it **copies the entire chunk** into a fresh allocation. And it *always*
  allocates the `Bytes` metadata even in the single-span case. The `bytesbuf` crate's own
  documentation is unambiguous: "You generally want to avoid this conversion in
  performance-sensitive code." A per-response body adapter is performance-sensitive code.
- **Impact:** Medium — potentially a full copy of the response body, proportional to body
  size, which for a storage or data-plane Azure client is the dominant cost.
- **Remediation:** The `azure_core::PinnedStream` signature requires `Bytes`, so the
  conversion cannot be eliminated outright at this layer. But it can be made cheap: ensure
  the upstream read path produces single-span views where possible (so the zero-copy branch
  is taken), or yield one `Bytes` per span rather than one per view, so no span is ever
  copied. Either keeps the public signature intact.
- **Evidence:** inferred from code reading, corroborated by the explicit in-tree warning at
  `crates/bytesbuf/src/bytes_compat/to_bytes.rs:34-45`. Confirmable with a Criterion
  benchmark streaming a multi-megabyte body through the adapter and comparing bytes copied
  (`alloc_tracker` totals) against body size — a zero-copy path is flat, the current one is
  linear.

#### F21. Method and URL passed as `&str`, forcing a re-parse per request

- **Location:** `crates/fetch_azure/src/client.rs:~55`.
- **Issue:** The adapter passes the method and URL into `client.request(...)` as string
  slices. The incoming `azure_core::Request` already holds a fully parsed
  `azure_core::Url`, so the URL is serialised back to a string (or borrowed as one) and then
  re-parsed into an `http::Uri` on every request — parsing work that was already done once,
  redone. The method is worse: it is one of a handful of static constants and is being
  re-parsed from text.
- **Impact:** Medium — per request, unconditional, and entirely redundant.
- **Remediation:** Construct the `http::Uri` from the parsed `Url`'s components (or cache
  the converted `Uri` on the request where possible) and map the method through a `match` on
  `azure_core`'s method enum to the corresponding `http::Method` constant, which is free.
- **Evidence:** inferred from code reading. Confirmable with a Callgrind benchmark
  `crates/fetch_azure/benches/adapter_cg.rs` isolating request conversion.

#### F22. `#[async_trait]` boxes a future per request

- **Location:** `crates/fetch_azure/src/client.rs` — the `execute_request` implementation.
- **Issue:** `#[async_trait]` desugars to `Pin<Box<dyn Future + Send>>`, so every request
  allocates a boxed future.
- **Impact:** Low — one allocation per request, which is real, but it is **imposed by the
  `azure_core::HttpClient` trait definition**, which is an external crate. Nothing can be
  done at this layer.
- **Remediation:** None available locally. If `azure_core` ever adopts native async fn in
  traits, drop the macro. Recorded so that a future reader does not re-litigate it.
- **Evidence:** inferred from code reading.

#### F23. `Box::pin` on the response body per response

- **Location:** `crates/fetch_azure/src/client.rs` — the response construction path.
- **Issue:** The body stream is `Box::pin`ned to satisfy `azure_core::PinnedStream`.
- **Impact:** Low — one allocation per response, and again **imposed by the external trait's
  type alias**. Unavoidable at this layer.
- **Remediation:** None available locally.
- **Evidence:** inferred from code reading.

### Benchmark coverage

**Zero benchmarks.** This crate is a thin adapter, but F19, F20 and F21 are all
per-request/per-response costs that a small `benches/adapter.rs` with `alloc_tracker` would
make visible immediately, without needing any network access — the conversions are pure
functions of a request/response value. Of everything missing in this group, this is among
the cheapest benchmark to write and among the most likely to show a real number.

### Considered and ruled out

- **Error conversion.** Cold path; allocations there are irrelevant.
- **Client construction.** Setup only.
- **Zero `#[inline]` (1 public function).** With a single public function, an `#[inline]`
  census finding would be noise. No finding.

## Crate: fetch_winhttp

### Summary

**No performance issues found.**

`crates/fetch_winhttp` is a 36-line design-only placeholder. There is no implementation: no
request path, no connection handling, no data structures, no dependencies of consequence.
There is nothing to analyse for allocation, dispatch, locking, layout or inlining, and no
code that could be benchmarked.

### Findings

None. The crate contains no executable logic.

### Benchmark coverage

No benchmarks, correctly — there is nothing to benchmark. When the WinHTTP transport is
actually implemented, it should be held to the same bar as `fetch_hyper`: a loopback
throughput benchmark and a connect-path benchmark with `alloc_tracker`, plus a Callgrind
pair for the per-request path. Recording that here so the gap is not silently inherited.

### Considered and ruled out

- **The placeholder's public surface.** Nothing is exported that a caller could route a
  request through, so there is no API-forces-slow-path finding to make.
- **Feature flags.** The crate declares nothing perf-relevant.
- **Platform-conditional compilation.** Not yet present.
- **Pre-emptive design review of the intended WinHTTP binding.** Out of scope for a
  performance analysis of existing code, and speculative; noted under benchmark coverage
  instead.

## Crate: rest_over_grpc

### Summary

`rest_over_grpc` is the largest crate in this group (about 13,134 lines) and the one whose
hot path is most clearly allocation-dominated: every request is transcoded from REST into
gRPC and every response back again, which means query parsing, JSON decoding, field
overlay, JSON encoding and status mapping, per request, in the request thread.

The findings cluster into four groups. **Wasted work in the overlay decoder**: a `HashSet`
is built to deduplicate query keys against a body that is provably empty for
`RequestBodyKind::None`, and the query string is scanned twice for two different character
classes. **Unsized and unreusable buffers**: response encoding starts from a
zero-capacity `Vec`, forcing serde_json to grow it repeatedly, and the buffer-reusing entry
point that would fix this exists but is `pub(crate)` so generated code cannot reach it.
**Layered boxing on the streaming path**: three nested `Box::pin` layers mean three
pointer indirections per `poll_next`. **Allocation in the status type**: `Status::new`
takes `impl Into<String>` and heap-allocates even for the overwhelmingly common
`&'static str` message.

Layered on top: 118 public functions with zero `#[inline]`, including the per-request query
helpers that generated code calls directly, and **zero benchmarks** in the crate itself.

There is also good work here, recorded in "Considered and ruled out" — the unary body path
is genuinely zero-copy and deliberately unboxed, and the feature factoring is clean.

### Findings

#### F24. `decode_flat` builds a deduplication `HashSet` against a body that is always empty for GET

- **Location:** `crates/rest_over_grpc/src/transcode/overlay.rs:94-114`; the wasted work is
  lines 105-106, with the relevant `body_entries` initialisation at line 102.
- **Issue:** `decode_flat` constructs a `HashSet<Cow<str>>` containing every query key
  (line 105) and then calls `body_entries.retain(...)` against it (line 106) to drop body
  fields shadowed by query parameters. But when `RequestBodyKind::None` — which is the case
  for every `GET` and `DELETE`, i.e. the majority of REST traffic — `body` is `None` and
  line 102 sets `body_entries` to an empty `Vec`. The `HashSet` is therefore built,
  populated with every query key (hashing each one), and consulted zero times, on every
  GET-with-query-parameters request. This is a heap allocation plus O(k) hashing per
  request for provably no effect, which is the "no allocation on the hot path" rule broken
  in its purest form: not a necessary allocation that could be avoided with effort, but an
  allocation whose result is discarded.
- **Impact:** Medium — one allocation plus k hash computations per GET-with-query request.
  It would be High if the transcode path were the only cost, but the surrounding JSON work
  is heavier; still, this is the single easiest win in the crate.
- **Remediation:** Guard both lines on `!body_entries.is_empty()`. Two lines moved inside an
  `if`. Behaviour is identical because `retain` on an empty `Vec` is a no-op regardless of
  the set's contents. This is as surgical as a fix gets.
- **Evidence:** inferred from code reading. Confirmable with a Criterion benchmark
  `crates/rest_over_grpc_tests/benches/rog_transcode.rs`, group `rog_transcode/decode_flat`,
  driving a `GET` with 0, 4 and 16 query parameters and asserting allocation counts with
  `alloc_tracker` — note that Callgrind alone will understate this, since it models
  allocation as a flat instruction cost (see the benchmark coverage section).

#### F25. `encode_response` serialises into a zero-capacity `Vec`

- **Location:** `crates/rest_over_grpc/src/transcode/mod.rs:140-144`; contrast with
  `crates/rest_over_grpc/src/stream.rs:~208-215` (`serialize_framed_item`).
- **Issue:** `encode_response` starts from `Vec::new()` — capacity zero — and hands it to
  serde_json. serde_json grows the buffer by doubling, so a response of n bytes costs
  roughly log2(n) reallocations, each of which is an allocation plus a memcpy of everything
  written so far. For a 4 KB JSON response that is on the order of nine reallocations and
  roughly 8 KB of redundant copying, per response. The crate already knows better: its own
  `serialize_framed_item` on the streaming path correctly uses
  `Vec::with_capacity(128 + 8)`, so the inconsistency is internal.
- **Impact:** Medium — per response, proportional to response size, and responses are the
  larger half of most REST traffic.
- **Remediation:** Start from `Vec::with_capacity(n)` with a sensible default (the streaming
  path's 128-byte floor is a reasonable model), or better, thread through a size hint from
  the message where one is available. One line.
- **Evidence:** inferred from code reading. Confirmable with a Criterion benchmark
  `rog_transcode.rs`, group `rog_transcode/encode_response`, sweeping response size over
  256 B / 4 KB / 64 KB and counting allocations with `alloc_tracker`.

#### F26. The buffer-reusing encode entry point exists but is `pub(crate)`, so generated code cannot use it

- **Location:** `crates/rest_over_grpc/src/transcode/mod.rs:146-160`
  (`encode_response_into(&mut Vec<u8>)`); the generated call site is produced by
  `crates/rest_over_grpc/src/build/service_definition.rs:941`.
- **Issue:** The crate already has the right primitive — `encode_response_into`, which
  serialises into a caller-supplied buffer and therefore permits buffer reuse across
  responses — but it is `pub(crate)`. The code generator at `service_definition.rs:941`
  emits calls to the allocating `encode_response` instead, because that is the only thing
  visible to generated code, which lives in the *user's* crate. So every generated service
  is locked out of the fast path that already exists, by a visibility modifier.
- **Impact:** Medium — this is what turns F25 from "one suboptimal default" into "no
  generated service can ever reuse a response buffer". Combined, they mean a fresh
  allocation and a growth sequence per response with no opt-out.
- **Remediation:** Make `encode_response_into` public (or expose it via a `#[doc(hidden)]`
  `__private` module, the ecosystem-standard way to expose codegen support functions without
  committing to a public API), and have the generator emit calls to it with a per-connection
  or per-task reusable buffer. The generator change is larger than the visibility change, so
  these can ship separately.
- **Evidence:** inferred from code reading. Confirmable by the same `encode_response`
  benchmark as F25 with a reuse variant added.

#### F27. Three nested `Box::pin` layers on the streaming response path

- **Location:** `crates/rest_over_grpc/src/transcode_response.rs:62`
  (`ResponseStream<T> = Pin<Box<dyn Stream>>`) and `:116` (`frames: Box::pin(frames)`);
  `crates/rest_over_grpc/src/stream.rs:~290-320` (`FrameState`, `stream: Box::pin(items)`).
- **Issue:** A streaming response passes through three independent boxing layers: the item
  stream is boxed into `ResponseStream`, then `encode_frames_response` boxes it again inside
  `FrameState`, then `StreamingResponse::new` boxes the frame stream a third time. The
  result is three heap allocations at stream setup (minor) and, more importantly, **three
  pointer indirections and three vtable dispatches on every single `poll_next`** — i.e. per
  frame, for the life of the stream. For a server-streaming RPC delivering thousands of
  frames this is paid thousands of times.
- **Impact:** Medium — per frame on every streaming response. Streaming is not the majority
  of REST-over-gRPC traffic, which keeps this below the top tier, but where it is used it is
  used for high-volume transfers, which is exactly where per-frame overhead matters.
- **Remediation:** Honest assessment: this is **not purely surgical**. Collapsing the layers
  requires either making the intermediate types generic over the stream (which changes the
  public `ResponseStream` alias and therefore the public API) or adding an
  `Unpin`-specialised entry point that skips the inner boxing when the source stream is
  already `Unpin` — also a public API addition. The contained first step is to remove the
  *middle* layer: `FrameState` boxing a stream that the caller has already boxed is
  redundant, and making `FrameState` generic over `S: Stream` is an internal change with no
  API impact. That alone removes one of the three indirections.
- **Evidence:** inferred from code reading. Confirmable with a Criterion benchmark
  `rog_transcode.rs`, group `rog_transcode/stream_frames`, driving a 10,000-frame stream to
  completion and measuring per-frame cost; a Callgrind pair would isolate the indirection
  cost cleanly since it is pure instruction count with no allocation involved.

#### F28. Per-request query helpers called by generated code are not `#[inline]`

- **Location:** `crates/rest_over_grpc/src/path.rs:28-33` (`split_query`), `:49-77`
  (`parse_query`), `:145-154` (`QueryPairs::{as_slice, iter, len, is_empty}`).
- **Issue:** These functions are called **per request** by code generated into the user's
  own crate, so every call crosses a crate boundary. They are generic only over lifetimes,
  which means they are not instantiated in the caller's crate and are therefore not
  available for inlining — and with `[profile.release]` setting no LTO, they are real calls
  in every shipped build. `QueryPairs::len` and `is_empty` in particular are one-line
  accessors being called through a non-inlinable cross-crate boundary.
- **Impact:** Medium — small per call, but these are on the per-request path of every
  generated service, and the accessors are called repeatedly within a single request's
  parameter binding.
- **Remediation:** Add `#[inline]` to the accessors and to `split_query`. `parse_query` is
  large enough that `#[inline]` is questionable — leave it, or use `#[inline]` only if a
  benchmark shows a win, in line with the "be judicious" guidance.
- **Evidence:** inferred from code reading (attribute census plus generated call-site
  inspection). Confirmable only under release-profile codegen settings; the current
  fat-LTO `[profile.bench]` hides it entirely.

#### F29. Body field decoding allocates a String per field plus a HashMap for duplicate detection

- **Location:** `crates/rest_over_grpc/src/transcode/overlay.rs:169-176`
  (`BodyTop::visit_map`).
- **Issue:** For each field of the request body, `visit_map` does three allocating things:
  it maintains a `HashMap<String, ()>` purely to detect duplicate keys; it calls
  `next_key::<String>()`, which forces serde to allocate an owned `String` for every key
  even though the JSON input buffer is available to borrow from; and it calls
  `normalized_key(&key).into_owned()`, which allocates **even when the returned `Cow` is
  already `Borrowed`** — the `into_owned()` is unconditional. So a body with k fields costs
  roughly 2k `String` allocations plus a `HashMap` and k hashes.
- **Impact:** Medium — proportional to field count on every request with a body, i.e. every
  POST/PUT/PATCH. The `into_owned()` on an already-borrowed `Cow` is the most clearly
  wasteful part and the easiest to fix.
- **Remediation:** Three independent steps, in increasing order of effort. (a) Keep the
  `Cow` as a `Cow` and only call `into_owned()` at the point where ownership is genuinely
  required — for the borrowed case this eliminates the allocation entirely. (b) Use
  `next_key::<&str>()` (or `Cow<str>`) so serde borrows from the input buffer where the JSON
  contains no escapes, which is the common case. (c) Replace the duplicate-detection
  `HashMap<String, ()>` with a `HashSet<&str>` borrowing the same keys, or with a bitset over
  the known field indices — the set of valid fields is known from the schema at codegen
  time, so duplicate detection does not need a general-purpose hash map at all.
- **Evidence:** inferred from code reading. Confirmable with a Criterion benchmark
  `rog_transcode.rs`, group `rog_transcode/decode_body`, over bodies with 2, 8 and 32 fields
  with `alloc_tracker` counting allocations — the expected signature of the current code is
  a clean linear rise with field count.

#### F30. Query string scanned twice for two different character classes

- **Location:** `crates/rest_over_grpc/src/transcode/overlay.rs:25-55`; the double scan is
  at line 50.
- **Issue:** `try_decode_overlay` calls `all_flat(query)`, which scans every key looking for
  `.` (to decide whether the query uses nested-field syntax), and then immediately runs
  `.iter().any(needs_decoding)`, which scans the same keys again looking for `%` or `+` (to
  decide whether percent-decoding is required). Two full passes over the same small strings
  where one pass could compute both predicates.
- **Impact:** Low — query strings are short and both scans are cache-friendly byte
  comparisons, so the absolute cost is small. Recorded because it is trivially fusible.
- **Remediation:** Fuse into a single pass returning both booleans. A handful of lines, no
  behaviour change.
- **Evidence:** inferred from code reading. Confirmable with a Callgrind benchmark
  `rog_transcode_cg.rs` (which already exists) extended with a query-scan case — this is
  exactly the kind of pure-instruction-count difference Callgrind measures well.

#### F31. `Status` always heap-allocates its message, including for `&'static str`

- **Location:** `crates/rest_over_grpc/src/status.rs:42-46` (the struct) and `:61-67`
  (`Status::new`).
- **Issue:** `Status` is `{ code, message: String, details: Vec<Value> }` and
  `Status::new(impl Into<String>)` therefore heap-allocates the message unconditionally. The
  overwhelmingly common construction site passes a `&'static str` literal — a fixed error
  message like `"invalid argument"` — which means an allocation and a copy purely to store a
  pointer to data that is already in the binary's rodata section. `Status` is also constructed
  on the *validation* path, not only on genuinely exceptional paths, so this is not purely a
  cold-path cost.
- **Impact:** Medium — one avoidable allocation per error response, and error responses are
  common enough in a REST API (400s from validation) to be considered warm.
- **Remediation:** Change the field to `Cow<'static, str>` and take
  `impl Into<Cow<'static, str>>`. `&'static str` then stores with zero allocation while
  `String` still works, and the type stays the same size (verified below). Callers are
  source-compatible in the common cases. Note this is a public API change, so it needs a
  deliberate decision, but it is a small and additive-feeling one.
- **Evidence:** **empirically verified** for the layout component (standalone `rustc`
  `size_of` replica, dependency-free, run in `/tmp` and deleted afterwards):
  `Status` = **56 bytes**, `Result<_, Status>` = **56 bytes**, `serde_json::Value` replica
  = **32 bytes**. The allocation behaviour itself is inferred from code reading.
  Confirmable with a Criterion benchmark `rog_transcode.rs`, group `rog_transcode/status`,
  with `alloc_tracker` asserting zero allocations for a `&'static str` message after the fix.

#### F32. `TranscodeError::from_source` formats, boxes and captures a backtrace eagerly

- **Location:** `crates/rest_over_grpc/src/transcode/error.rs:80-87`.
- **Issue:** `from_source` performs `source.to_string()` (allocating and formatting), then
  `Box::new(source)` (a second allocation), and the surrounding `ohno`-based error type
  captures a backtrace. That is three distinct costs paid at error-construction time.
- **Impact:** Low — this is the error path, and `docs/performance.md` is clear that
  defensive and diagnostic work should be preserved rather than trimmed. Eager
  `to_string()` of the source is the one part that is arguably redundant, since the boxed
  source is retained and could be formatted on demand at display time.
- **Remediation:** Consider dropping the eager `to_string()` and formatting from the retained
  boxed source in the `Display` impl. Do **not** remove the backtrace capture — that is
  diagnostic value the house philosophy tells us to keep.
- **Evidence:** **empirically verified** for layout (`TranscodeError` ≈ **56 bytes** by
  standalone `rustc` replica) — with the caveat that the replica models `ohno`'s backtrace
  field as a `Box`-like pointer, which was not verifiable in this container, so the number is
  exact only under that assumption. The allocation behaviour is inferred from code reading.
- **Philosophy note:** **Partially conflicting.** Trimming error-path work runs against
  "preserve defensive runtime checks" and the general deprioritisation of cold paths.
  Reported for completeness; only the redundant `to_string()` is worth considering.

#### F33. Static error bodies are copied with `to_vec()` on every invocation

- **Location:** `crates/rest_over_grpc/src/serving.rs:224-238` (`body_read_failed`,
  `body_too_large`).
- **Issue:** Both helpers call `.to_vec()` on a `&'static [u8]` error body, allocating and
  copying a fixed byte string every time they are invoked.
- **Impact:** Low — cold path (these fire only on malformed or oversized requests), and the
  bodies are short.
- **Remediation:** `Bytes::from_static(...)` is allocation-free and is the direct
  replacement. A one-line change per function, so despite the low impact the
  cost/benefit is favourable.
- **Evidence:** inferred from code reading.

#### F34. Zero `#[inline]` across 118 public functions

- **Location:** crate-wide; census of `crates/rest_over_grpc/src`.
- **Issue:** The largest crate in the group, with by far the most public surface called
  from generated code in other crates, has **zero** `#[inline]` attributes. F28 covers the
  most clearly hot subset; this records the crate-wide picture. As elsewhere, the absence of
  LTO in `[profile.release]` means none of this is recovered at link time for consumers.
- **Impact:** Medium.
- **Remediation:** Prioritise the functions that generated code calls per request (the
  `path` module, the transcode entry points, the `Status` constructors) and leave the rest.
- **Evidence:** inferred from code reading (attribute census).

#### F35. Zero benchmarks in a 13,134-line crate

- **Location:** `crates/rest_over_grpc` — no `benches/` directory.
- **Issue:** The crate with the group's most complex per-request work has no benchmark of
  its own. The only benchmarks that touch it live in `rest_over_grpc_tests` and are
  Callgrind-only (see that crate's section), which is a poor fit for allocation-dominated
  code.
- **Impact:** Medium — not a runtime cost, but it is why F24 through F31 all had to be
  reported as inferred rather than measured, and why a regression in any of them would go
  unnoticed.
- **Remediation:** Add Criterion benchmarks. Whether they live here or in
  `rest_over_grpc_tests` is a judgement call — the tests crate already has the route table
  and the generated services, so hosting them there is pragmatic and also fixes the pairing
  violation.
- **Evidence:** directly observed (directory listing).

### Benchmark coverage

The crate itself has **no benchmarks**. Its only coverage is the two Callgrind files in
`rest_over_grpc_tests` (`rog_router_cg.rs`, `rog_transcode_cg.rs`), which have no paired
Criterion files — a violation of `docs/naming.md:81-90`, analysed in that crate's section.

The deeper problem is a **structural blind spot**: the transcode path's dominant cost is
allocation (F24, F25, F26, F29, F31 are all allocation findings), and Callgrind models an
allocation as a flat instruction count for the call itself — it does not see the allocator's
real cost, the memcpy on realloc, the cache effects, or the pressure on the global allocator
under concurrency. So the only benchmarks that exist for this crate are the kind least able
to see its actual performance characteristics. `docs/callgrind-benchmarks.md` positions
Callgrind as a complement to Criterion, not a substitute, and this is a live example of why.

Priority additions: Criterion `rog_transcode.rs` with `alloc_tracker`, covering
`decode_flat` (F24), `encode_response` (F25/F26), `decode_body` (F29) and `status` (F31);
Criterion `rog_router.rs` to pair with the existing `rog_router_cg.rs`; and a
`stream_frames` group for F27.

### Considered and ruled out

- **`Bytes::from(Vec<u8>)` at `crates/rest_over_grpc/src/serving.rs:262` and `:267`.**
  This conversion is O(1) — `Bytes` takes ownership of the `Vec`'s allocation without
  copying. Looks like a copy, isn't one. **No finding.**
- **`read_body_uncapped` (`serving.rs`).** Uses `collect().to_bytes()`, which on this path is
  the zero-copy single-span case. Correct as written. **No finding.**
- **`RestBody` keeps unary bodies unboxed (`serving.rs:63-65`).** A deliberate and correct
  choice: only the streaming variant pays for boxing, so the common unary case is free.
  **Positive note.**
- **Feature factoring (`crates/rest_over_grpc/Cargo.toml:54-84`).** `default = ["serving"]`
  with `tower`, `layered`, `axum` and `build` all opt-in. This is well-factored — a consumer
  who only needs transcoding does not compile the axum or tower integration, and the build-time
  code generator is not in the default runtime dependency set. **Positive note, no finding.**
- **`RequestBodyKind` (24 bytes) and `ResponseBodyKind` (32 bytes).** Verified by standalone
  `rustc` replica. Both are small and neither has a disproportionate variant; no
  large-enum-variant finding.
- **`serde_json::Value` as the intermediate representation.** Using a dynamically typed
  intermediate necessarily allocates, and a fully borrowed/zero-copy transcoder would be an
  architectural rewrite of the crate's core. `docs/performance.md` asks for surgical over
  architectural and for a real scenario before a structural change; I do not have the
  measurements to justify one, so I am explicitly *not* raising this as a finding. It is
  recorded here so that a future reader knows it was considered.
- **Router implementation.** The existing `rog_router_cg.rs` benchmark compares against
  `matchit`, which suggests the routing algorithm has already had performance attention. I
  found nothing to add.

## Crate: rest_over_grpc_tests

### Summary

A test-support crate that hosts generated services and the only benchmarks that touch
`rest_over_grpc`. It contains no production code, so there are no runtime findings. Both
findings concern the benchmark suite itself: it violates the mandatory Criterion/Callgrind
pairing rule, and — more importantly — the benchmarks it does have are structurally unable
to see the costs that dominate the code they benchmark.

### Findings

#### F36. Callgrind benchmarks with no paired Criterion files — violates `docs/naming.md:81-90`

- **Location:** `crates/rest_over_grpc_tests/benches/rog_router_cg.rs`,
  `crates/rest_over_grpc_tests/benches/rog_transcode_cg.rs`; the `[[bench]]` registrations at
  the bottom of `crates/rest_over_grpc_tests/Cargo.toml` list only these two.
- **Issue:** `docs/naming.md:81-90` states the pairing rule: a Criterion benchmark may exist
  without a Callgrind counterpart, but a Callgrind benchmark must have a paired Criterion
  file of the corresponding name. Both `_cg.rs` files here are unpaired — there is no
  `rog_router.rs` and no `rog_transcode.rs`. The `Cargo.toml` `[[bench]]` entries confirm it
  is not merely a missing file but a deliberate registration of only the Callgrind halves.
  The rule exists because instruction counts alone cannot tell you whether a change made the
  wall-clock faster; without the Criterion half there is no wall-clock signal at all.
- **Impact:** Medium — a process/coverage defect rather than a runtime cost, but it is a
  documented mandatory rule being violated in the only benchmarks covering the group's
  largest crate.
- **Remediation:** Add `benches/rog_router.rs` and `benches/rog_transcode.rs` with Criterion
  groups named `rog_router/...` and `rog_transcode/...` to match the file basenames per
  `docs/naming.md`, and register them in `Cargo.toml`.
- **Evidence:** directly observed (file listing and `Cargo.toml` inspection) against the
  rule text at `docs/naming.md:81-90`.

#### F37. The existing benchmarks are structurally blind to the transcode path's dominant cost

- **Location:** `crates/rest_over_grpc_tests/benches/rog_transcode_cg.rs` (in relation to
  `crates/rest_over_grpc/src/transcode/`).
- **Issue:** Every significant finding against `rest_over_grpc` is an allocation finding
  (F24, F25, F26, F29, F31). Callgrind counts instructions and models a call to the allocator
  as a flat instruction cost; it does not capture allocator work proportional to size, the
  memcpy performed on `Vec` growth, cache behaviour, or contention on the global allocator
  under concurrent load. So the crate's only benchmarks measure the dimension along which its
  actual problems are least visible. A change that removed every allocation in F24-F31 might
  show only a modest instruction-count improvement while producing a much larger wall-clock
  and throughput improvement — and conversely, an allocation regression could pass unnoticed.
- **Impact:** Medium — it means the safety net that exists gives false confidence.
- **Remediation:** The Criterion files required by F36 should use `alloc_tracker` (as
  `crates/fetch`'s benches already do) so that allocation counts are asserted directly rather
  than inferred from instruction counts. That single addition converts most of this
  document's `rest_over_grpc` findings from inferred to measurable.
- **Evidence:** inferred from reading the benchmark files against
  `docs/callgrind-benchmarks.md`, which itself positions Callgrind as complementary to, not a
  replacement for, wall-clock measurement.

### Benchmark coverage

Two Callgrind files, zero Criterion files — the inverse of the recommended shape. The route
table used by both `build.rs` and the `grs_router_vs_matchit` benchmark lives in
`bench_routes.rs` at the **crate root** rather than under `src/` or `benches/`, which is
unusual placement; it is shared between the build script and the benchmark, which explains
it, but it makes the benchmark's inputs easy to miss when auditing coverage. The table holds
40-plus routes, which is a realistic size for a router benchmark — that part is well done.

### Considered and ruled out

- **Runtime performance of the test-support code itself.** It is test scaffolding; its own
  speed is irrelevant except insofar as it slows CI, which is out of scope.
- **Generated service code volume.** Large, but it is generated at build time from the route
  table and does not affect the shipped product.
- **`bench_routes.rs` placement.** Unusual but justified by the build-script sharing. Noted
  above rather than raised as a finding.

## Crate: rest_over_grpc_examples

### Summary

An examples crate. It contains no library code on any hot path, so there are no runtime
findings. Its performance relevance is entirely about opportunity: several of its examples
already construct exactly the scenarios that the missing benchmarks need, so it is the
cheapest available host for closing the `rest_over_grpc` coverage gap.

### Findings

#### F38. Examples exercise realistic performance-relevant paths but host no benchmarks

- **Location:** `crates/rest_over_grpc_examples/examples/serving/streaming_response.rs`,
  `crates/rest_over_grpc_examples/examples/transcoding/basic_transcode.rs`,
  `crates/rest_over_grpc_examples/examples/handling/client_streaming_upload.rs`; no
  `benches/` directory in the crate.
- **Issue:** The examples under `examples/{build,handling,serving,transcoding}` set up
  genuinely realistic paths — `basic_transcode.rs` drives the request/response transcode that
  F24, F25 and F29 all live on; `streaming_response.rs` drives exactly the triple-boxed frame
  path of F27; `client_streaming_upload.rs` drives the request-body streaming path. That
  scaffolding is already written and already compiles, yet the crate has no benchmarks, so
  none of it is measured.
- **Impact:** Low — no runtime cost; this is a coverage opportunity rather than a defect.
- **Remediation:** Either add `benches/` here reusing the example setups, or (probably better)
  lift the shared setup into the crate so both the examples and the new
  `rest_over_grpc_tests` benchmarks required by F36 can use it. Consolidating in
  `rest_over_grpc_tests` avoids splitting benchmark coverage of one crate across two hosts.
- **Evidence:** directly observed (directory listing and example inspection).

### Benchmark coverage

**Zero benchmarks.** For an examples crate that is normal and not itself a defect; the point
of F38 is that the setup code here is a ready-made foundation for the benchmarks that
`rest_over_grpc` is missing.

### Considered and ruled out

- **Example code efficiency.** Examples are optimised for clarity, and should be; nothing
  here is shipped.
- **Compile time of the examples.** Out of scope.
- **Whether examples should be run in CI as smoke tests.** A correctness/process question,
  not a performance one.

## Appendix: cross-cutting observations

### Inline census

| Crate | Public fns | `#[inline]` |
|---|---:|---:|
| `fetch` | 62 | 0 |
| `fetch_hyper` | 17 | 0 |
| `fetch_tls` | 21 | 0 |
| `fetch_options` | 25 | 0 |
| `fetch_azure` | 1 | 0 |
| `rest_over_grpc` | 118 | 0 |
| **Total** | **244** | **0** |

This is only actionable because `[profile.release]` sets no `lto` and no
`codegen-units = 1`: consumers building in release mode get no cross-crate inlining of
non-generic functions. Under `[profile.bench]` (fat LTO, one codegen unit) the compiler
recovers all of it, which is precisely why no existing benchmark can show the cost. The
recommendation is not to blanket-annotate — `docs/performance.md` asks for judgement — but
the trivial `Copy` accessors in F17 and the generated-code entry points in F28 are the
uncontroversial cases.

### Verified type layouts

Obtained by compiling a dependency-free replica with plain `rustc` in `/tmp` and printing
`size_of`/`align_of`; the program was deleted afterwards and never added to the repo.

| Type | Size | Note |
|---|---:|---|
| `serde_json::Value` (replica) | 32 B | baseline for the types below |
| `rest_over_grpc::Status` | 56 B | `Cow` swap in F31 keeps this unchanged |
| `Result<_, Status>` | 56 B | niche-packed, no growth |
| `TranscodeError` | 56 B | assumes `ohno`'s backtrace field is pointer-sized |
| `RequestBodyKind` | 24 B | no oversized variant |
| `ResponseBodyKind` | 32 B | no oversized variant |

No large-enum-variant finding arises from any of these; the enums in this group are
well-proportioned, and the one type that *is* large (`TlsBackend`) has a documented and
correct justification.

### Benchmark coverage across the group

| Crate | Criterion | Callgrind |
|---|---|---|
| `fetch` | `http_crate.rs`, `pipelines.rs` | none |
| `fetch_hyper` | none | none |
| `fetch_tls` | none | none |
| `fetch_options` | none | none |
| `fetch_azure` | none | none |
| `fetch_winhttp` | none (no code) | none |
| `rest_over_grpc` | none | none |
| `rest_over_grpc_tests` | none | `rog_router_cg.rs`, `rog_transcode_cg.rs` |
| `rest_over_grpc_examples` | none | none |

Seven of nine crates have no benchmarks at all. The two that do have complementary
problems: `fetch` has Criterion but only single-threaded, so it cannot see its own worst
finding (F2); `rest_over_grpc_tests` has Callgrind but no Criterion, so it cannot see the
allocation costs that dominate the code it measures (F37). Combined with the cross-group
observation that `[profile.bench]` uses settings no consumer gets, the practical conclusion
is that the group's benchmark suite would not currently catch a regression in any of this
document's High or Medium findings.

### Severity roll-up

- **High (2):** F1 (eager redaction per request), F2 (double type erasure plus shared mutex
  under `Isolation::Shared`).
- **Medium (22):** F3, F4, F6, F7, F10, F13, F17, F18, F19, F20, F21, F24, F25, F26, F27,
  F28, F29, F31, F34, F35, F36, F37 — of which the highest confidence-to-effort ratios are
  F24 (two lines), F25 (one line), F17 (`#[inline]` on trivial accessors) and F33
  (`Bytes::from_static`).
- **Low (rest):** F5, F8, F9, F11, F12, F14, F15, F16, F22, F23, F30, F32, F33, F38.

Findings flagged as conflicting with the house philosophy: F11 and F16 (both setup costs
that `docs/performance.md` deprioritises) and F32 (partially — trimming error-path work runs
against "preserve defensive runtime checks"). They are reported because the brief asks for
exhaustive coverage, not because they are recommended.
