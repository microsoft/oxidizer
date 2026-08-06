# g2-seatbelt findings

Scope: `crates/seatbelt`, `crates/seatbelt_http`.

**Environment caveat.** The analysis container has no egress to `index.crates.io` /
`static.crates.io`, no cargo registry cache and no prebuilt `target/`. `cargo build`,
`cargo bench`, `cargo clippy`, `--offline` and every `just` recipe fail at dependency
resolution (`error: no matching package named 'tokio' found ... location searched:
crates.io index`). This was confirmed and then set aside per instructions. Consequently
almost every finding below is labelled **inferred from code reading**, and each names the
specific benchmark that would confirm it.

The one empirical avenue that *does* work is type layout: a dependency-free program in
`/tmp`, compiled with plain `rustc -O`, containing field-for-field replicas of the private
types. Those numbers are labelled **empirically verified (rustc layout replica)** and the
replica was deleted afterwards; it was never added to the repository. The replicas were
checked field-by-field against `engine_core.rs:176-187`, `engine_core.rs:51-55`,
`health.rs:106-115`, `probing/options.rs:88-92`, `probing/probes.rs:12-15` and
`probing/health_probe.rs:10-17`.

**No source file was modified.** This is an analysis-only deliverable.

---

## Crate: seatbelt

### Summary

`seatbelt` is a well-built resilience middleware crate and the hot paths have clearly been
thought about: `Engines::get_engine` has an explicit lock-free fast path with a comment
explaining it (`engines.rs:35-41`), `EngineCore` carries a comment reminding maintainers to
keep `Clock::instant()` outside the lock (`engine_core.rs:38`), `HealthMetrics` keeps a
running aggregate instead of re-summing its sliding window (`health.rs:110-113`),
`EnableIf` is an enum so the default costs no virtual call (`utils/mod.rs:24-53`), `Rnd` is
a ZST outside `cfg(test)` (`rnd.rs:15-22`), and every telemetry field is `#[cfg]`-gated out
when the `metrics`/`logs` features are off (`utils/telemetry_helper.rs:4-14`,
`engine/engine_telemetry.rs:17-25`).

The residual issues cluster in four places:

1. **The circuit-breaker engine is a single `Mutex<State>` taken twice per request**
   (F1) and `State` is a 264-byte enum whose largest variant is only live during
   recovery (F2). This is the crate's scalability ceiling, not merely a latency cost.
2. **Hedging and retry pay unconditional per-request costs that only the *multi-attempt*
   case needs** — a `FuturesUnordered` allocation (F3), a full input clone that is
   discarded on the last attempt (F4), and a backoff-iterator construction (F5).
3. **Telemetry is cloned per attempt rather than shared** (F6), and the metric attribute
   arrays are rebuilt from scratch on every emission even though they are constant per
   engine (F7).
4. **Two allocations sit inside, or just before, a held lock** (F8, F9).

Plus a set of smaller items: the near-total absence of `#[inline]` (F10), an unconditional
RNG draw in chaos (F11), `f64::powi` in backoff (F12), boxed futures on the whole
`tower-service` surface (F13) and an `ExitCircuitResult` return-by-value of 64 bytes (F14).

### Findings

#### F1. The breaker engine takes a single global `Mutex<State>` twice on every request, even in the steady closed state

- **Location:** `crates/seatbelt/src/breaker/engine/engine_core.rs:34-48`; the state it
  guards at `:50-55`; the `Closed` arm of `State::enter` at `:58-62`.
- **Issue:** `EngineCore::enter()` (`:35-40`) does `self.clock.instant()` and then
  `self.state.lock()`. `EngineCore::exit()` (`:42-47`) does the same. So every single
  request through a breaker acquires and releases the *same* mutex twice. In the steady
  `Closed` state the work performed under the first lock is a single enum-discriminant
  read returning `EnterCircuitResult::Accepted { mode: ExecutionMode::Normal }`
  (`:59-62`) — it does not read `now` and does not mutate anything. The `now` computed at
  `:36` is therefore discarded on the common path, so the crate pays an unconditional
  `Clock::instant()` (a `clock_gettime` / vDSO read, or a virtual call through `Clock`'s
  frozen-clock abstraction) for nothing.
- **Impact:** **High** — this is a scalability ceiling rather than a fixed latency cost. A
  single `Breaker` layer is shared by every caller of the pipeline, and `Arc<Engine>` is
  shared across all threads (`engines.rs:16-17`). Two uncontended lock round-trips are
  ~40-80 cycles; under multi-core load on one hot origin the cache line holding the mutex
  word ping-pongs between cores and throughput saturates regardless of how fast the inner
  service is. The default `seatbelt_http` configuration makes this worse by partitioning
  per origin, which does *not* help when all traffic goes to one origin (the common case,
  and precisely the case `breaker.rs`'s `with-partitioned` bench models).
- **Remediation (surgical):** add an `AtomicU8` state summary alongside the mutex in
  `EngineCore`, written under the lock whenever `State` transitions. `enter()` then reads
  it with `Ordering::Relaxed` (or `Acquire`) and, when it reads `Closed`, returns
  `Accepted { mode: Normal }` without taking the lock *and without calling
  `clock.instant()*`. `exit()` still needs the lock in `Closed` because it records into
  `HealthMetrics`, so this halves rather than eliminates the locking; the `enter()` half is
  the cheap, high-value half because it also removes the wasted clock read. This preserves
  all defensive behaviour: a stale `Closed` read can at worst admit one extra request
  during the microsecond window of an open transition, which is already possible today
  because `enter()` and the inner service call are not atomic with respect to each other.
- **Evidence:** inferred from code reading. Confirmed by: a new **multi-threaded**
  Criterion scenario in `crates/seatbelt/benches/breaker.rs` (`with-breaker-contended`,
  N threads hammering one closed breaker) — there is currently no multi-threaded benchmark
  anywhere in the crate, so this cost is structurally invisible to the existing suite. The
  single-threaded half of the claim (the wasted `clock.instant()` plus two uncontended
  lock/unlock pairs) is measurable today by `breaker_cg.rs`'s `with_breaker` vs
  `no_breaker` instruction delta.

#### F2. `State` is a 264-byte enum whose largest variant is only live during recovery

- **Location:** `crates/seatbelt/src/breaker/engine/engine_core.rs:50-55`; payload types at
  `engine_core.rs:176-187` (`Stats`), `health.rs:106-115` (`HealthMetrics`),
  `probing/probes.rs:12-15` (`Probes`), `probing/health_probe.rs:10-17` (`HealthProbe`).
- **Issue:** measured sizes (x86-64):

  | type | size | align |
  |---|---|---|
  | `ExecutionInfo` | 12 | 4 |
  | `HealthEvaluator` | 16 | 4 |
  | `HealthMetrics` | 96 | 8 |
  | `Stats` | 64 | 8 |
  | `SingleProbe` | 40 | 8 |
  | `HealthProbe` | 168 | 8 |
  | `Probe` | 168 | 8 |
  | `Probes` | 200 | 8 |
  | **`State`** | **264** | 8 |
  | `Mutex<State>` | 272 | 8 |
  | `ExitCircuitResult` | 64 | 8 |
  | `EnterCircuitResult` | 1 | 1 |

  `State::Closed { health: HealthMetrics }` needs 96 bytes; `State::Open` needs 80;
  `State::HalfOpen { probes: Probes, stats: Stats }` needs 264. Every engine — including
  every engine that is permanently closed, which is all of them in a healthy system —
  reserves the full 264 bytes inline inside its mutex. That is ~5 cache lines per engine,
  of which the closed steady state touches at most 2 (the mutex word plus the discriminant
  and, on `exit()`, the `HealthMetrics` head).
- **Impact:** **Medium** — the per-engine memory cost is trivial in absolute terms, but the
  cache-locality cost is on the hottest path in the crate, and it scales with partition
  count: a partitioned breaker with 256 origins (the `with-partitioned-large` bench,
  `benches/breaker.rs:73`) holds 256 × 272 bytes ≈ 70 KB of engine state, most of it dead
  `Probes` space, competing for L2 with the `BTreeMap` it is reached through.
- **Remediation (surgical):** box the recovery-only payload —
  `HalfOpen { probes: Box<Probes>, stats: Stats }` — shrinking `State` to ~104 bytes and
  `Mutex<State>` to ~112 (two cache lines). The added allocation happens once per
  `Open → HalfOpen` transition, which is by definition rare and already on a path that
  allocates (F8), and the house guidance explicitly deprioritises first-insert/transition
  costs relative to steady-state costs.
- **Evidence:** **empirically verified (rustc layout replica in `/tmp`, `rustc -O`,
  field-for-field replicas of the six private types listed above; program deleted)**. The
  *benefit* of boxing is inferred; confirmed by `breaker_cg.rs` `with_partitioned_large`
  D1/LL miss counts before and after (Callgrind reports cache metrics, which is exactly
  what this finding is about).

#### F3. Hedging allocates a `FuturesUnordered` on every request even when no hedge can ever fire

- **Location:** `crates/seatbelt/src/hedging/service.rs:126-148`, specifically `:141-142`.
- **Issue:** `run_hedging` unconditionally does `FuturesUnordered::new()` and pushes the
  primary attempt into it. `FuturesUnordered::new()` is not free — it constructs an
  `Arc<ReadyToRunQueue>` containing a stub `Arc<Task>`, i.e. at least two heap allocations
  before a single future is pushed, and `push` allocates an `Arc<Task>` per future. The
  primary attempt is then driven through the intrusive-list machinery (atomic wakeup
  refcounting, `ReadyToRunQueue` dequeue) instead of being polled directly.
- **Impact:** **Medium-High** — this is on the *design-target* case for hedging: hedging
  exists so that the overwhelming majority of requests complete on the first attempt and no
  hedge is launched. Charging those requests ≥3 heap allocations plus intrusive-queue
  overhead directly contradicts the house rule "no allocation on the hot path"
  (`docs/performance.md`). Note also that the early-return escape hatch at `:134-137`
  (which skips `FuturesUnordered` entirely) is only taken when `clone_input` returns
  `None`, and the default `seatbelt_http` clone strategy returns `Some` for every safe
  method — see H3.
- **Remediation (surgical):** when `total_attempts == 1` (i.e. `max_hedged_attempts == 0`,
  computed at `:130`), take the same early-return shape as `:134-137` and `await` the
  single future directly with its `TelemetryGuard`, never constructing the
  `FuturesUnordered`. That is a two-line guard and changes no observable behaviour, because
  a `FuturesUnordered` containing exactly one future that is never joined resolves
  identically. A second, larger step — keeping the primary attempt out of the collection
  and `select!`ing it against the hedge set — would also help the `max_hedged_attempts > 0`
  case, but is architectural and should be a separate proposal.
- **Evidence:** inferred from code reading; the exact allocation count of
  `FuturesUnordered::new()` could not be verified because `futures-util`'s source is not
  available offline. Confirmed by: `crates/seatbelt/benches/hedging.rs` already has a
  `with-hedging-passthrough` scenario (`benches/hedging.rs:50-68`) using
  `max_hedged_attempts(0)`, and the file already installs `alloc_tracker::Allocator` as
  the global allocator (`:14-15`) — so the allocation count for this exact case is already
  being recorded and merely needs to be read. `with-hedging-delay` (`:31-48`) covers the
  `max_hedged_attempts > 0` variant.

#### F4. Retry clones the input on the final attempt and then throws the clone away

- **Location:** `crates/seatbelt/src/retry/service.rs:92-118`, specifically `:102`; the
  loop-exit logic in `evaluate_attempt`; `Attempt::first` in
  `crates/seatbelt/src/attempt.rs:156-158`.
- **Issue:** the retry loop calls `self.shared.clone_input(input, attempt, ...)` at the top
  of *every* iteration, including the last one. `clone_input` returns
  `(original_input, attempt_input)` — the original is retained purely so the loop can retry
  with it. On the last attempt `evaluate_attempt` always returns `ControlFlow::Break`, so
  the retained original is dropped unused. When `max_attempts == 1` (no retries
  configured, `Attempt::first(1)` yields `is_last() == true`), *every* request pays a full
  input clone whose only destiny is `Drop`.
- **Impact:** **High for `seatbelt_http` consumers, Medium in the abstract.** For a generic
  `In` the clone may be cheap. For the default HTTP configuration it is
  `HttpRequest::try_clone` — a body clone plus a `HeaderMap` allocation plus an
  `Extensions` (AnyMap) allocation with every boxed value re-boxed (see H1). Paying that
  on a pipeline configured with a single attempt is pure waste.
- **Remediation (surgical, and it belongs in `seatbelt_http`, not here):** the callback is
  supplied by the consumer, and `retry` already handles `None` as "no clone available"
  (`hedging/service.rs:134` shows the analogous handling). So `HttpRetryLayerExt::http_clone`
  (`crates/seatbelt_http/src/retry.rs:120-122`) can apply its `attach_attempt` /
  `update_request_uri` side effects and then return `None` when `args.attempt().is_last()`,
  skipping the deep clone. See H3. Doing the same *inside* `seatbelt` — e.g. having
  `RetryShared::clone_input` skip the callback entirely on the last attempt — would change
  observable behaviour, because the callback has documented side effects that consumers
  rely on (`http_clone.rs:58-59` mutates the request in place). See the philosophy note.
- **Evidence:** inferred from code reading. Confirmed by: `benches/retry.rs`'s `with-retry`
  scenario (`:33-50`, `recovery_with(|_, _| RecoveryInfo::never())`, so exactly one
  attempt) with `alloc_tracker` already installed at `:16-17` — but only if the benchmark's
  `Input` is changed from a ZST (`benches/retry.rs:80`) to something whose `Clone`
  allocates. As written, the benchmark cannot see this cost at all: `struct Input;` clones
  for free. That is itself a benchmark-coverage gap (see below).
- **Philosophy note:** **conflicting** if implemented inside `seatbelt`. Suppressing the
  `clone_input` callback on the last attempt removes an observable side-effect invocation
  that consumers depend on, which crosses the line from "surgical optimisation" into
  "behaviour change". The version recommended above keeps the change entirely in
  `seatbelt_http`, where the callback's side effects are known, and is not conflicting.

#### F5. Retry builds a backoff-delay iterator on every request, including requests that never retry

- **Location:** `crates/seatbelt/src/retry/service.rs:98`
  (`let mut delays = self.shared.backoff.delays();`); `crates/seatbelt/src/retry/backoff.rs`.
- **Issue:** `delays()` clones `BackoffOptions` into a fresh iterator before the first
  attempt is even launched. No heap allocation is involved (the options are ~40 bytes of
  `Duration` / `f64` / enum), but it is an unconditional several-word copy plus the
  iterator state, and it inflates the retry future by that amount — and the retry future is
  boxed on the `tower-service` path (F13) and may be held inside a hedging
  `FuturesUnordered` node.
- **Impact:** **Low** — a stack copy, not an allocation. Listed for completeness and
  because it is trivially avoidable.
- **Remediation:** make `delays` lazily initialised (`Option<Delays>` filled in the
  `ControlFlow::Continue` arm at `:109`), or move its construction inside that arm since it
  is only consumed by `evaluate_attempt`'s retry branch.
- **Evidence:** inferred from code reading. Confirmed by: `retry_cg.rs`'s `with_retry`
  instruction count (Callgrind resolves tens-of-instructions deltas that Criterion cannot).

#### F6. `TelemetryHelper` is deep-cloned once per hedging attempt, including the primary attempt

- **Location:** `crates/seatbelt/src/hedging/service.rs:266-273` (`create_guard`, the clone
  at `:271`); call sites at `:140` (primary attempt — i.e. **every request**) and `:254`
  (each hedge); the type at `crates/seatbelt/src/utils/telemetry_helper.rs:4-14`;
  `TelemetryString = Cow<'static, str>` at `crates/seatbelt/src/lib.rs:282`.
- **Issue:** `TelemetryHelper` holds two `Cow<'static, str>` (24 bytes each), an
  `Option<opentelemetry::metrics::Counter<u64>>` and a `bool`. Cloning it per attempt
  costs: a ~64-72-byte struct copy; an `Arc` refcount increment on the counter (an atomic
  RMW) plus a matching decrement when the `TelemetryGuard` drops; and — if the pipeline was
  named with an owned `String` rather than a `&'static str` — **two heap allocations per
  attempt**, because `Cow::Owned(String).clone()` allocates. The layer constructors accept
  `impl Into<Cow<'static, str>>` (e.g. `hedging/service.rs:71-76`), so a `String` name is a
  perfectly ordinary, documented usage that silently switches on this cost.
- **Impact:** **Medium** — the atomic pair is unconditional whenever the `metrics` feature
  is compiled in (which `--all-features` benchmark and docs.rs builds always do), and the
  two-allocations-per-attempt case is reachable through the public API without any warning.
  It also enlarges the hedging future by the size of the helper, on a path that is already
  boxed inside `FuturesUnordered` nodes.
- **Remediation (surgical):** store `Arc<TelemetryHelper>` in `HedgingShared` and clone the
  `Arc` into the guard — one atomic instead of one atomic plus two `Cow` clones plus a
  70-byte copy, and it makes the `String`-name case free. Alternatively make
  `TelemetryGuard` borrow the helper, though that requires a lifetime on the guard and may
  fight the `'static` bounds on the launched futures.
- **Evidence:** inferred from code reading (the `Cow`/`Arc` clone semantics are certain;
  the exact size of `opentelemetry::metrics::Counter<u64>` could not be measured offline).
  Confirmed by: a new `hedging` Criterion scenario built with `--features metrics` and a
  pipeline named with an owned `String`, read through the already-installed `alloc_tracker`
  session (`benches/hedging.rs:14-15`). No such scenario exists today — `benches/hedging.rs`
  does not require the `metrics` feature (`Cargo.toml:182-185`), so this cost is currently
  invisible.

#### F7. Metric attribute arrays are rebuilt per emission although they are constant per engine

- **Location:** `crates/seatbelt/src/breaker/engine/engine_telemetry.rs:50-58` (rejection),
  `:79-88` (probe), `:117-125` and onward (`report_state_change`).
- **Issue:** each emission constructs a fresh 5- or 6-element `[KeyValue]` array in which
  four of the five entries are invariant for the lifetime of the engine:
  `PIPELINE_NAME → self.telemetry.pipeline_name.clone()`,
  `STRATEGY_NAME → self.telemetry.strategy_name.clone()`,
  `EVENT_NAME → <const>` and `CIRCUIT_ID → self.breaker_id.clone()`. Only
  `CIRCUIT_STATE` / `CIRCUIT_PROBE_RESULT` vary, and those vary over a tiny fixed set. As
  in F6, the `.clone()` on a `Cow::Owned` name allocates.
- **Impact:** **Medium**, and specifically on the *worst possible* path: the rejection
  emission at `:48-58` fires on **every rejected request while the circuit is open** — that
  is, during an outage, when the service is already under stress and the whole point of the
  breaker is to shed load cheaply. Rebuilding a 5-element `KeyValue` array (each `KeyValue`
  is a `Key` + `Value` enum, ~48 bytes) with two potential `String` allocations per
  rejection is exactly backwards.
- **Remediation (surgical):** precompute the invariant attribute sets in
  `EngineTelemetry::new` (`:29-36`) — one `Box<[KeyValue]>` per (event, state) pair, or a
  small array indexed by a `CircuitState` discriminant — and pass a slice to
  `report_metrics`. The engine already owns everything needed at construction time. The
  same pattern applies to the equivalent emission sites in `chaos/injection/service.rs:198+`
  and the other strategies.
- **Evidence:** inferred from code reading. The `Cow::Owned` allocation claim is certain;
  whether `KeyValue::new(&'static str, Cow<'static, str>)` itself allocates depends on
  `opentelemetry`'s `StringValue` representation (believed to be a
  `Static(&'static str) | Owned(Box<str>) | RefCounted(Arc<str>)` enum, in which case a
  `Cow::Borrowed` is free) and could not be checked offline. Confirmed by: **there is no
  benchmark for the open-circuit rejection path at all** — `benches/breaker.rs:38` and
  `breaker_cg.rs:65` both set `.min_throughput(1000)` with the explicit comment "High
  threshold to keep circuit closed". A `rejected` scenario (trip the circuit in `setup`,
  measure the rejection) is the single biggest gap in the breaker suite.

#### F8. `ProbesOptions::probes()` heap-clones a `Vec` while the engine mutex is held

- **Location:** `crates/seatbelt/src/breaker/engine/probing/options.rs:83-85`
  (`self.probes.clone().into_iter()`); consumed by
  `crates/seatbelt/src/breaker/engine/probing/probes.rs:18-26`; called from
  `State::enter`'s `Open → HalfOpen` transition at
  `crates/seatbelt/src/breaker/engine/engine_core.rs:65`, which runs **inside**
  `self.state.lock()` (`engine_core.rs:39`).
- **Issue:** the `Vec<ProbeOptions>` is cloned (heap allocation + element copy;
  `ProbeOptions` is 40 bytes and progressive probing configures up to 7 of them) purely to
  obtain an owned iterator, and the allocation happens with the engine's mutex held. The
  file's own comment at `engine_core.rs:38` says "Remember to execute all expensive
  operations (like time checks) outside the lock" — an allocator call is strictly more
  expensive than the clock read that comment was written about.
- **Impact:** **Medium** — the transition itself is rare, but it occurs precisely when the
  circuit is recovering, i.e. when the mutex is hottest (every in-flight request is queued
  behind `enter()`/`exit()`). Holding the lock across a `malloc` that can block on the
  allocator's own lock lengthens the critical section by an unbounded amount.
- **Remediation (surgical):** store the probe list as `Arc<[ProbeOptions]>` in
  `ProbesOptions` and have `Probes` hold `(Arc<[ProbeOptions]>, usize)` — an `Arc` clone
  plus an index instead of a `Vec` clone, no allocation, and `ProbeOptions` need no longer
  be `Clone`. Alternatively keep the `Vec` but build the `Probes` value before taking the
  lock (harder, because the decision to transition is made under the lock).
- **Evidence:** inferred from code reading. Confirmed by: a new breaker Criterion/Callgrind
  scenario that forces an `Open → HalfOpen` transition (currently unbenchmarked — the whole
  recovery path is), with `alloc_tracker` counting allocations across the transition.
#### F9. `Engines::get_engine` clones the `BreakerId` on the insert path while holding the write lock

- **Location:** `crates/seatbelt/src/breaker/engine/engines.rs:54-60`, specifically
  `map.entry(key.clone())` at `:57` and the `create_engine` call at `:58`.
- **Issue:** the write-lock arm clones the `BreakerId` (a `Cow<'static, str>`, so a `String`
  allocation for a request-derived ID) *unconditionally* — `BTreeMap::entry` takes the key
  by value, so the clone happens even when the entry already exists (the double-checked
  case after a race). It then builds a whole `Engine` — including the 264-byte `State`
  (F2), a `HealthMetrics` with its `VecDeque`, and the cloned `TelemetryHelper` at `:73` —
  inside `or_insert_with`, with the write lock held and all readers blocked.
- **Impact:** **Low-Medium** — this is a first-insert cost, which house guidance explicitly
  deprioritises, and `get_engine` already has both a lock-free default path (`:39-41`) and
  a read-lock hit path (`:47-52`). It matters only for high-cardinality partitioned
  breakers under a cold start or a partition-churn burst, which the docs discourage anyway.
  It is recorded because `benches/breaker_cg.rs:118-120` deliberately benchmarks exactly
  this (`with_partitioned_miss`), so the project evidently considers it in scope.
- **Remediation:** use `if let Some(e) = map.get(key) { return Arc::clone(e) }` before the
  `entry` call to avoid the clone on the raced-insert path, and construct the `Engine`
  before taking the write lock (discarding it on a race). Both are small; neither is
  urgent.
- **Evidence:** inferred from code reading. Confirmed by: the existing
  `breaker_cg.rs` `with_partitioned_miss` benchmark, which already isolates this path.

#### F10. The crate has 2 `#[inline]` attributes across 128 public functions

- **Location:** workspace-wide; the only two are
  `crates/seatbelt/src/retry/backoff.rs:115` and `:147`. Counted with
  `grep -rn '#\[inline' crates/seatbelt/src` (2 hits) versus `grep -rn 'pub fn '` (128
  hits).
- **Issue:** `docs/performance.md` rule 1 says `#[inline]` should be applied to
  non-generic exported functions on hot paths, because without it a downstream crate
  cannot inline them across the crate boundary in a normal (non-LTO) build. Many of
  seatbelt's smallest and hottest public accessors have no attribute — e.g. `Attempt`'s
  accessors (`crates/seatbelt/src/attempt.rs`), `RecoveryInfo` constructors, `BreakerId`'s
  `is_default`, and the `*Args` accessors that every user callback calls on every request.
  Note that a large fraction of the crate's surface *is* generic (the layer builders and
  `Service` impls are monomorphised into the consumer anyway), so the true gap is smaller
  than 126 — but it is not 0.
- **Impact:** **Medium** — invisible in this repo's own measurements and therefore likely to
  stay invisible, which is the real problem. See F-bench-1 below: `[profile.bench]` sets
  `lto = "fat"` while `[profile.release]` sets neither `lto` nor `codegen-units`
  (`Cargo.toml:340-346`). Fat LTO inlines across crate boundaries regardless of the
  attribute, so **every benchmark number in this repository is produced by a build in which
  missing `#[inline]` cannot hurt, while every consumer gets a build in which it can.**
- **Remediation:** audit the small, non-generic, public, per-request-path functions —
  primarily the `Attempt`, `RecoveryInfo`, `BreakerId` and `*Args` accessors — and add
  `#[inline]`. Do *not* blanket-apply it; the house guidance is explicit about being
  judicious, and adding it to large or cold functions costs compile time and code size for
  nothing.
- **Evidence:** **empirically verified (grep census: 2 `#[inline]`, 128 `pub fn` in
  `crates/seatbelt/src`; 0 `#[inline]`, 5 `pub fn` in `crates/seatbelt_http/src`)**; the
  performance consequence is inferred. Confirmed by: running any existing `*_cg.rs`
  benchmark with `lto = "off"` in `[profile.bench]` and diffing instruction counts against
  the fat-LTO baseline. That diff is the size of the crate-boundary inlining the repo is
  currently hiding from itself.

#### F11. Chaos strategies draw a random number unconditionally, even when the injection rate is zero

- **Location:** `crates/seatbelt/src/chaos/injection/service.rs:190-193`; the same shape in
  `crates/seatbelt/src/chaos/latency/service.rs`.
- **Issue:** `should_inject` evaluates the (user-supplied, `Arc<dyn Fn>` — one indirect
  call) rate closure, clamps it, and then *unconditionally* calls `self.rnd.next_f64()`
  before comparing. When the rate is `0.0` the comparison `x < 0.0` can never be true for a
  non-negative `x`, so the RNG draw is provably dead work.
- **Impact:** **Low-Medium** — but note the deployment pattern this crate is designed for:
  chaos layers are left permanently in the production pipeline and switched *off* by
  configuration (rate 0), so that they can be switched on for an experiment without a
  redeploy. In that steady state, every single request pays a `fastrand` draw (a wyrand
  step plus a `u64→f64` conversion) for a comparison whose answer is known.
- **Remediation (surgical):** `if rate <= 0.0 { return false; }` before the draw. Two
  lines; no behaviour change (`rnd` is not a shared sequence whose consumption anyone
  observes — `Rnd::Real` delegates to `fastrand`'s thread-local generator).
- **Evidence:** inferred from code reading. Confirmed by: **there is no benchmark for
  `chaos-injection` or `chaos-latency` at all** (`Cargo.toml:162-210` lists five bench
  pairs: observability, timeout, retry, breaker, hedging). A `chaos-injection` bench with
  `rate = 0.0` versus a bare service is the missing measurement.

#### F12. Exponential backoff uses `f64::powi`

- **Location:** `crates/seatbelt/src/retry/backoff.rs:105-108` (`duration_mul_pow2`).
- **Issue:** the backoff multiplier is computed with a floating-point power function where
  the exponent is a small integer attempt index. `powi` on a small integer exponent is
  cheap-ish but is still a libm-adjacent call; a shift or a small lookup table would be
  exact and branch-free.
- **Impact:** **Low** — this is on the retry-delay path, which by construction only runs
  when a retry is actually happening, and is immediately followed by an `await` on a delay
  measured in milliseconds. Recorded for completeness only; optimising it would be
  micro-optimising a path dominated by a sleep. Note this function already carries one of
  the crate's two `#[inline]` attributes (`:115`), so it has evidently had attention.
- **Remediation:** none recommended. If touched at all, replace with an integer shift for
  the power-of-two case.
- **Evidence:** inferred from code reading. Confirmed by: `retry_cg.rs`'s
  `with_retry_and_recovery` instruction count.

#### F13. The entire `tower-service` surface boxes its future on every call

- **Location:** `crates/seatbelt/src/breaker/service.rs:152,162,176`;
  `crates/seatbelt/src/timeout/service.rs:167,175`;
  `crates/seatbelt/src/chaos/injection/service.rs:183-185`;
  `crates/seatbelt/src/hedging/service.rs:276-280` (`HedgingFuture` is literally
  `Pin<Box<dyn Future + Send>>`); the equivalent in `fallback` and `chaos/latency`.
- **Issue:** `tower_service::Service` has an associated `Future` type that must be
  nameable, and `async fn` produces an unnameable opaque type, so every strategy wraps its
  body in `Box::pin`. That is one heap allocation and one layer of dynamic dispatch per
  request **per layer in the stack** — a `retry + breaker + timeout` pipeline used through
  `tower` pays three.
- **Impact:** **Medium** for `tower` consumers, **zero** for `layered` consumers (the
  `Execute`/`Service` path is fully monomorphised and allocation-free).
- **Remediation:** none that is both surgical and safe. The idiomatic fix is a named future
  type per strategy implementing `Future` by hand — that is an architectural rewrite of
  every strategy, it duplicates the `execute` logic, and `crates/seatbelt/AGENTS.md`
  contains an explicit crate-local rule that the `layered` and `tower` `Service` impls must
  stay logic-equivalent, which hand-rolled state machines would immediately jeopardise. A
  future `impl Trait` in associated position would remove the problem for free; until then
  this is a documented cost of using the `tower-service` feature, and consumers who care
  should use the `layered` path.
- **Evidence:** inferred from code reading. Confirmed by: **the `tower_service` path is not
  benchmarked anywhere** — all five Criterion files and all five Callgrind files use
  `layered::Execute` / `Stack::into_service`. A `tower`-flavoured variant of
  `benches/retry.rs` would quantify the per-layer boxing cost.
- **Philosophy note:** **conflicting** — the only real fix is architectural, is explicitly
  discouraged by `docs/performance.md`'s "surgical over architectural" principle, and would
  break the crate-local logic-equivalence rule in `crates/seatbelt/AGENTS.md`. Reported for
  completeness; **not recommended**.

#### F14. `ExitCircuitResult` is a 64-byte return value produced on every request

- **Location:** `crates/seatbelt/src/breaker/engine/engine_core.rs:42-47` (`exit` returns
  it); the type in `crates/seatbelt/src/breaker/engine/mod.rs`; the `Stats` payload at
  `engine_core.rs:176-187`.
- **Issue:** `ExitCircuitResult::Closed(Stats)` forces the enum to 64 bytes (measured),
  because `Stats` is 64 bytes on its own and the niche allows the discriminant to be
  folded in. Every request's `exit()` therefore returns through a 64-byte stack slot, even
  though the steady-state answer is the payload-free `Unchanged`. The wrapper
  `EngineTelemetry::exit` (`engine_telemetry.rs:104-108`) passes it through by value and
  then borrows it for `report_state_change`, so it is materialised twice.
- **Impact:** **Low** — return-slot writes for the `Unchanged` discriminant are a single
  store; the 64-byte slot costs stack space, not copies. Crucially it does *not* inflate
  any async future, because `exit()` is called from synchronous code with no `await` across
  it. Recorded because it is a real layout smell and cheap to fix if the area is touched
  for F1 or F2 anyway.
- **Remediation:** `Closed(Box<Stats>)`, allocating only on the rare `HalfOpen → Closed`
  transition, shrinks `ExitCircuitResult` to 16 bytes. Given the Low impact, do this only
  as a rider on F2, not on its own.
- **Evidence:** **empirically verified (rustc layout replica: `ExitCircuitResult` = 64
  bytes / align 8, `Stats` = 64 / 8, `EnterCircuitResult` = 1 / 1)**; the cost of the
  64-byte slot is inferred. Confirmed by: `breaker_cg.rs` `with_breaker` instruction and
  branch counts.

### Benchmark coverage

**What exists.** Five Criterion files and five paired Callgrind files, declared at
`crates/seatbelt/Cargo.toml:162-210`:

| area | Criterion | Callgrind | scenarios |
|---|---|---|---|
| breaker | `benches/breaker.rs` | `benches/breaker_cg.rs` | no-breaker, with-breaker (closed), with-partitioned {1, 16, 256}; `_cg` adds `with_partitioned_miss` |
| hedging | `benches/hedging.rs` | `benches/hedging_cg.rs` | no-hedging, with-hedging-delay, with-hedging-passthrough |
| retry | `benches/retry.rs` | `benches/retry_cg.rs` | no-retry, with-retry, with-retry-and-recovery |
| timeout | `benches/timeout.rs` | `benches/timeout_cg.rs` | no-timeout, with-timeout |
| observability | `benches/observability.rs` | `benches/observability_cg.rs` | logs + metrics enabled |

The suite follows `docs/naming.md` correctly: every Callgrind file is `<criterion>_cg.rs`,
group names match the file stem, and each has a baseline (`no-*`) scenario so the delta
attributable to the strategy is isolable. Every Criterion file installs
`alloc_tracker::Allocator` as `#[global_allocator]` and wraps each measured loop in an
`operation.measure_thread().iterations(iters)` span, so allocation counts are already being
captured — good practice, and it means several findings above (F3 in particular) could be
confirmed from data the suite already produces. `breaker_cg.rs:114-120` carries a
thoughtful comment explaining why the partition-miss case exists only in Callgrind (a
Criterion loop would insert once and hit thereafter). The `_cg` files correctly gate on
`target_os = "linux"` and enable `--branch-sim=yes`.

**What is missing.**

1. **No multi-threaded scenario anywhere in the crate.** Every benchmark is
   `block_on(service.execute(...))` on one thread. The breaker's shared `Mutex<State>`
   (F1), the `RwLock<BTreeMap>` in `Engines` (`engines.rs:17`), and the `Arc` refcount
   traffic in F6 are all *contention* costs that are structurally invisible to a
   single-threaded harness. This is the single largest gap: the crate's most important
   performance property cannot be observed by its own benchmarks.
2. **The open-circuit rejection path is unbenchmarked.** `benches/breaker.rs:38` and
   `breaker_cg.rs:65` both set `.min_throughput(1000)` with the comment "High threshold to
   keep circuit closed". So the load-shedding path — the one that runs during an outage,
   at the highest request rate the system will ever see, and that emits a five-attribute
   metric per rejection (F7) — has no measurement at all.
3. **The recovery path is unbenchmarked.** No scenario drives `Open → HalfOpen → Closed`,
   so `Probes`, `HealthProbe`, `SingleProbe`, `ProbesOptions::probes()` (F8) and the whole
   `probing/` module are unmeasured.
4. **Partitioned breaker benches key on `u64`, the real consumer keys on `String`.**
   `benches/breaker.rs:92` and `breaker_cg.rs:78` use `BreakerId::from(input.0)` where
   `input.0: u64`. The actual default consumer is
   `seatbelt_http`'s `create_breaker_id` (`crates/seatbelt_http/src/breaker.rs:154-159`),
   which produces a freshly formatted `String`. The benchmark therefore measures neither
   the per-request formatting (H1) nor the `BTreeMap` walk of *string* comparisons that a
   real deployment performs — integer comparisons are dramatically cheaper and have no
   pointer-chasing. This makes `with-partitioned-large` materially optimistic.
5. **Retry and hedging benchmark a ZST input.** `benches/retry.rs:80` (`struct Input;`) and
   `benches/hedging.rs:76-77` clone for free, so the per-attempt clone cost — the dominant
   cost for the real HTTP consumer (F4, H1) — is measured as zero. A second scenario with
   an input whose `Clone` allocates would make F4 visible.
6. **Three strategies have no benchmarks at all:** `fallback`, `chaos-injection`,
   `chaos-latency`. All three are declared features (`Cargo.toml:52-55`) with examples
   (`:146-160`) but no `[[bench]]` entry.
7. **The `tower-service` path has no benchmarks.** Every bench uses
   `layered::Execute`/`Stack`, so the per-layer `Box::pin` cost (F13) is never measured,
   despite `tower-service` being a first-class feature with its own example
   (`Cargo.toml:130-132`).
8. **`benches/hedging.rs` does not enable `metrics`.** `Cargo.toml:182-185` requires only
   `hedging`, so the per-attempt `TelemetryHelper` clone (F6) is compiled out of the
   benchmark entirely. Only `observability` enables `logs` + `metrics`, and only for
   `retry`.
9. **All benchmarks use `Clock::new_frozen()`**, which is correct for determinism but means
   the unconditional `Clock::instant()` in `EngineCore::enter` (F1) is measured as a
   frozen-clock read rather than a real `clock_gettime`. The real cost is higher than the
   benchmarks report.

**Profile concern (cross-cutting, confirmed here).** Root `Cargo.toml:340-346`:
`[profile.release]` sets only `debug = "line-tables-only"` — no `lto`, no
`codegen-units`. `[profile.bench]` sets `lto = "fat"` and `codegen-units = 1`. Every
published benchmark number therefore comes from a build configuration no consumer of the
crate receives, and, worse, fat LTO makes the suite structurally blind to F10 (missing
`#[inline]`), because cross-crate inlining happens anyway. Recommendation: either align
`[profile.release]` with `[profile.bench]`, or add a second benchmark profile without LTO
so the crate-boundary inlining gap is visible.

### Considered and ruled out

- **`EnableIf` dynamic dispatch** (`crates/seatbelt/src/utils/mod.rs:24-53`) — it is an
  enum with `Enabled` / `Disabled` / `Custom(Arc<dyn Fn>)` variants, so the default
  configuration costs a predictable branch, not a virtual call. Good design; nothing to do.
- **`HealthMetrics::health_info` re-summing the sliding window** — it does not.
  `health.rs:110-113` keeps a running `ExecutionInfo` aggregate updated as windows are
  added and evicted, with a comment saying exactly why. Already optimal.
- **`Engines::get_engine` using `BTreeMap` instead of `HashMap`** — deliberate, and the
  comment at `engines.rs:43-46` justifies it: partitioned breakers are low-cardinality by
  design, and a `BTreeMap` is not exposed to hash-flooding via request-derived IDs. That is
  a security-vs-speed trade the house guidance ("preserve defensive runtime checks")
  supports. Not a finding. (The *string* comparison cost is real, but the fix belongs in
  `seatbelt_http` — see H1 — not in swapping the map type.)
- **`Rnd`** (`crates/seatbelt/src/rnd.rs:15-22`) — a unit enum outside `cfg(test)`, so it
  is a ZST and `Rnd::Real` compiles to a direct `fastrand` call. Zero cost.
- **`FallbackAction`** (`crates/seatbelt/src/fallback/callbacks.rs:17-45`) — already avoids
  boxing for the synchronous case. Nothing to do.
- **`Arc<dyn Fn>` user callbacks** (`utils/define_fn_wrapper.rs:29,95`) — one indirect call
  per configured callback per request. This is inherent to a configurable resilience
  pipeline and matches the ecosystem pattern (`tower`, `tokio-retry`, Polly). Not a finding.
- **`expect(ERR_POISONED_LOCK)` on every lock acquisition** (`engine_core.rs:39,46`,
  `engines.rs:48,55`) — a defensive check the house guidance explicitly says to preserve;
  the branch is perfectly predicted. Not a finding.
- **The assertions in `ProbesOptions::new` / `HealthProbeOptions::new`**
  (`probing/options.rs:79`, `:96-98`) — configuration-time only, and they are precisely the
  kind of defensive check `docs/performance.md` says not to remove.
- **`Attempt` size** — measured at 8 bytes; passed by value throughout. Fine.

---
## Crate: seatbelt_http

### Summary

`seatbelt_http` is a thin adapter: five public functions, four feature-gated modules, and
no benchmarks. Its performance significance is out of all proportion to its size, because
every one of its `http_configure_defaults()` helpers installs a callback that runs on
**every request** in the pipeline it configures, and those callbacks are the ones the crate
documentation tells users to reach for first.

The two dominant findings are H1 (the breaker ID is formatted into a fresh `String` per
request, which additionally defeats `seatbelt`'s lock-free default-engine fast path) and
H2 (the default clone strategy performs a deep `HttpRequest` clone per request, including
on attempts that can never be retried). H3 is the surgical fix that resolves the wasted
half of H2 and simultaneously unlocks `seatbelt`'s hedging early-return (F3).

Note a correction to a claim carried over from an earlier analysis round: `create_breaker_id`
is **not** the only per-request heap allocation reachable from these crates. H1 is one
allocation; H2 is three or more, and much larger ones. H1 is the more *structurally*
damaging of the two (it changes which code path the breaker takes), but H2 is the bigger
byte count.

### Findings

#### H1. The default breaker ID formats a fresh `String` per request — and thereby defeats seatbelt's lock-free fast path

- **Location:** `crates/seatbelt_http/src/breaker.rs:154-159` (`create_breaker_id`);
  installed by `http_configure_defaults` at `:119-123`, specifically the
  `.breaker_id(|req: &HttpRequest| create_breaker_id(req.uri()))` on line `:122`.
- **Issue:** two compounding costs.
  - **(a) Allocation.** `Origin::from_parts(scheme.clone(), authority.clone()).to_string().into()`
    (`:156`) clones two refcounted `http` types (cheap — `Scheme` and `Authority` are
    `Bytes`-backed), formats them into a brand-new `String` via `Display`, and then converts
    that into a `BreakerId` (a `Cow<'static, str>`, so it stays `Owned`). That is one heap
    allocation plus a formatting machine invocation **per request**, producing a string that
    is byte-identical to the one produced by the previous request in the overwhelmingly
    common single-origin case.
  - **(b) Fast-path defeat.** Because a `BreakerId` built this way is never
    `is_default()`, `Engines::get_engine` (`crates/seatbelt/src/breaker/engine/engines.rs:35-61`)
    skips its lock-free branch at `:39-41` and instead takes `self.map.read()` (`:48`) and
    walks a `BTreeMap` whose keys are strings — i.e. a sequence of `str` comparisons, each
    a pointer dereference into a separately-allocated buffer — followed by an `Arc::clone`
    (`:50`, an atomic RMW). Note the asymmetry this creates: a user who configures no ID
    provider at all gets `BreakerShared::id_provider == None`
    (`crates/seatbelt/src/breaker/service.rs:48`, `get_breaker_id` at `:186-190`) and the
    lock-free path; a user who follows the documented `http_configure_defaults()`
    recommendation loses it.
- **Impact:** **High** — it is per-request, it is on the path the crate's own documentation
  recommends, and it converts an `Arc::clone` of a pre-created engine into
  `format!` + `RwLock::read` + a string-keyed tree walk + `Arc::clone`. On a service that
  talks to a single backend origin — which is most services — 100% of that work produces
  the same answer every time.
- **Remediation (surgical):** add a `BreakerId` constructor that stores `http`'s
  refcounted `Scheme` and `Authority` (or their `Bytes` buffers) instead of a formatted
  `String`, so cloning an ID is a refcount bump rather than an allocation. Failing that,
  memoise per connection/client: the origin is fixed for the lifetime of most clients, so
  the ID can be computed once and stored in the layer or in a request extension. A third,
  even smaller option: keep the formatting but short-circuit to `BreakerId::default()` when
  the pipeline is known to be single-origin, restoring the lock-free path.
- **Evidence:** inferred from code reading (line references verified against the current
  source). Confirmed by: a new `crates/seatbelt_http/benches/breaker.rs` with an
  `alloc_tracker` global allocator, comparing `http_configure_defaults()` against a
  breaker with no ID provider. **`seatbelt_http` has no `benches/` directory, no `[[bench]]`
  entry, and no `criterion` / `gungraun` / `alloc_tracker` dev-dependency**
  (`crates/seatbelt_http/Cargo.toml:47-53`), so nothing in this crate is measured today.
  Additionally, `crates/seatbelt/benches/breaker.rs:92` keys its partitioned scenarios on
  `u64`, so even the `seatbelt`-side cost of string-keyed lookup is unmeasured.

#### H2. The default clone strategy deep-clones the whole `HttpRequest` per request

- **Location:** `crates/seatbelt_http/src/http_clone.rs:43-65` (`HttpClone::try_clone`),
  specifically `request.try_clone()` at `:50`; installed by
  `HttpRetryLayerExt::http_configure_defaults` (`crates/seatbelt_http/src/retry.rs:114-118`,
  via `http_clone` at `:120-122`) and `HttpHedgingLayerExt::http_configure_defaults`
  (`crates/seatbelt_http/src/hedging.rs:100-102`, via `:104-106`). The underlying clone is
  `crates/http_extensions/src/extensions/http_request_ext.rs:21-31`.
- **Issue:** `HttpRequest::try_clone` clones the body, clones the `HeaderMap`
  (one heap allocation plus per-header value clones), and clones the `Extensions` map
  (a boxed `AnyMap` allocation, with every stored value re-boxed — one allocation each).
  For a typical request with a handful of headers and two or three extensions this is
  comfortably 4-8 heap allocations and a few hundred bytes copied, **per request**, on both
  the default retry path and the default hedging path.
- **Impact:** **High** — it is unconditional on the recommended configuration, and by far
  the largest per-request cost in either crate. It is not *wasted* when a retry or hedge
  actually happens (the clone is genuinely needed), which is why the finding is really H3:
  the waste is confined to the attempts that can never be retried.
- **Remediation:** the clone itself is necessary for multi-attempt configurations and
  cannot be removed without changing semantics. What can be removed is the clone on the
  last attempt (H3) and the `Extensions` re-boxing (which belongs to `http_extensions`, and
  is that group's scope, not mine). Consider also documenting the cost on
  `http_configure_defaults` so users configuring a single attempt know to pass
  `HttpClone` variants or skip the layer.
- **Evidence:** inferred from code reading (`http_request_ext.rs:21-31` read directly).
  Confirmed by: a new `seatbelt_http` Criterion benchmark with `alloc_tracker`, comparing a
  `http_configure_defaults()` retry pipeline against a bare service, over a realistic
  request (5-10 headers, 2 extensions). None exists.

#### H3. The clone on the final attempt is always discarded — and skipping it also unlocks seatbelt's hedging early-return

- **Location:** `crates/seatbelt_http/src/retry.rs:120-122` and
  `crates/seatbelt_http/src/hedging.rs:104-106` (the two `http_clone` installers);
  the consuming logic at `crates/seatbelt/src/retry/service.rs:102-116` and
  `crates/seatbelt/src/hedging/service.rs:130-142`.
- **Issue:** both installers call `clone_strategy.try_clone(request, args.attempt(), ...)`
  unconditionally. On the last attempt the retry loop always breaks
  (`retry/service.rs:108-116`), so the retained original is dropped unused — see F4. With
  a single-attempt configuration, *every* request pays the full H2 clone for nothing.
  Symmetrically, in hedging, `run_hedging` computes
  `total_attempts = max_hedged_attempts + 1` (`hedging/service.rs:130`) and marks the
  primary attempt `is_last` when `total_attempts == 1` (`:131`); if `clone_input` returned
  `None` in that case, the early-return at `:134-137` would fire and the whole
  `FuturesUnordered` construction (F3) would be skipped.
- **Impact:** **High** — a two-line change in each installer that removes 4-8 heap
  allocations plus a `HeaderMap` and `AnyMap` copy from every request on a single-attempt
  pipeline, and additionally removes ≥3 more allocations from every request on a
  hedging-disabled pipeline.
- **Remediation (surgical, and it belongs here rather than in `seatbelt`):** in
  `HttpClone::try_clone` (`http_clone.rs:43-65`), skip the `request.try_clone()` at `:50`
  when `attempt.is_last()` — but **still apply** the `attach_attempt` (`:58`) and
  `update_request_uri` (`:59`) side effects to the original, which the existing code
  already does via `result.as_mut().unwrap_or(request)`, so the shape of the function
  barely changes. Return `None`. Both `seatbelt::retry` and `seatbelt::hedging` already
  treat `None` as "no clone available" and handle it correctly, so observable behaviour is
  preserved. Care needed: `update_request_uri` returning `false` must still return `None`
  (it already does, `:60-61`), and `retry`'s `restore_input_from_error`
  (`retry.rs:130-132`) path must be checked to confirm it does not depend on the retained
  original — it takes the request back out of the error, so it does not.
- **Evidence:** inferred from code reading. Confirmed by: a `seatbelt_http` Criterion
  benchmark (does not exist) with `alloc_tracker`, scenario "retry with max_attempts=1",
  before and after. On the `seatbelt` side, `benches/hedging.rs`'s existing
  `with-hedging-passthrough` scenario (`:50-68`) already isolates the hedging half of this,
  but its `Input` is a ZST (`:76-77`), so it currently shows nothing.
- **Philosophy note:** **conflicting only in its rejected variant.** Implementing this
  inside `seatbelt` — by having `RetryShared::clone_input` not invoke the user callback at
  all on the last attempt — would suppress documented side effects and is a behaviour
  change; that variant should not be pursued. The version described above is confined to
  `seatbelt_http`, where the callback's side effects are known and are preserved
  explicitly, and is not conflicting.

#### H4. `attach_attempt` forces an `Extensions` allocation on requests that have none

- **Location:** `crates/seatbelt_http/src/http_clone.rs:106-108`; called from `try_clone`
  at `:58`.
- **Issue:** `request.extensions_mut().insert(attempt)` runs on every request through a
  default-configured retry or hedging layer. `http::Extensions` is lazily allocated — the
  first `insert` on an empty `Extensions` allocates the boxed `AnyMap`. So a request that
  carries no extensions of its own is charged one allocation purely so the attempt index
  can be attached, even when there will only ever be one attempt.
- **Impact:** **Low-Medium** — one allocation, and only for requests that would otherwise
  have an empty extensions map (many clients populate extensions anyway, and if they do,
  this is just a hash insert). Combined with H3 it disappears for the single-attempt case
  only if `attach_attempt` is also gated, which it should *not* be — the attempt index is
  observable by downstream layers and by `update_request_uri` at `:84-95`.
- **Remediation:** none that is clearly correct. The insert is semantically required. If
  measurement shows it matters, the attempt could be carried out-of-band (e.g. in the
  seatbelt `*Args` already passed to callbacks) rather than in the request extensions, but
  that is a public-behaviour change for downstream consumers that read the extension.
  Recorded for visibility, not for action.
- **Evidence:** inferred from code reading; the lazy-allocation behaviour of
  `http::Extensions` could not be verified offline. Confirmed by: the same
  `alloc_tracker`-instrumented `seatbelt_http` benchmark as H2, with and without a
  pre-populated extensions map on the input request.

#### H5. `update_request_uri` performs a typed extension lookup on every request

- **Location:** `crates/seatbelt_http/src/http_clone.rs:78-96`, specifically the
  `request.extensions().get::<Router>()` at `:84`; called unconditionally from `try_clone`
  at `:59`.
- **Issue:** every request through a default retry/hedging layer does a `TypeId`-keyed hash
  lookup in the extensions map to discover whether a `Router` is present, even though the
  match at `:85` immediately rejects the result unless the router has alternatives *and*
  the attempt is not the first — i.e. the lookup can only ever matter on attempt ≥ 2.
- **Impact:** **Low** — a hash lookup with a `TypeId` key is fast and the map is small. But
  it is unconditional, it is on the first attempt of every request, and the guard that
  makes it useless (`!attempt.is_first()`) is available *before* the lookup at zero cost.
- **Remediation (surgical):** reorder the condition so the cheap `attempt.is_first()` test
  short-circuits before the extension lookup:
  `if attempt.is_first() { return true; }` at the top of `update_request_uri`. This is a
  pure reordering with identical semantics — the existing match at `:85-86` already returns
  `true` in that case.
- **Evidence:** inferred from code reading. Confirmed by: a Callgrind (`gungraun`)
  benchmark of the `seatbelt_http` retry path; none exists, and the crate has no
  `gungraun` dev-dependency (`Cargo.toml:47-53`).

#### H6. `seatbelt_http` has no benchmarks of any kind

- **Location:** `crates/seatbelt_http/Cargo.toml` — no `[[bench]]` section (contrast
  `crates/seatbelt/Cargo.toml:162-210`), no `benches/` directory, and dev-dependencies
  (`:47-53`) containing no `criterion`, no `gungraun`, no `alloc_tracker`, no
  `benchmarking`.
- **Issue:** the crate whose default configuration installs the most expensive per-request
  callbacks in the whole `seatbelt` family (H1, H2) has zero performance measurement. Every
  finding in this section is consequently unconfirmable in-repo, and any regression in
  `http_configure_defaults()` would ship silently. This also means the *sibling* crate's
  benchmarks are measuring the wrong shape: `crates/seatbelt/benches/breaker.rs:92` keys on
  `u64`, `benches/retry.rs:80` clones a ZST — both dramatically cheaper than what
  `seatbelt_http` actually does.
- **Impact:** **High** (as a process finding).
- **Remediation:** add `crates/seatbelt_http/benches/{breaker,retry,hedging}.rs` plus
  `*_cg.rs` pairs, following `docs/naming.md` and the shape of the existing `seatbelt`
  benches (`alloc_tracker` global allocator, `benchmarking::time_sample`, a `no-*`
  baseline). Priority order: (1) `http_configure_defaults()` breaker vs. a no-ID-provider
  breaker, on a single origin — measures H1; (2) `http_configure_defaults()` retry with
  `max_attempts = 1` vs. a bare service, over a realistic request — measures H2 and H3;
  (3) hedging with `max_hedged_attempts = 0` — measures H3's hedging half and F3.
- **Evidence:** **empirically verified (manifest and directory inspection)**.

#### H7. The crate declares no `default` feature, so `cargo add seatbelt_http` yields an empty library

- **Location:** `crates/seatbelt_http/Cargo.toml:34-38` — the `[features]` table lists
  `timeout`, `retry`, `hedging`, `breaker` and no `default` key at all; the modules are
  gated in `crates/seatbelt_http/src/lib.rs:52-72`. (`crates/seatbelt` at least states
  `default = []` explicitly, `Cargo.toml:48`.)
- **Issue:** a user who adds the crate without features compiles a library with no public
  items. This is not a runtime performance problem — and in fact the *absence* of heavy
  default features is exactly what `docs/performance.md` would want, so the direction is
  right — but the omission of an explicit `default = []` makes the intent ambiguous to
  readers and to tooling.
- **Impact:** **Low** — a build/ergonomics observation, included because "perf-relevant
  feature flags and heavy default features" is in scope and the answer here is "there are
  none, deliberately". Worth recording as a positive with a one-line nit.
- **Remediation:** add an explicit `default = []` for symmetry with `seatbelt` and to
  document the intent. No functional change.
- **Evidence:** **empirically verified (manifest inspection)**.

#### H8. Feature-gated `Box`-free design is preserved, but every default helper installs an `Arc<dyn Fn>` indirection

- **Location:** `crates/seatbelt_http/src/http_recovery.rs:91-96` (`CustomDelegate =
  Arc<dyn Fn(&HttpResponse, &Clock) -> RecoveryInfo + Send + Sync>`, `Inner::Default |
  Inner::Custom`); `HttpRecovery::recovery` at `:64-69`; the closures installed by
  `http_recovery` in `retry.rs:124-128`, `hedging.rs:108-112` and `breaker.rs:125-129`.
- **Issue:** `HttpRecovery` itself is well designed — `Inner::Default` is a plain enum
  variant, so the *default* recovery classification is a direct call to
  `response.recovery_with_clock(clock)` (`:66`) with no virtual dispatch. However, the
  `http_recovery` installers wrap it in a `move |out, args| detect_recovery(...)` closure
  which `seatbelt` stores as an `Arc<dyn Fn>` (`crates/seatbelt/src/utils/define_fn_wrapper.rs:29,95`),
  so there is still exactly one indirect call per request per strategy — unavoidable given
  the callback-configured design.
- **Impact:** **Low** — one predictable indirect call per strategy per request; matches the
  ecosystem pattern for configurable middleware.
- **Remediation:** none recommended. Recorded to close out the "avoidable dynamic dispatch"
  question for this crate: it is present, it is one level deep, and it is inherent.
- **Evidence:** inferred from code reading.

### Benchmark coverage

**None.** See H6. `crates/seatbelt_http` has no `benches/` directory, no `[[bench]]`
manifest entries, and no benchmarking dev-dependencies (`Cargo.toml:47-53`: `futures`,
`http_extensions`, `layered`, `mutants`, `ohno`, `tick`). Nothing in this crate — including
the two highest-impact per-request costs found in this entire review, H1 and H2 — is
measured.

The gap is compounded by the shape of the sibling crate's benchmarks: `seatbelt`'s
partitioned-breaker scenarios key on `u64` (`crates/seatbelt/benches/breaker.rs:92`,
`breaker_cg.rs:78`) while the real consumer keys on a freshly formatted `String`, and
`seatbelt`'s retry/hedging scenarios clone a ZST (`benches/retry.rs:80`,
`benches/hedging.rs:76-77`) while the real consumer deep-clones an `HttpRequest`. So even
the benchmarks that *do* exist are measuring a materially cheaper workload than the one the
documentation steers users towards.

### Considered and ruled out

- **`Retry-After` header parsing on every response** — it is guarded.
  `ResponseExt::recovery_with_clock` (`crates/http_extensions/src/extensions/response_ext.rs:29-38`)
  only calls `get_retry_after_duration` when the status-derived recovery kind is
  `RecoveryKind::Retry`, so successful responses never touch the header map or the RFC-2822
  parser. Correct as written.
- **`HttpRecovery::recovery` dispatch** (`http_recovery.rs:64-69`) — `Inner::Default` is a
  direct call, not a virtual one. Only user-supplied custom recovery pays the `Arc<dyn Fn>`
  hop, which is the expected trade. See H8.
- **`extract_http_request`** (`crates/seatbelt_http/src/retry.rs:143-150`) — checks the
  recovery kind before calling `take_request()`, so the request is only moved out of the
  error on the path that will actually retry. Already minimal.
- **`HttpClone::can_clone`** (`http_clone.rs:67-73`) — a three-way enum match on the method;
  free. Note it is checked *before* the expensive clone at `:49-53`, which is the right
  order.
- **`Scheme`/`Authority` clones in `create_breaker_id`** (`breaker.rs:156`) — these are
  `Bytes`-backed refcounted types, so the clones themselves are refcount bumps. The cost in
  H1 is the `to_string()` that follows them, not the clones.
- **Heavy default features** — there are none; both crates default to an empty feature set
  (`crates/seatbelt/Cargo.toml:48`; `seatbelt_http` implicitly, see H7). The
  feature graph is fine-grained and each `seatbelt_http` feature forwards to exactly the
  matching `seatbelt` feature (`Cargo.toml:35-38`). Good.

---

## Cross-crate note

The chain H1 → F1(b) → F1(c) is the single most valuable thing in this document: one
convenience helper in a 400-line adapter crate (`http_configure_defaults`) causes a
per-request `String` format, *and* pushes every request off `seatbelt`'s carefully built
lock-free engine fast path, *and* onto a string-keyed `BTreeMap` walk, *and* into a global
mutex taken twice. Each of the three links has a small, surgical, behaviour-preserving fix,
and none of them is currently measured by any benchmark in the repository.
