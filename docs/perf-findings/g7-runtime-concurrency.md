# g7-runtime-concurrency findings

Scope: `crates/tick`, `crates/thread_aware`, `crates/thread_aware_macros`,
`crates/thread_aware_macros_impl`, `crates/anyspawn`, `crates/anyspawn_azure`,
`crates/uniflight`, `crates/layered`.

## Preamble — method and environment

The analysis environment has **no egress to `index.crates.io` / `static.crates.io`**
(403 `CONNECT tunnel failed`), no cargo registry cache and no prebuilt `target/`.
`cargo build`, `cargo bench`, `cargo clippy`, `cargo … --offline` and every `just`
recipe therefore fail at dependency resolution. This was confirmed and then not
fought further.

Consequences for this document:

* Almost every finding is labelled **`inferred from code reading`**. Each one names
  the specific benchmark that would confirm or refute it, so the claims are
  falsifiable once a network-enabled machine is available.
* The one empirical avenue that *does* work offline is a standalone,
  dependency-free program compiled with plain `rustc -O` containing
  layout-identical replicas of the types under study. That was used for the
  `thread_aware::Arc` / `Factory` / `Affinity` size claims, which are labelled
  **`empirically verified`**. The probe lived in `/tmp` and has been deleted; it was
  never added to the repository.

Cross-cutting build context supplied by the workspace-level group, which colours
every "Benchmark coverage" section below:

* `[profile.bench]` sets `lto = "fat"` and `codegen-units = 1`; `[profile.release]`
  sets neither. Repo benchmark numbers therefore come from a build configuration no
  consumer of these crates actually gets, and — critically for this cluster — a fat-LTO
  build **hides missing `#[inline]`**, because cross-crate inlining happens anyway at
  link time. Every "missing `#[inline]`" finding below is consequently *invisible* to
  the current benchmark suite by construction.
* Benchmarks build with `--all-features`, and dev-dependency feature unification
  pulls in `tick/test-util` across the workspace. See finding **T7**.

House philosophy alignment (`docs/performance.md`): findings are ordered to favour
surgical fixes; architectural findings are reported (as instructed) but explicitly
flagged with a **Philosophy note**. First-insert / teardown-only costs are
deprioritised and mostly appear under "Considered and ruled out". No finding
proposes removing a defensive runtime check.

**Headline picture for this cluster.** This is the concurrency/runtime cluster, and
its dominant theme is that *shared mutable rendezvous points sit directly on
per-operation paths*: one process-wide `Mutex<Timers>` per timeout in `tick`, one
shared `Mutex<Pool>` per request in `layered`, and a DashMap shard **write** lock per
completion in `uniflight`. Compounding this, `thread_aware::Arc` — the workspace's
own smart pointer, used by `tick`, `anyspawn` and `uniflight` on their hot paths — is
48 bytes and costs 3–4 atomic read-modify-writes per clone, versus 8 bytes and 1 RMW
for `std::sync::Arc`. And none of it is measured: seven of the eight crates have zero
Callgrind coverage and, `uniflight` aside, **nothing in the cluster benchmarks
anything multithreaded at all**, so contention — the single thing this cluster
exists to manage — is structurally unmeasured.

Counts: **49 findings** — 8 High, 20 Medium, 21 Low.

---

## Crate: tick

### Summary

`tick` is the clock abstraction: `Clock` (system time, monotonic instants, delays,
timeouts, periodic timers, stopwatches) plus a `test-util`-gated controllable clock.
It is on the request path of at least `fetch`, `seatbelt`, `cachet` and
`http_extensions`, all of which create a timeout per request.

Two structural problems dominate. First, every `Delay` and every `timeout`
registers *and* unregisters a waker in a `BTreeMap` behind a mutex; in the
`Shared` (Tokio) configuration that is **one process-wide mutex** shared by every
timer in the process, despite an inline comment asserting this is "not accessed on
a hot path". Second, the timer driver **calls `Waker::wake()` while still holding
that mutex**, so every woken task's very next action is to contend for the lock it
was just woken under — a textbook lock convoy at the exact moment of peak
contention.

The good news is that the *cheap* clock reads are genuinely cheap: without
`test-util`, `SimpleClock` is a one-byte enum and `instant()` / `system_time()`
compile down to a direct `Instant::now()` / `SystemTime::now()` with a single
predictable branch. The virtual-clock abstraction imposes essentially no
production overhead *at the type level* — but see **T7** for how feature
unification undoes that in the benchmark build.

### Findings

#### T1. Every delay and timeout registers through a process-wide mutex

- **Location:** `crates/tick/src/state.rs:71-90` (the `SynchronizedTimers` enum and
  the comment claiming this is not a hot path), `crates/tick/src/state.rs:112-127`
  (`with_timers`), `crates/tick/src/clock.rs:458-472`
  (`register_timer` / `unregister_timer`), `crates/tick/src/delay.rs:88-113`.
- **Issue:** `SynchronizedTimers` has two variants. The `Isolated` variant stores the
  timers in a `thread_aware::Arc<Mutex<Timers>, PerThread>`-style per-thread cell,
  which is fine for thread-per-core runtimes. The `Shared` variant — the one the
  Tokio integration uses, and therefore the one essentially every consumer gets —
  stores a single `std::sync::Arc<Mutex<Timers>>` for the whole process. Every
  `Delay::poll` that finds the deadline in the future calls `register_timer`, which
  locks it; every `Delay` drop or ready-poll calls `unregister_timer`, which locks it
  again. A server issuing one `timeout` per request therefore takes a global mutex a
  minimum of twice per request, on top of two `BTreeMap` operations, and *all* cores
  serialise on that one lock.

  The inline comment at `state.rs:71-77` explicitly asserts that timer registration
  is "not accessed on a hot path". That assumption does not survive contact with the
  consumers: `fetch` and `seatbelt` wrap every outbound request in a `tick` timeout.
- **Impact:** **High** — a single uncontended `Mutex` lock/unlock pair is ~20–40
  cycles, which is unremarkable; the problem is that it is *one* mutex for the whole
  process, so throughput on this path is capped at roughly one registration per
  lock round-trip regardless of core count, and the cache line holding the mutex plus
  the `BTreeMap` root ping-pongs between every core in the system. On a 64-core box
  under load this is the difference between a scaling and a non-scaling timeout path.
- **Remediation:** Surgical, in increasing order of invasiveness:
  1. Shard the `Shared` variant: replace `Mutex<Timers>` with a small fixed array of
     `Mutex<Timers>` selected by `Instant`-derived hash or by thread ID, with the
     driver advancing all shards. Preserves the API exactly.
  2. Or make the Tokio integration use the `Isolated` (per-thread) variant, which
     already exists and is already tested — this is the architecturally cleanest fix
     but requires a per-worker-thread driver, which the `runtime` module already
     supports for thread-per-core.
  3. At minimum, correct the comment at `state.rs:71-77`, which currently misleads
     future maintainers into leaving this alone.
  Combine with **T3**, which removes half the lock acquisitions outright.
- **Evidence:** inferred from code reading. **Confirming benchmark:** a new
  multithreaded Criterion benchmark `crates/tick/benches/delay_bench.rs`, group
  `delay_bench/register_unregister`, run via `bench_on_threadpool()` (per
  `docs/benchmarks.md`) at 1, 2, 4, 8, 16 threads, each thread creating and
  immediately dropping a `Clock::delay(Duration::from_secs(3600))`. If T1 is real the
  per-operation time will rise super-linearly with thread count; if the comment is
  right it will stay flat.

#### T2. Wakers are invoked while the timers mutex is held — guaranteed lock convoy

- **Location:** `crates/tick/src/timers.rs:101-130` (`advance_timers`, with the wake
  loop at `:118-120`), reached through `crates/tick/src/state.rs:112-127`
  (`with_timers` holds the lock for the whole closure), driven from
  `crates/tick/src/runtime/clock_driver.rs:39-52`.
- **Issue:** `advance_timers` splits the expired entries out of the `BTreeMap` and
  then calls `waker.wake()` on each of them — *inside* the `with_timers` closure,
  i.e. with the mutex still held. `Waker::wake` on a multi-threaded Tokio runtime
  synchronously schedules the task, and on an already-idle worker it can begin
  polling it almost immediately. That task's first action in `Delay::poll` is
  `unregister_timer` (see **T3**), which needs the very lock the waking thread is
  still holding. With a batch of N expired timers, the driver wakes N tasks that then
  queue up behind the driver on one lock, and behind each other.

  This is the classic "notify under the lock" antipattern. The standard remedy is
  well established in the ecosystem (`std::sync::Condvar` docs, Tokio's own timer
  wheel): collect the wakers into a local `Vec`, drop the lock, *then* wake.
- **Impact:** **High** — this is a contention amplifier that fires precisely when
  contention is highest (a batch of coincident deadlines, e.g. a burst of requests
  that all timed out at the same nominal deadline). It converts what should be N
  parallel wakeups into an N-deep serial queue on one mutex. It is also *invisible* to
  every current benchmark, since none of them is multithreaded.
- **Remediation:** Surgical and entirely contained in `timers.rs::advance_timers`:
  `let expired = …split_off…;` inside the closure, return the expired wakers out of
  `with_timers`, and run the `wake()` loop after `with_timers` returns. The function
  already computes the expired set as a separate collection, so the change is a few
  lines and no API surface moves.
- **Evidence:** inferred from code reading. **Confirming benchmark:** a multithreaded
  benchmark that arms K delays with an identical deadline across T threads and
  measures wall-clock time from deadline to last-task-resumed. Additionally a
  Callgrind `delay_bench_cg.rs` will *not* show this (single-threaded, no contention),
  which is itself worth recording — this is a defect only a threaded wall-clock
  benchmark can see.

#### T3. Redundant `unregister_timer` on the normal timer-fired path

- **Location:** `crates/tick/src/delay.rs:98-113` (the ready branch),
  `crates/tick/src/periodic_timer.rs:126-140` (same pattern),
  `crates/tick/src/clock.rs:465-472` (`unregister_timer`).
- **Issue:** When a timer fires, `Timers::advance_timers` has *already* removed its
  entry from the `BTreeMap` via `split_off` (`timers.rs:113-115`). The woken task
  then polls `Delay`, observes the deadline has passed, and — before returning
  `Poll::Ready` — still calls `self.clock.unregister_timer(key)`. That takes the
  mutex (see **T1**) to perform a `BTreeMap::remove` that is *guaranteed to miss*.
  So the common, successful path pays a third lock acquisition for nothing.
- **Impact:** **Medium** — a third of the lock traffic on the timeout path is pure
  waste, and it lands immediately after **T2** has already queued the task behind the
  driver. Individually cheap; systematically wasteful, and it is the cheapest of the
  three `tick` locking findings to fix.
- **Remediation:** Track registration state in the `Delay` (it already has a
  `registered: Option<TimerKey>`-shaped notion in its state machine) and only call
  `unregister_timer` when the timer is still believed to be armed — i.e. on the
  `Pending`→drop path and not on the `Ready` path. Alternatively have
  `advance_timers` mark fired keys so the miss can be detected without the lock.
  This is a textbook surgical intervention per `docs/performance.md`.
- **Evidence:** inferred from code reading. **Confirming benchmark:**
  `crates/tick/benches/delay_bench_cg.rs` (Callgrind), scenario
  `delay_bench_cg::delay_elapsed` — arm a delay with a deadline already in the past,
  advance, poll to completion, and count instructions. Removing the redundant
  unregister should drop a measurable, stable instruction count (a mutex
  lock/unlock plus a BTree descent).

#### T4. A whole `Clock` is cloned per delay, timeout and periodic timer

- **Location:** `crates/tick/src/delay.rs:72-78` (`Delay::new`),
  `crates/tick/src/clock.rs:425-431` (`Clock::delay`),
  `crates/tick/src/periodic_timer.rs:95-105`,
  `crates/tick/src/future_ext.rs:28-38` (`FutureExt::timeout`),
  `crates/tick/src/timeout.rs` (holds the cloned `Clock` for the future's lifetime).
- **Issue:** Every `Delay`, `Timeout` and `PeriodicTimer` stores its own cloned
  `Clock`. `Clock::clone` clones `ClockState`, which in the `Isolated` configuration
  is a `thread_aware::Arc` — **3 atomic read-modify-writes**, not one (see **TA2**) —
  plus a `SimpleClock` clone (free without `test-util`, one atomic RMW with it, see
  **T7**). So a per-request `timeout` pays 3–4 atomic RMWs on construction and the
  matching decrements on drop, purely to hold a handle it uses twice.
- **Impact:** **Medium** — atomic RMWs on a shared refcount are the classic scaling
  killer: uncontended they are ~20 cycles, but the cache line is shared by every
  thread that owns a `Clock`, so under load each one is a coherence miss. Six to
  eight of them per request, on top of **T1**'s global mutex, on the same cache
  lines.
- **Remediation:** Two options, both surgical:
  1. Have `Delay` / `Timeout` borrow the clock (`&'a Clock`) where the lifetime
     permits — `FutureExt::timeout` is called as
     `fut.timeout(&clock, dur).await`-shaped at most call sites, so the clock
     outlives the future. This is a breaking API change and should be weighed.
  2. Cheaper and non-breaking: store only the parts the future actually needs
     (the `SynchronizedTimers` handle), not the whole `Clock`, so the clone cost
     drops to one `Arc` clone rather than a `thread_aware::Arc` clone.
  Fixing **TA2** in `thread_aware` would also reduce this from 3 RMWs to 1 with no
  change to `tick` at all.
- **Evidence:** inferred from code reading; the "3 atomic RMWs" component is
  empirically grounded via the `thread_aware` layout probe and by reading
  `cell/mod.rs:142-150` + `factory.rs:43-52`. **Confirming benchmark:**
  `crates/tick/benches/delay_bench_cg.rs`, scenario `delay_bench_cg::timeout_noop` —
  wrap an immediately-ready future in `timeout` and count instructions; the atomic
  RMW pairs are directly visible in the Callgrind instruction counts.

#### T5. Zero `#[inline]` across 35 public functions crossing the crate boundary

- **Location:** whole crate. Representative offenders on the hot path:
  `crates/tick/src/clock.rs` (`system_time`, `instant`, `simple_clock`),
  `crates/tick/src/simple_clock.rs:57-90` (`system_time`, `instant`),
  `crates/tick/src/stopwatch.rs:56-90` (`new`, `elapsed`).
- **Issue:** A census of the crate finds **zero** `#[inline]` attributes against 35
  public functions. `docs/performance.md` rule 1 asks for `#[inline]` on small public
  functions that cross a crate boundary, precisely because without LTO the compiler
  cannot see across the boundary. `SimpleClock::instant()` without `test-util` is a
  one-branch wrapper around `Instant::now()`; unannotated, a downstream crate compiled
  with default `[profile.release]` (which, per the preamble, sets **no LTO**) must
  emit a real call for it. `Stopwatch::elapsed()` is likewise a subtraction.
- **Impact:** **Medium** — individually a call/ret is a handful of cycles, but
  `instant()` is the single most frequently called function in the crate (every
  delay poll, every stopwatch, every consumer's latency accounting), and the
  non-inlined call also blocks constant folding of the surrounding branch on
  `TimeKind`. Medium rather than High because the absolute per-call cost is small.
- **Remediation:** Add `#[inline]` judiciously — not blanket. The defensible set is:
  `SimpleClock::{instant, system_time}`, `Clock::{instant, system_time,
  simple_clock}`, `Stopwatch::{new, elapsed}`, and the `Duration`/`Instant`
  accessors. Explicitly *not* `Clock::delay` or anything that touches the timer map.
- **Evidence:** empirically verified by grep census (0 `#[inline]` / 35 `pub fn` in
  `crates/tick/src`); the performance consequence is inferred from code reading.
  **Confirming benchmark:** this one is *structurally unconfirmable with the current
  setup* — `[profile.bench]` uses `lto = "fat"`, which inlines across crates anyway.
  Confirming it requires either a bench profile with `lto = false` (matching
  `[profile.release]`) or a separate consumer-crate microbenchmark. Recording that
  gap is itself part of this finding.

#### T6. Idle clock driver pays an RwLock read and an O(processors) scan every tick

- **Location:** `crates/tick/src/runtime/clock_driver.rs:39-52` (`advance_timers`,
  the `self.state.is_unique()` call at `:49`),
  `crates/tick/src/state.rs:129-140` (`is_unique`),
  `crates/thread_aware/src/cell/mod.rs:571-576` (`Arc::strong_count`),
  `crates/thread_aware/src/cell/storage.rs:70-85`.
- **Issue:** `ClockDriver::advance_timers` is called by the runtime on *every* tick,
  including when there are no timers at all. When `next_timer` is `None` it evaluates
  `self.state.is_unique()` to decide whether to report `ClockGone`. For the
  `Isolated` variant that resolves to `thread_aware::Arc::strong_count`, which takes
  the storage **RwLock read lock** and then iterates *every* per-processor slot doing
  an `Arc::ptr_eq` and a `strong_count` per slot. On a 64-core machine that is a
  64-iteration scan plus a lock acquisition, performed on every idle driver tick, to
  answer a question ("has everyone dropped the clock?") that is only interesting at
  shutdown.
- **Impact:** **Medium** — it is idle-path work, so it does not affect request
  latency directly, but it is O(cores) per tick per driver, and with a
  thread-per-core runtime that is O(cores²) of pure background work per tick
  workspace-wide. It also keeps the storage cache lines warm-and-shared for no
  reason, which interacts badly with **TA3**.
- **Remediation:** Only check `is_unique()` when the timer map has just transitioned
  to empty (i.e. when the previous tick had timers and this one does not), rather
  than on every empty tick. Or maintain an explicit `AtomicUsize` owner count on
  `ClockState`. Both are surgical.
- **Evidence:** inferred from code reading. **Confirming benchmark:**
  `crates/tick/benches/clock_bench_cg.rs`, scenario
  `clock_bench_cg::driver_idle_tick` — construct an `InactiveClock`, activate, and
  `advance_timers` with no timers registered. Instruction count should scale with
  `many_cpus`-reported processor count if this finding is correct, and be flat if not.

#### T7. `test-util` feature unification makes every repo benchmark measure the slow clock

- **Location:** `crates/tick/src/simple_clock.rs:30-50` (the `TimeKind` enum, whose
  `Controlled(ClockControl)` variant exists only under `test-util`),
  `crates/tick/src/stopwatch.rs:56-60`, `crates/tick/Cargo.toml` (`test-util`
  feature); consumers declaring `tick = { …, features = ["test-util"] }` in
  `[dev-dependencies]` include `fetch`, `seatbelt`, `cachet` and `http_extensions`.
- **Issue:** Without `test-util`, `TimeKind` has a single variant, `SimpleClock` is
  effectively a ZST, `SimpleClock::clone` is free, and `instant()` is an
  unconditional `Instant::now()`. With `test-util`, `TimeKind` gains a
  `Controlled(ClockControl)` variant carrying an `Arc`, so `SimpleClock::clone`
  becomes an atomic RMW, `instant()` gains a branch, and `Stopwatch::new` (which
  clones the clock) goes from free to one atomic RMW.

  Cargo unifies features across the dependency graph for a given build. Because
  benchmarks are built with dev-dependencies *and*, per the workspace context, with
  `--all-features`, every benchmark in the workspace that transitively touches `tick`
  measures the `test-util` variant. **The numbers in the repo therefore describe a
  configuration production never runs.**
- **Impact:** **Medium** — the per-call delta is small (a predictable branch, one
  atomic RMW per clock clone), but the *epistemic* impact is large: it means the
  crate's own benchmark suite systematically over-reports the cost of the production
  clock and, worse, would fail to notice a regression that only affects the
  production variant. Combined with the `lto = "fat"` mismatch from the preamble, the
  benchmark build differs from the shipped build along two independent axes.
- **Remediation:** No code change to `tick` itself. Either (a) document in
  `docs/benchmarks.md` that `tick`-touching benchmarks must be read as
  `test-util`-enabled numbers, or (b) add a benchmark that explicitly asserts the
  no-`test-util` shape (e.g. a `const _: () = assert!(size_of::<SimpleClock>() == 1)`
  compile-time check in the non-`test-util` cfg), so the cheap-in-production property
  is guarded rather than assumed.
- **Evidence:** inferred from code reading (feature gating in `simple_clock.rs` is
  explicit; the dev-dependency declarations were read from consumer `Cargo.toml`s).
  **Confirming benchmark:** run `clock_bench` twice, once with
  `--no-default-features` and once with `--all-features`, and diff. Any difference
  proves the point.
#### T8. `advance_timers` allocates a fresh `BTreeMap` per firing batch

- **Location:** `crates/tick/src/timers.rs:101-130`, specifically the
  `split_off` + `mem::replace` pair at `:113-115`.
- **Issue:** The expired-timer extraction is `let mut expired = mem::replace(&mut
  self.timers, BTreeMap::new()); let remaining = expired.split_off(&key); self.timers
  = remaining;`-shaped. `BTreeMap::split_off` is O(log n) but allocates new internal
  nodes for the split boundary, and the pattern churns the map's root on every firing
  batch even when only one timer expired.
- **Impact:** **Low** — it happens once per driver tick that has expiries, not once
  per timer, and the allocation is small. Reported for completeness because it sits
  *inside* the critical section identified in **T2**, so its cost is multiplied by the
  convoy.
- **Remediation:** For the common case of few expiries, `while let Some(entry) =
  self.timers.first_entry() { if entry.key() > &now { break } expired.push(entry.remove()) }`
  avoids the split entirely and touches only the leftmost spine. Worth doing only in
  combination with **T2**, since both edit the same function.
- **Evidence:** inferred from code reading. **Confirming benchmark:**
  `crates/tick/benches/timers_cg.rs`, scenarios `timers_cg::advance_one_expired` and
  `timers_cg::advance_none_expired` with 1 / 100 / 10 000 armed timers.

#### T9. `PeriodicTimer` re-anchors each tick on a fresh clock read, accumulating drift

- **Location:** `crates/tick/src/periodic_timer.rs:107-119`.
- **Issue:** The next deadline is computed as `self.clock.instant() + self.period`
  rather than `previous_deadline + self.period`. Every tick therefore absorbs the
  scheduling latency of the tick before it, and the period drifts monotonically
  later. It also costs an extra clock read per tick (see **T5** — unannotated, so a
  real call).
- **Impact:** **Low** as a *performance* matter (one extra `Instant::now()` per
  period, and periods are by definition not hot). Flagged mainly because the
  correctness consequence — unbounded drift — is likely to surprise, and because the
  fix is free.
- **Remediation:** Anchor on the previous deadline: `self.next += self.period`, with a
  catch-up clamp (`while self.next <= now { self.next += self.period }`) so a stalled
  timer does not fire a burst. This matches `tokio::time::interval`'s
  `MissedTickBehavior::Delay` semantics, so it is the ecosystem-idiomatic shape.
- **Evidence:** inferred from code reading. **Confirming benchmark:** not a
  benchmark — a test asserting that N ticks of period P complete within
  N·P + ε would catch the drift.

#### T10. `Clock` is copied by value into every future it constructs, and is not small

- **Location:** `crates/tick/src/clock.rs:40-70` (the `Clock` struct),
  `crates/tick/src/state.rs:78-90` (`ClockState`), plus every constructor listed
  in **T4**.
- **Issue:** `Clock` holds a `ClockState` (an enum whose largest variant is a
  `thread_aware::Arc`, i.e. 48 bytes — see **TA1**) plus a `SimpleClock`. So a
  `Clock` is ~56 bytes, and every `Delay` / `Timeout` / `PeriodicTimer` embeds one by
  value. `Timeout<F>` therefore has a state machine at least 56 bytes larger than the
  wrapped future, which matters when the caller boxes it or stores it in a
  `select!`/`FuturesUnordered`.
- **Impact:** **Low** on its own — it is stack/inline space, not heap traffic — but
  it compounds **T4** and it directly propagates **TA1**. Downgrading
  `thread_aware::Arc` to `std::sync::Arc` in `ClockState` where per-thread affinity is
  not needed would take `Clock` from ~56 bytes to ~16.
- **Remediation:** Deferred to **TA1**; nothing to do inside `tick` that does not
  first change `thread_aware`.
- **Evidence:** the size components are empirically verified (standalone `rustc`
  layout replica: `thread_aware::Arc<T, PerCore>` = 48 bytes); the composition into
  `Clock` is inferred from code reading. **Confirming benchmark:** a
  `const _: () = { … size_of::<Clock>() … }` assertion, or Callgrind stack-usage
  inspection on `timeout_noop`.

### Benchmark coverage

`crates/tick/benches/` contains exactly one file, `clock_bench.rs`. There is **no
Callgrind (`*_cg.rs`) coverage at all** for this crate.

Problems with the existing benchmark:

1. **It is one compound 15-operation benchmark.** `docs/benchmarks.md` is explicit
   that benchmarks should measure *elementary* operations. A single closure that
   performs fifteen different clock operations cannot attribute a regression to any
   one of them, which is the whole point of the guidance.
2. **`Instant::now()` is called inside the measured closure** as part of the setup for
   later operations, so the measurement includes work that is not under test.
3. **Naming violation.** The Criterion group is named `clock_operations`. Per
   `docs/naming.md`, the group must be prefixed with the benchmark file's basename —
   it should be `clock_bench/…`. Without the prefix, groups from different files can
   collide in Criterion's output directory.
4. **The highest-frequency APIs are unbenchmarked in isolation**: `instant()`,
   `system_time()` and `stopwatch()` are the calls consumers make per request, and
   none has its own benchmark.
5. **Nothing is multithreaded.** Per `docs/benchmarks.md`, Criterion benchmarks are
   single-threaded by default and multithreaded ones must be written explicitly with
   `bench_on_threadpool()`. As a consequence **T1, T2 and T3 — the three highest-value
   findings in this crate — cannot be observed by the current suite at all.** For a
   crate whose central data structure is a shared, mutex-protected timer map, that is
   the coverage gap that matters most.

Recommended additions, in priority order:

- `crates/tick/benches/delay_bench.rs` with a `bench_on_threadpool()` group
  `delay_bench/register_unregister_contended` at 1/2/4/8/16 threads → confirms T1, T2.
- `crates/tick/benches/delay_bench_cg.rs` (Callgrind, paired with the above per
  `docs/naming.md`) with scenarios `delay_elapsed` and `timeout_noop` → confirms T3,
  T4, T10 in stable instruction counts.
- Split `clock_bench.rs` into elementary groups `clock_bench/instant`,
  `clock_bench/system_time`, `clock_bench/stopwatch_elapsed`, and add
  `clock_bench_cg.rs` with a `driver_idle_tick` scenario → confirms T5, T6.

### Considered and ruled out

- **Atomic ordering.** A grep of the crate's production code finds **no `SeqCst`
  anywhere** outside tests. The orderings in use are appropriate; nothing to tighten.
- **`Timeout` future overhead.** `timeout.rs` is a plain two-branch `poll` with no
  boxing and no allocation. It is well written; the only cost is the embedded
  `Clock` (**T10**).
- **`TimerKey` ordering cost.** Ordering `Instant` keys compares a two-word
  `timespec`, ~2 instructions per BTree probe. Immaterial next to the lock.
- **`ClockControl` cost.** All of `clock_control.rs` is behind `test-util` and does
  not exist in a production build. Correctly gated; not a production concern (but see
  **T7** for how it leaks into *benchmarks*).
- **`fmt` module.** Formatting helpers are not on any hot path; excluded.
- **First `Timers` map allocation.** `docs/performance.md` explicitly deprioritises
  first-insert costs. Not reported.

---

## Crate: thread_aware

### Summary

`thread_aware` provides `ThreadAware` (a trait for types that must be told when they
move between processors) and a per-affinity smart pointer, `thread_aware::Arc<T, S>`,
which keeps one instance of `T` per storage slot (per-core, per-thread, per-process,
per-memory-region) so that hot data is not shared across cores.

The intent is sound and the false-sharing problem it solves is real. But the
implementation has a cost profile that is very different from what the name
suggests, and three downstream crates in this cluster — `tick`, `anyspawn`,
`uniflight` — use it on their per-operation paths on the assumption that it behaves
like `std::sync::Arc`.

Measured facts (standalone `rustc -O` layout replica, x86-64 Linux):

| Type | Size |
|---|---|
| `std::sync::Arc<u32>` | 8 bytes |
| `thread_aware::Arc<u32, PerCore>` | **48 bytes** |
| `thread_aware::Arc<dyn Trait, PerCore>` | **64 bytes** |
| `Factory<u32>` | 32 bytes |
| `ErasedCloneFn<u32>` | 24 bytes |
| `Option<Affinity>` | 10 bytes (no niche) |

And `clone()` costs **2 to 4 atomic read-modify-writes** depending on the factory
variant, versus 1 for `std::sync::Arc`.

### Findings

#### TA1. `thread_aware::Arc` is 48 bytes (64 for `dyn`), six to eight times `std::sync::Arc`

- **Location:** `crates/thread_aware/src/cell/mod.rs:109-114` (the struct),
  `crates/thread_aware/src/cell/factory.rs:18-30` (the `Factory` enum),
  `crates/thread_aware/src/cell/clone_fn.rs:19-23` (`ErasedCloneFn`),
  `crates/thread_aware/src/affinity.rs` (`Affinity`).
- **Issue:** The struct carries a `std::sync::Arc<RwLock<Storage<…>>>` (8), a
  `Factory<T>` (32) and the cached value/affinity fields. `Factory` is an enum whose
  widest variant is `ErasedCloneFn` (24 bytes: two `Arc`s plus a vtable-ish pointer)
  plus a discriminant, rounded to 32. `Affinity` is four `u16`s with **no niche**, so
  `Option<Affinity>` is 10 bytes rather than 8. For `T: ?Sized` the value pointer is
  fat, taking the total to 64.

  The consequence is that any type embedding one — `tick::Clock` (**T10**),
  `anyspawn::Spawner` (**AS2**), `uniflight::Merger` — is at least 40 bytes larger
  than the author probably expects, and passing one by value moves 48 bytes rather
  than 8. In `uniflight::Merger::execute` and `tick`'s `Delay`, that struct is copied
  per operation.
- **Impact:** **High** — not because 48 bytes is inherently expensive, but because
  the type is *presented as an `Arc`* and is used as one on per-operation paths by
  three sibling crates in this cluster. Every one of those authors reasonably
  assumed pointer-sized-and-cheap. The size shows up as extra stack traffic in every
  async state machine that holds one across an `.await`, which for `tick::Timeout`
  and `uniflight`'s returned future is every single call.
- **Remediation:** The surgical, non-architectural mitigation is **not** to redesign
  `thread_aware::Arc` but to stop using it where per-affinity storage is not actually
  needed:
  * `tick::ClockState` — the `Shared` variant is already a plain `Arc`; the
    `Isolated` variant is the one paying, and it is only used by thread-per-core
    runtimes where a `thread_local!` would serve.
  * `anyspawn::CustomSpawner` — see **AS2**.
  * `uniflight::Merger` — see **U5**.
  Within `thread_aware` itself, the only cheap win is boxing the cold `Factory`
  variants: `Factory::Closure(Box<…>)` / `Factory::ErasedCloneFn(Box<…>)` would take
  `Factory` from 32 bytes to 8 and `Arc` from 48 to 24, at the cost of one extra
  indirection on the (cold) miss path. That *is* surgical.
- **Evidence:** **empirically verified** — a standalone dependency-free program
  compiled with `rustc -O` containing layout-identical replicas of `Affinity`,
  `Storage`, `Factory`, `ErasedCloneFn` and `Arc`, printing `size_of`/`align_of`.
  Results as tabulated above. **Confirming benchmark:** `const _: () =
  assert!(size_of::<Arc<u32, PerCore>>() <= N)` as a layout guard, plus a
  `thread_aware/benches/arc_cg.rs` scenario measuring stack traffic for
  pass-by-value.
- **Philosophy note:** **Conflicting.** `docs/performance.md` prefers surgical
  interventions over architectural rewrites. Shrinking `Arc` below 24 bytes would
  require rethinking `Factory`, which is architectural. This is reported because it
  materially explains the cost profile of three sibling crates, **not** recommended
  as an action. The boxed-cold-variant mitigation above is the only part of this
  finding that clears the surgical bar.

#### TA2. `Arc::clone` performs 3 atomic RMWs (4 for the `ErasedCloneFn` factory), not 1

- **Location:** `crates/thread_aware/src/cell/mod.rs:142-150` (`impl Clone`),
  `crates/thread_aware/src/cell/factory.rs:43-52` (`Factory::clone`),
  `crates/thread_aware/src/cell/clone_fn.rs:70-77` (`ErasedCloneFn::clone`, which
  clones **two** inner `Arc`s).
- **Issue:** `Arc::clone` clones (1) the storage `Arc<RwLock<Storage<…>>>`, (2) the
  cached value `Arc<T>`, and (3) the `Factory`. For `Factory::Closure` the third
  clone is itself an `Arc` clone → 3 RMWs total. For `Factory::ErasedCloneFn` the
  third clone is `ErasedCloneFn::clone`, which clones two `Arc`s → **4 RMWs total**.
  Only the `Data` / `Manual` variants get away with 2.

  This matters because the *whole point* of a per-affinity cell is to avoid
  cross-core contention, and yet cloning the handle touches two-to-four
  globally-shared refcount cache lines. `anyspawn::CustomSpawner` constructs its cell
  with `with_clone_fn` (`crates/anyspawn/src/custom.rs:92`), so it is on the 4-RMW
  path.
- **Impact:** **High** — an uncontended atomic RMW is ~20 cycles; a *contended* one
  (the same line being written by another core) is 100–400. Multiplying that by 3–4
  and placing it on the per-request path of `tick`, `anyspawn` and `uniflight` means
  this single implementation detail is plausibly the largest per-operation cost in
  the entire cluster. It is also completely invisible: nothing benchmarks it.
- **Remediation:** Surgical options that do not change the design:
  1. Store the `Factory` *inside* the already-shared
     `Arc<RwLock<Storage<…>>>` allocation rather than beside it. The factory is
     immutable after construction and is only consulted on a storage miss, so it
     belongs with the storage. This removes 1–2 RMWs from every clone and shrinks
     `Arc` at the same time (see **TA1**).
  2. Make `ErasedCloneFn` hold a single `Arc<(A, B)>` rather than two `Arc`s,
     removing the 4th RMW on the `anyspawn` path. Two lines.
  Option 2 alone is a clear, contained win.
- **Evidence:** inferred from code reading, with the clone-chain traced through all
  three files cited; the *type layout* underpinning it is empirically verified.
  **Confirming benchmark:** `crates/thread_aware/benches/arc_cg.rs` (Callgrind),
  scenarios `arc_cg::clone_data_factory`, `arc_cg::clone_closure_factory`,
  `arc_cg::clone_erased_clone_fn` — the `lock xadd` count is directly readable from
  Callgrind output and should be 2 / 3 / 4 respectively. This is the single most
  valuable benchmark missing from this crate.

#### TA3. `strong_count` takes a read lock and scans every storage slot

- **Location:** `crates/thread_aware/src/cell/mod.rs:571-576`,
  `crates/thread_aware/src/cell/storage.rs:70-85`.
- **Issue:** `strong_count` acquires the storage `RwLock` for reading and then walks
  the entire slot vector — one slot per processor — summing per-slot strong counts.
  It is O(processors) with a lock, not O(1) like `std::sync::Arc::strong_count`.
- **Impact:** **Medium** — it is not itself a hot path *within* `thread_aware`, but
  `tick::ClockState::is_unique()` calls it on **every idle driver tick** (see
  **T6**), which turns an O(cores) locked scan into per-tick background work. The API
  shape invites exactly this misuse because it looks like the `std` method it is
  named after.
- **Remediation:** Either (a) document loudly on the method that it is
  O(processors) + lock, not O(1), so callers stop treating it as free; or (b) maintain
  a separate `AtomicUsize` total, updated on slot creation/destruction only (rare),
  making the query O(1). (b) is contained to `storage.rs`.
- **Evidence:** inferred from code reading. **Confirming benchmark:**
  `thread_aware/benches/arc_cg.rs`, scenario `arc_cg::strong_count`, run on machines
  with different processor counts; instruction count should scale with core count.

#### TA4. Zero `#[inline]` across 27 public functions

- **Location:** whole crate. Hot ones: `crates/thread_aware/src/cell/mod.rs`
  (`Deref::deref`, `as_ref`), `crates/thread_aware/src/affinity.rs` (the `u16`
  accessors — `processor_id`, `memory_region_id` etc.),
  `crates/thread_aware/src/cell/storage.rs` (`get_clone`).
- **Issue:** 0 `#[inline]` attributes against 27 public functions. `Deref::deref` on
  a smart pointer is *the* canonical case for `#[inline]`: it is a single field load
  and it is called on every single use of the pointer. Unannotated, and with
  `[profile.release]` setting no LTO, a downstream crate must emit a call
  instruction to dereference a smart pointer. The `Affinity` accessors are single
  `u16` field reads.
- **Impact:** **Medium** — `deref` frequency is essentially "every access to any
  value held in a `thread_aware::Arc`", so even a few cycles is meaningful; and the
  missed inlining also blocks the surrounding code from keeping the target in a
  register.
- **Remediation:** Add `#[inline]` to `Deref::deref`, `AsRef::as_ref`, the `Affinity`
  field accessors, and `Storage::get_clone`. Deliberately *not* to `relocate`,
  `strong_count` or the factory paths, per the "be judicious" guidance.
- **Evidence:** empirically verified by grep census (0/27); performance consequence
  inferred. **Confirming benchmark:** as with **T5**, unconfirmable under the current
  `lto = "fat"` bench profile — needs an `lto = false` comparison build.

#### TA5. `relocate` takes the storage write lock even when only a read is needed

- **Location:** `crates/thread_aware/src/cell/mod.rs:585-648`, specifically the
  unconditional write-lock acquisition at `:587` and the early
  already-present check at `:589-600`.
- **Issue:** `relocate` acquires the storage `RwLock` for **writing** at the top of
  the function, then checks whether the destination slot already holds a value. In
  the steady state — a task that keeps returning to processors it has visited before,
  which is the common case for a pinned thread-per-core runtime — that check
  succeeds and the function only *reads*. So the common path takes an exclusive lock
  to do a read, excluding every other processor's relocate for its duration.
- **Impact:** **Medium** — `relocate` is called whenever a `ThreadAware` value is
  observed to have moved, which in a work-stealing runtime can be frequently. A write
  lock serialises *all* processors, not just the two involved, so this converts a
  per-processor operation into a global one — precisely the property the crate exists
  to avoid.
- **Remediation:** Double-checked locking: take the read lock, check whether the
  destination slot is populated, and only upgrade to a write lock on the miss. This
  is the standard `RwLock` idiom and is contained to one function. It preserves all
  defensive checks (the write-lock branch re-checks after upgrading, as it must).
- **Evidence:** inferred from code reading. **Confirming benchmark:** a
  multithreaded `thread_aware/benches/arc_relocate.rs` using `bench_on_threadpool()`
  where T threads repeatedly relocate their own cell between two known affinities;
  read-lock fast path should make throughput scale with T, write-lock should make it
  flat.

#### TA6. `Affinity` has no niche, inflating `Option<Affinity>` and `Factory`

- **Location:** `crates/thread_aware/src/affinity.rs` (the four `u16` fields),
  `crates/thread_aware/src/cell/factory.rs:18-30`.
- **Issue:** `Affinity` is four plain `u16`s, so every bit pattern is valid and
  `Option<Affinity>` costs an extra discriminant byte plus padding: **10 bytes
  measured**, versus 8 if a niche existed.
- **Impact:** **Low** — and, importantly, fixing it does **not** help where you would
  expect. See "Considered and ruled out" below: I tested making `Affinity` use
  `NonZeroU16`-style niches and `Factory` stays 32 bytes regardless, because the
  `ErasedCloneFn` variant (24 bytes) dominates. So the only gain is 2 bytes on
  standalone `Option<Affinity>` values.
- **Remediation:** Not worth doing on its own. Only relevant if **TA1**'s boxed-cold-
  variant change is made, at which point the `Closure` variant becomes the widest and
  the niche would matter.
- **Evidence:** **empirically verified** (standalone `rustc` layout replica measured
  `Option<Affinity>` at 10 bytes, and measured that a niche-carrying variant leaves
  `Factory` at 32).
#### TA7. Erased closures cost a `Box` inside an `Arc` — double indirection per relocation

- **Location:** `crates/thread_aware/src/closure/erased.rs:14-16` (`ErasedClosureOnce`
  holds `Box<dyn Erased<T>>`), `:52-58` (`Clone` calls `clone_boxed`),
  `:78-82` (`clone_boxed` heap-allocates a fresh `Box`),
  `:46-50` (`relocate` is a virtual call through the box).
- **Issue:** Cloning an `ErasedClosureOnce` **allocates** — `clone_boxed` does a
  `Box::new`. And `relocate` on it is a virtual call to `transfer_boxed_mut`, which
  then makes a second (static) call to the inner closure. So a cell whose factory is
  an erased closure pays a heap allocation on every handle clone that has to
  materialise a new slot value, plus two-level indirection on every relocation.
- **Impact:** **Low** — this is on the storage-miss path (new processor first
  touches the cell), which per `docs/performance.md` is a first-insert cost and is
  explicitly deprioritised. Recorded so the cost is known, not as a call to action.
- **Remediation:** None recommended. If ever needed, `Arc<dyn Erased<T>>` instead of
  `Box<dyn Erased<T>>` would make clone refcount-only, at the cost of forcing the
  closure to be shared rather than duplicated — which would change semantics, so it
  is not a drop-in.
- **Evidence:** inferred from code reading (the `Box::new` in `clone_boxed` is
  explicit at `erased.rs:79`).

#### TA8. `Storage` grows with `resize_with`, reallocating the slot vector

- **Location:** `crates/thread_aware/src/cell/storage.rs` (the `resize_with` /
  slot-growth path).
- **Issue:** Slots are added lazily as new processors touch the cell, via
  `Vec::resize_with`, which can reallocate and memcpy the whole slot vector while the
  **write** lock is held.
- **Impact:** **Low** — bounded by processor count, happens at most once per
  processor per cell, and is a first-insert cost. Deprioritised per
  `docs/performance.md`.
- **Remediation:** Pre-size the vector to the `many_cpus`-reported processor count at
  construction, so growth never happens after the first use. One line, but it moves
  cost from steady state to construction, which is only a win if cells are long-lived
  — which they are. Optional.
- **Evidence:** inferred from code reading.

#### TA9. `count_where` iterates every slot under the lock

- **Location:** `crates/thread_aware/src/cell/storage.rs` (`count_where`), called from
  `crates/thread_aware/src/cell/mod.rs:571-576`.
- **Issue:** Takes a predicate by value and performs a full O(slots) scan under the
  read lock. Same shape as **TA3** and shares its cost.
- **Impact:** **Low** — subsumed by **TA3**; listed separately because it is the
  reusable primitive and any fix to **TA3** should address it here.
- **Remediation:** See **TA3**.
- **Evidence:** inferred from code reading.

### Benchmark coverage

`crates/thread_aware/benches/` contains `criterion_third_party.rs` and
`gungraun_third_party/main.rs`. This is the **only** crate in the cluster with any
Callgrind coverage — but both files benchmark the *third-party `ThreadAware` trait
impls* (`bytes`, `http`, `jiff`, `uuid`), which are thin `relocate` forwarding
shims, and neither touches the crate's own `Arc`, `Storage`, `Factory` or `closure`
machinery.

So the situation is: the crate's least interesting code is the only code that is
measured, and the smart pointer that three sibling crates put on their per-operation
paths has **zero** benchmark coverage of any kind. Findings **TA1, TA2, TA3, TA5** —
every High and Medium in this crate — are unmeasured.

Both `[[bench]]` entries carry `required-features = ["bytes", "http", "jiff02",
"uuid"]`, so they do not even run in a default-feature build.

Nothing here is multithreaded, so **TA5** (the write-lock-for-a-read in `relocate`)
is structurally unobservable.

Recommended additions, in priority order:

- `crates/thread_aware/benches/arc_cg.rs` (Callgrind) with scenarios
  `clone_data_factory`, `clone_closure_factory`, `clone_erased_clone_fn`,
  `deref`, `strong_count` → confirms **TA2**, **TA3**, **TA4** in exact instruction
  and `lock`-prefix counts. Highest value in the crate; Callgrind is uniquely suited
  because atomic RMW counts are exactly what it reports deterministically.
- Its Criterion pair `crates/thread_aware/benches/arc.rs` (required by
  `docs/naming.md`'s pairing convention), group `arc/clone`, `arc/deref`.
- `crates/thread_aware/benches/arc_relocate.rs` using `bench_on_threadpool()` →
  confirms **TA5**.
- Layout guards (`const _: () = assert!(size_of::<…>() == …)`) for `Arc`, `Factory`
  and `Affinity` so **TA1** cannot silently regress.

### Considered and ruled out

- **Giving `Affinity` a niche to shrink `Factory`.** Tested empirically with the
  layout replica: adding a niche takes `Option<Affinity>` from 10 to 8 bytes but
  leaves `Factory` at 32, because the `ErasedCloneFn` variant (24 bytes + tag +
  padding) sets the width. **Do not report this as a fix** — it does not do what it
  appears to.
- **Atomic ordering.** No `SeqCst` in production code anywhere in the crate; the
  orderings in `storage.rs` and `mod.rs` are appropriate.
- **`ThreadRegistry::current_affinity`** (`crates/thread_aware/src/registry.rs:153-159`)
  is a global `Mutex<HashMap<ThreadId, Affinity>>` lookup, which looked alarming.
  Traced its callers: it is not reached from `Arc::clone`, `Deref` or any
  per-operation path — only from explicit registration and from `threads`-feature
  helpers. Not a hot-path concern.
- **`wrappers.rs`** — the `Manual`/`Data` wrapper types are trivially forwarding, no
  allocation, no locking.
- **`impls.rs` / `third_party/`** — blanket and third-party `ThreadAware` impls are
  no-op or single-field `relocate` forwards. Nothing to optimise (and these are
  the only things currently benchmarked).
- **`no_std` support.** The `std` feature gating is clean; the `alloc`-only path
  does not add indirection to the hosted path.

---

## Crate: thread_aware_macros

### Summary

A 27-line facade crate that re-exports the derive macro from
`thread_aware_macros_impl`. There is nothing here to make faster at runtime, and its
compile-time cost is a single extra (trivial) crate in the graph, which is the
standard ecosystem pattern for separating a `proc-macro` crate from its
implementation.

### Findings

#### MA1. Facade adds one proc-macro crate to the build graph

- **Location:** `crates/thread_aware_macros/src/lib.rs:1-27`.
- **Issue:** The crate exists only to forward `#[derive(ThreadAware)]` to
  `thread_aware_macros_impl`. Downstream builds therefore compile two crates instead
  of one, and proc-macro crates are compiled for the *host* even when
  cross-compiling, so they sit on the critical path of the build graph.
- **Impact:** **Low** — this is the deliberate and idiomatic split (it lets the
  implementation be unit-tested as a normal library, which
  `thread_aware_macros_impl` does extensively). The cost is one small crate
  compilation, amortised across the whole workspace build. Reported for completeness
  only; **no change recommended**.
- **Remediation:** None. Deviating from the ecosystem pattern here would cost
  testability for a negligible build-time gain, and `docs/performance.md` asks that
  deviations from standard ecosystem patterns be justified — there is no
  justification available.
- **Evidence:** inferred from code reading.

Otherwise: **no performance issues found.**

### Benchmark coverage

**None**, and correctly so — the crate contains no runtime code. Benchmarking a
facade would measure nothing. The compile-time cost of the *generated* code is
properly the concern of `thread_aware_macros_impl` (see **MI1**–**MI3**) and of the
consuming crates.

### Considered and ruled out

- **Runtime cost** — the crate emits no runtime code of its own; all generated code
  originates in `thread_aware_macros_impl`.
- **`proc-macro2`/`syn`/`quote` dependency weight** — declared by the `_impl` crate,
  not here.

---

## Crate: thread_aware_macros_impl

### Summary

The real derive implementation: `struct_gen.rs` and `enum_gen.rs` generate
`ThreadAware::relocate` bodies. "Performance" for a proc-macro crate means (a) the
compile-time cost it imposes on every downstream crate that derives, and (b) the
quality of the code it emits.

**(b) is good** — the generated `relocate` is fully statically dispatched, allocates
nothing, and passes `source`/`destination` (both `Copy`) by value. There is no
runtime overhead to speak of.

**(a) has one clear, trivially fixable defect**: the generators re-run the `syn`
parser for a *constant* identifier once per field, and clone the crate root path once
per field, inside the field loop.

### Findings

#### MI1. `parse_quote!` and a path clone are executed once per field, inside the loop

- **Location:** `crates/thread_aware_macros_impl/src/struct_gen.rs:17-18` and
  `:30-31`; `crates/thread_aware_macros_impl/src/enum_gen.rs:25-26` and `:40-41`.
- **Issue:** Inside the per-field loop, each generator performs
  `root_path.clone()` and `syn::parse_quote!(ThreadAware)`. `parse_quote!` is not a
  cheap macro — it constructs a `TokenStream` and runs the full `syn` parser to
  produce a `syn::Path`. Doing that for the string `ThreadAware`, which is constant
  across every field of every type, means a struct with 20 fields runs the `syn`
  parser 20 times to produce 20 identical `Path` values, and clones the crate-root
  `Path` 20 times.
- **Impact:** **Medium** (compile-time) — proc-macro expansion time is paid by every
  downstream crate on every build, including incremental ones, and `syn` parsing is
  among the more expensive things a derive can do. The absolute number is small per
  type, but it scales with (types × fields) across the entire workspace and every
  consumer of `thread_aware`, and it buys literally nothing.
- **Remediation:** Hoist both out of the loop:
  ```rust
  let trait_path: syn::Path = parse_quote!(ThreadAware);   // once
  let root = root_path.clone();                             // once
  for field in fields { … use &trait_path, &root … }
  ```
  Two lines moved in each of two files. This is as surgical as an optimisation gets.
- **Evidence:** inferred from code reading (the `parse_quote!` call sites are inside
  the `for` bodies at the cited lines). **Confirming benchmark:** `cargo build
  --timings` on a synthetic crate deriving `ThreadAware` on a 100-field struct,
  before and after; or `-Zself-profile` attributing time to `expand_proc_macro`.

#### MI2. `collect_generics_in_fields` rebuilds its `HashSet` per enum variant

- **Location:** `crates/thread_aware_macros_impl/src/lib.rs:82-116` (`add_bounds`),
  with the per-variant rebuild at `:87-93` and the call site at `:121-128`.
- **Issue:** `add_bounds` constructs a `HashSet` of the type's generic identifiers
  and then scans fields against it. For an enum, this is invoked per variant, so the
  identifier set — which depends only on the type's generics, not on the variant — is
  allocated and populated once per variant.
- **Impact:** **Low** (compile-time) — enums with many variants are less common than
  structs with many fields, and the set is small. Same class of waste as **MI1** but
  smaller; fix it in the same commit.
- **Remediation:** Build the `HashSet` once in the enum entry point and pass it by
  reference into `add_bounds`.
- **Evidence:** inferred from code reading. **Confirming benchmark:** as **MI1**,
  with a 50-variant enum.

#### MI3. Generated code is sound; one minor emission redundancy

- **Location:** `crates/thread_aware_macros_impl/src/enum_gen.rs:20-60`.
- **Issue:** The enum generator always emits a `match self { … }` over all variants,
  including for enums where every variant is a unit variant or is skipped — in which
  case the generated `relocate` is a match that does nothing. The optimiser will
  remove it, so this costs compile time (a larger token stream to parse and codegen)
  rather than run time.
- **Impact:** **Low** — an empty `match` on a discriminant is free at run time after
  optimisation; the cost is a marginally larger token stream. Reported for
  completeness. On the positive side, and worth recording explicitly: the generated
  code performs **no allocation, no dynamic dispatch and no locking**, and
  `source`/`destination` are `Copy`, so there is no per-call cost beyond the field
  recursion itself. This is the right shape.
- **Remediation:** Emit an empty body when no variant has a non-skipped
  `ThreadAware` field. Optional; low value.
- **Evidence:** inferred from code reading.

### Benchmark coverage

**None.** There is no benchmark of any kind for this crate — neither Criterion nor
Callgrind — which is expected for a proc-macro crate, since neither harness is
designed to measure expansion time.

That said, the crate's cost is *real* and currently ungated: **MI1** could regress
arbitrarily far without anyone noticing. The appropriate instrument is not Criterion
but a `cargo build --timings` or `-Zself-profile` check on a synthetic
many-field/many-variant crate, run manually when the generators change. Worth a note
in the crate's docs rather than a benchmark file.

The *generated* code's runtime cost is, in principle, covered by whatever
benchmarks exist for the types that derive `ThreadAware` — which in practice means
`crates/thread_aware/benches/criterion_third_party.rs`, and those types implement
the trait by hand rather than deriving it. So the derive's output is unbenchmarked
too. Low priority, since the output is trivially optimal.

### Considered and ruled out

- **`syn`/`quote`/`proc-macro2` as dependencies** — these are the universal
  ecosystem standard for derives; `docs/performance.md` asks for justification when
  deviating from ecosystem patterns, and there is no case for deviating here.
  Using `syn` with narrowed features would be the only lever, and the crate already
  does not enable `full` unnecessarily.
- **Recursion depth in field traversal** — bounded by type nesting depth, not a
  concern.
- **Generated runtime dispatch** — checked and confirmed static. No `dyn`, no
  boxing, no `Arc` in emitted code.

---

## Crate: anyspawn

### Summary

`anyspawn` abstracts task spawning over Tokio, smol and arbitrary custom spawners.
The design is honest — the docs already say the custom path is slower than the
native one — but the *magnitude* is larger than the docs suggest, and it is
unmeasured.

The core issue is that the custom-spawner path unconditionally creates a
`futures_channel::oneshot` channel per spawn (a heap allocation plus two atomics)
**even when the caller drops the `JoinHandle` immediately**, i.e. even in
fire-and-forget mode, which is the majority of spawns in a server. On top of that
it boxes the future, hops through a `dyn` call, and — because `CustomSpawner` holds
its spawn function in a `thread_aware::Arc<dyn SpawnCustom, PerCore>` built with
`with_clone_fn` — pays the **worst-case 4-atomic-RMW clone** from **TA2** every time
a `Spawner` is cloned.

### Findings

#### AS1. Custom spawn allocates a oneshot channel per task, even fire-and-forget

- **Location:** `crates/anyspawn/src/custom.rs:94-101` (`CustomSpawner::spawn`, the
  `oneshot::channel()` at `:95` and the `Box::pin` at `:96`),
  `crates/anyspawn/src/handle.rs:22-26` (the `JoinHandle` wrapping the receiver),
  contrasted with the native Tokio path at
  `crates/anyspawn/src/spawner.rs:209-221`, which just hands back Tokio's own handle.
- **Issue:** Every custom spawn does:
  1. `oneshot::channel()` — one heap allocation for the shared channel state, plus
     atomics on sender/receiver registration and on `send`.
  2. `Box::pin(async move { let _ = tx.send(work.await); })` — a second heap
     allocation wrapping the caller's future.
  3. A virtual call through `dyn SpawnCustom`.
  4. Whatever the underlying spawner then does (Tokio's own allocation, typically).

  Crucially, step 1 happens unconditionally. A caller that writes
  `spawner.spawn(fut);` and discards the returned handle — the fire-and-forget shape
  that dominates in servers — still pays for a channel that will never be read. The
  native Tokio path (`spawner.rs:209-221`) does not, so the two backends have
  materially different cost profiles behind one API.
- **Impact:** **High** — this is the crate's single per-operation cost and it is
  roughly 2–3× the allocation count of a bare `tokio::spawn`. Spawn is by definition
  a per-unit-of-work operation, so a server spawning per request pays it per request.
  `docs/performance.md` names "no allocation on the hot path" explicitly; this path
  has two, one of which is pure waste in the common case.
- **Remediation:** Surgical: make the channel lazy. Return a `JoinHandle` enum with a
  `Detached` variant and only allocate the oneshot when the caller actually holds
  onto the handle — or, simpler and fully compatible, add a `spawn_detached` method
  that skips the channel entirely and have the docs steer fire-and-forget callers to
  it. The second option is a purely additive API change with no risk.
- **Evidence:** inferred from code reading (allocation sites cited above are
  explicit). **Confirming benchmark:**
  `crates/anyspawn/benches/spawner_cg.rs` (Callgrind), scenarios
  `spawner_cg::spawn_tokio_native`, `spawner_cg::spawn_custom`,
  `spawner_cg::spawn_custom_detached`. Callgrind is the right tool because the
  finding is about a *count* of allocations, which it reports deterministically,
  whereas the existing wall-clock benchmark is dominated by runtime scheduling noise.
#### AS2. `CustomSpawner` uses the 4-atomic-RMW `thread_aware::Arc` variant

- **Location:** `crates/anyspawn/src/custom.rs:84-87` (the struct: `spawn:
  thread_aware::Arc<dyn SpawnCustom, PerCore>`), `:90-93` (`new`, which calls
  `thread_aware::Arc::with_clone_fn`), plus
  `crates/thread_aware/src/cell/clone_fn.rs:70-77`.
- **Issue:** `CustomSpawner::new` builds the cell with `with_clone_fn`, which
  produces a `Factory::ErasedCloneFn`. Per **TA2**, that is the *most expensive*
  factory variant to clone: `ErasedCloneFn::clone` itself clones two `Arc`s, so a
  `CustomSpawner` clone is **4 atomic RMWs**. `CustomSpawner` derives `Clone`, and
  `Spawner` is cloned freely by consumers (it is the idiomatic way to hand a spawner
  to a task). Additionally, because `T = dyn SpawnCustom` is unsized, the cell is the
  64-byte variant from **TA1**, so `CustomSpawner` is ~72 bytes and `Spawner`
  correspondingly large.
- **Impact:** **High** — the per-clone cost is 4× what a `std::sync::Arc<dyn
  SpawnCustom>` would cost, on a type explicitly designed to be cloned and passed
  around. And it is not obvious: nothing at the `anyspawn` call site suggests
  `with_clone_fn` selects a more expensive path than the alternatives.
- **Remediation:** Two surgical options:
  1. If per-core storage of the spawn function is not actually needed — and it is
     hard to see why it would be, since the function is stateless dispatch — use a
     plain `std::sync::Arc<dyn SpawnCustom>`: 8 bytes, 1 RMW.
  2. If per-core storage *is* wanted, construct with `with_closure_boxed` instead of
     `with_clone_fn` to get `Factory::Closure` (3 RMWs rather than 4), or apply
     **TA2**'s remediation 2 in `thread_aware`.
  Option 1 is a two-line change and should be evaluated first.
- **Evidence:** inferred from code reading; the underlying RMW counts and type sizes
  are traced through `thread_aware` and the sizes are empirically verified.
  **Confirming benchmark:** `spawner_cg.rs` scenario `spawner_cg::clone_spawner`,
  comparing `Spawner::new_tokio()` (plain handle) against a custom spawner.

#### AS3. `spawn_anywhere` on the custom path costs three allocations

- **Location:** `crates/anyspawn/src/custom.rs:103-114` (`spawn_anywhere`: the
  `oneshot::channel()` at `:109` and the `Box::new(SpawnAnywhereTask{…})` at `:110`),
  plus `crates/anyspawn/src/custom.rs:75-81`
  (`SpawnAnywhereTask::call_once`, which does a further `Box::pin`).
- **Issue:** `spawn_anywhere` allocates the oneshot channel, then boxes the
  `SpawnAnywhereTask`, and later — when the task is actually invoked — `call_once`
  boxes the async block into a `BoxFuture`. Three heap allocations per
  affinity-agnostic spawn, before the underlying runtime's own.
- **Impact:** **Medium** — `spawn_anywhere` is the less common entry point (it exists
  for `ThreadAware` payloads that may be relocated), so the frequency is lower than
  **AS1**'s. But three allocations for one logical spawn is a lot, and the third is
  forced by the `ThreadAwareAsyncFnOnce` trait returning `BoxFuture`, which is a
  design decision worth revisiting.
- **Remediation:** The `Box::new(SpawnAnywhereTask)` at `:110` is avoidable — the
  task could be constructed in place inside the boxed future rather than boxed
  separately and then boxed again. That collapses two allocations to one and is
  contained to `custom.rs`. The channel is **AS1**'s concern.
- **Evidence:** inferred from code reading. **Confirming benchmark:** `spawner_cg.rs`
  scenario `spawner_cg::spawn_anywhere_custom`, counted against
  `spawner_cg::spawn_custom`.

#### AS4. Every builder layer adds a boxed wrapper per spawn

- **Location:** `crates/anyspawn/src/builder.rs:56-65` (the `LayeredTask` wrapping in
  the layer-application loop).
- **Issue:** `CustomSpawnerBuilder` composes layers by wrapping each task in a
  `LayeredTask`, boxed. With N layers configured, every `spawn_anywhere` allocates N
  additional `Box`es and performs N additional virtual calls before the task body
  runs. The cost is linear in the number of layers and paid per task, not per
  builder.
- **Impact:** **Medium** — layers are configured once but charged per spawn, so a
  three-layer spawner triples the wrapper allocation count on an already
  three-allocation path (**AS3**). Anyone using layers for tracing/context
  propagation — which is the documented use case, cf. the `otel_context` example —
  is on this path for every task.
- **Remediation:** Compose the layers into a single boxed closure **at build time**
  rather than per spawn: the layer stack is known when the builder is finalised, so
  the N-deep wrapper can be constructed once and stored, leaving one allocation per
  spawn regardless of layer count. This is the standard "compose at build, not at
  call" transformation and is contained to `builder.rs`.
- **Evidence:** inferred from code reading. **Confirming benchmark:** `spawner_cg.rs`
  scenario family `spawner_cg::spawn_custom_layers_{0,1,3}` — instruction count should
  be flat in layer count after the fix and linear before it.

#### AS5. Zero `#[inline]` across 12 public functions

- **Location:** whole crate; notably `crates/anyspawn/src/handle.rs` (`JoinHandle`'s
  `poll`, a thin forward to the oneshot receiver) and
  `crates/anyspawn/src/spawner.rs` accessors.
- **Issue:** 0 `#[inline]` / 12 `pub fn`. Most of these are generic and therefore
  get monomorphised into the caller anyway, which blunts the finding considerably —
  but `JoinHandle::poll` and the non-generic accessors are not.
- **Impact:** **Low** — mostly mooted by monomorphisation; `docs/performance.md` asks
  for judiciousness, and blanket annotation here would be unjustified.
- **Remediation:** Consider `#[inline]` only on `JoinHandle`'s `Future::poll` and
  `Spawner::name()`. Nothing else.
- **Evidence:** empirically verified by grep census (0/12); consequence inferred.

#### AS6. `tokio` backend is an optional feature, so the fast path is opt-in

- **Location:** `crates/anyspawn/Cargo.toml` (`[features] default = []`,
  `tokio = ["dep:tokio"]`).
- **Issue:** The crate's default feature set is empty, so a consumer who adds
  `anyspawn` without `features = ["tokio"]` gets *only* the custom-spawner path — the
  slow one (**AS1**–**AS4**) — even when running on Tokio. Nothing warns them.
- **Impact:** **Low** — it is a documentation/ergonomics issue rather than a code
  defect, and defaulting to no runtime is the correct choice for a runtime-agnostic
  crate. Recorded because the performance delta between the two paths is large enough
  (see **AS1**) that silently landing on the slow one matters.
- **Remediation:** Document the cost difference prominently in the crate-level docs
  (the `custom.rs` docs already hint at it; the crate root does not).
- **Evidence:** read from `Cargo.toml`.

### Benchmark coverage

`crates/anyspawn/benches/` contains one file, `spawner.rs`, gated on
`required-features = ["tokio"]`. There is **no Callgrind coverage**.

Problems:

1. **The measured region includes `rt.block_on(…)`.** The benchmark spawns a task
   and blocks on its completion inside `iter()`, so each sample measures runtime
   entry, scheduling, a thread handoff and a wakeup — all of which dwarf the spawn
   cost being compared. A regression that doubled `CustomSpawner::spawn`'s
   allocation count could easily be invisible in this noise.
2. **It compares tokio-direct against tokio-via-spawner**, which is the right
   comparison in principle, but at this signal-to-noise ratio it cannot resolve the
   ~2 allocations that separate them.
3. **No fire-and-forget scenario**, so **AS1** — the finding that the channel is
   allocated even when unused — is entirely unmeasured.
4. **No layered-spawner scenario**, so **AS4** is unmeasured.
5. **No `Spawner::clone` scenario**, so **AS2** is unmeasured.
6. **Nothing is multithreaded** (in the contention sense — `block_on` on a
   multi-thread runtime exercises the scheduler but not this crate's own
   synchronisation).

This crate is the clearest case in the cluster for Callgrind: every finding here is
about a *count* (allocations, atomic RMWs, virtual calls), which is exactly what
`docs/callgrind-benchmarks.md` says instruction-count benchmarking is for, and
exactly what a wall-clock benchmark of an async spawn cannot resolve.

Recommended addition: `crates/anyspawn/benches/spawner_cg.rs`, paired with the
existing `spawner.rs` per `docs/naming.md`, with scenarios `spawn_tokio_native`,
`spawn_custom`, `spawn_custom_detached`, `spawn_anywhere_custom`, `clone_spawner`,
`spawn_custom_layers_{0,1,3}`. That one file would make **AS1** through **AS4**
measurable.

### Considered and ruled out

- **Atomic ordering.** No `SeqCst` in production code.
- **`futures-channel` as a dependency.** It is declared with
  `default-features = false, features = ["alloc"]`, which is the minimal correct
  choice. The channel *usage* is the issue (**AS1**), not the dependency.
- **`SpawnCustom` dynamic dispatch.** One virtual call per spawn, next to two heap
  allocations and a runtime scheduling decision. Immaterial, and it is what makes the
  abstraction possible. Not worth removing.
- **`smol` support.** Handled through the same custom path; no separate findings.
- **`JoinHandle` size.** It wraps a `oneshot::Receiver`, which is pointer-sized.
  Fine.

---

## Crate: anyspawn_azure

### Summary

A small adapter implementing `azure_core`'s async runtime traits on top of
`anyspawn` + `tick`. Everything in it is on the Azure SDK's per-request path.

Its cost profile is largely **forced by the `azure_core` trait signatures**, which
return `Pin<Box<dyn Future>>` — so boxing per operation is not a choice this crate
makes. What *is* this crate's choice is the extra work layered on top: cloning a
whole `tick::Clock` per `sleep`, heap-allocating a one-byte `YieldNow` future per
`yield_now`, and stacking `AbortHandle`/`Abortable` on top of `anyspawn`'s own
oneshot channel so that a single `spawn` costs roughly four allocations.

The crate has **no benchmarks of any kind**.

### Findings

#### AZ1. `sleep` clones a whole `Clock` and boxes, per call

- **Location:** `crates/anyspawn_azure/src/runtime.rs:75-82`.
- **Issue:** The `sleep` implementation clones `self.clock` — which per **T4**/**T10**
  is a ~56-byte struct whose clone costs 3–4 atomic RMWs — and then `Box::pin`s the
  resulting delay future. The clone is needed only because the returned boxed future
  must be `'static`, but the `Clock` handle is already owned by `self`, which
  outlives the call in every realistic usage.
- **Impact:** **Medium** — the Azure SDK calls `sleep` on every retry backoff and
  every poll interval, so this is on the retry path of every Azure operation.
  3–4 atomic RMWs plus a heap allocation for what is semantically "wait a bit".
- **Remediation:** The `Box::pin` is forced by the trait. The `Clock` clone is not
  entirely: storing an `Arc<Clock>` in `Runtime` and cloning *that* would reduce it
  to one atomic RMW. Alternatively, fixing **TA2** in `thread_aware` removes 2 of the
  RMWs with no change here.
- **Evidence:** inferred from code reading. **Confirming benchmark:** a new
  `crates/anyspawn_azure/benches/runtime_cg.rs`, scenario `runtime_cg::sleep_create`
  (construct the future without polling it to completion) — instruction count makes
  the RMWs visible.

#### AZ2. `yield_now` heap-allocates a one-byte future per call

- **Location:** `crates/anyspawn_azure/src/runtime.rs:84-86`.
- **Issue:** `yield_now` returns `Box::pin(YieldNow { yielded: false })`. `YieldNow`
  is a single `bool`. The heap allocation therefore costs more than the entire
  operation it wraps — a malloc/free pair (~50–100 cycles plus allocator lock
  contention) to hold one byte and yield once.
- **Impact:** **Medium** — `yield_now` exists precisely to be called in tight loops
  (cooperative yielding during long synchronous stretches), so the frequency can be
  very high. This is the clearest "allocation on the hot path" violation of
  `docs/performance.md` in the crate.
- **Remediation:** The `Pin<Box<…>>` return type is mandated by `azure_core`, so the
  allocation cannot simply be removed. But it can be made nearly free: keep a
  pre-allocated `Box` per `Runtime`... no — the future is consumed. The realistic
  surgical option is to note that `Box::pin` of a ZST-adjacent type is already
  special-cased for true ZSTs by the allocator, and make `YieldNow` a genuine ZST by
  encoding the `yielded` state in the `Pin<Box<>>` — or, more practically, to accept
  the allocation and document it, steering callers away from tight `yield_now` loops.
  **Report, do not force a fix**: the constraint is external.
- **Evidence:** inferred from code reading. **Confirming benchmark:**
  `runtime_cg::yield_now` — a single allocation/deallocation pair should be plainly
  visible.

#### AZ3. `spawn` costs roughly four heap allocations per Azure SDK task

- **Location:** `crates/anyspawn_azure/src/runtime.rs:62-73`.
- **Issue:** Per spawn: `AbortHandle::new_pair()` (an `Arc<AbortInner>` allocation),
  the `Abortable` wrapper, `Box::pin` of the resulting `RuntimeTask`, and then
  `anyspawn`'s own `oneshot::channel()` + `Box::pin` from **AS1**. That is roughly
  four heap allocations and several atomics for one task, plus an abort check on
  every poll of the wrapped future for the lifetime of the task.
- **Impact:** **Medium** — the Azure SDK spawns per long-running operation rather
  than per request, so the frequency is lower than **AS1**'s; but the constant factor
  is the highest in the cluster, and the `Abortable` poll-time check is paid on
  every single poll of every spawned task, not just at spawn.
- **Remediation:** If `azure_core` does not require abort semantics on every spawned
  task, only wrap in `Abortable` where cancellation is actually used. Otherwise
  **AS1**'s lazy-channel fix removes one of the four allocations from underneath
  with no change here.
- **Evidence:** inferred from code reading. **Confirming benchmark:**
  `runtime_cg::spawn` against `anyspawn`'s `spawner_cg::spawn_custom` — the delta
  isolates this crate's contribution.

#### AZ4. Depends on the full `futures` facade for two types

- **Location:** `crates/anyspawn_azure/Cargo.toml`
  (`futures = { workspace = true, features = ["std"] }`).
- **Issue:** The crate uses `futures` only for `AbortHandle` / `Abortable`, both of
  which live in `futures-util`. Depending on the `futures` facade with `std` pulls in
  `futures-executor`, `futures-io`, `futures-sink` and `futures-task` as well.
- **Impact:** **Low** — no run-time cost (unused code is not linked), but it is
  compile-time cost for every consumer and it widens the crate's public dependency
  surface for no benefit.
- **Remediation:** Replace with
  `futures-util = { workspace = true, default-features = false, features = ["std"] }`.
  One line. Note the rest of the workspace already prefers `futures-util` directly
  (e.g. `uniflight`), so this is also a consistency fix.
- **Evidence:** read from `Cargo.toml` and the crate's `use` statements.

#### AZ5. No `#[inline]`, no benchmarks, on a per-request adapter

- **Location:** whole crate.
- **Issue:** 0 `#[inline]` attributes; the crate is a thin adapter whose methods are
  small forwards, which is the archetypal `#[inline]` case. And there is no
  benchmark of any kind, so **AZ1**–**AZ3** have never been measured.
- **Impact:** **Low** individually — the boxed-future indirection dominates any
  call overhead, so inlining the forwards buys little here. The *absence of
  benchmarks* is the more significant half and is treated under "Benchmark
  coverage".
- **Remediation:** Low priority. Prioritise the benchmark file over the attributes.
- **Evidence:** empirically verified by grep census (0 `#[inline]`, 0 bench files).

### Benchmark coverage

**Zero.** There is no `benches/` directory, no Criterion benchmark, no Callgrind
benchmark. Every finding in this section is therefore unmeasured, and the crate sits
on the Azure SDK's per-request path.

This is the largest single coverage gap in the cluster relative to the crate's
position in the stack: `tick` and `anyspawn` at least have *something*.

Recommended addition: `crates/anyspawn_azure/benches/runtime_cg.rs` with scenarios
`sleep_create`, `yield_now`, `spawn`, and its Criterion pair
`crates/anyspawn_azure/benches/runtime.rs` with groups `runtime/sleep`,
`runtime/yield_now`, `runtime/spawn` (pairing required by `docs/naming.md`).
Callgrind is the right primary instrument because all three findings are allocation
counts.

### Considered and ruled out

- **Boxed futures in the trait implementations.** Forced by `azure_core`'s
  `TaskFuture = Pin<Box<dyn Future + Send>>` signature. This crate cannot avoid them
  and should not be criticised for them. Recorded here so that the boxing is not
  re-raised as a finding by a future reviewer.
- **`async-trait` dependency.** Optional, behind the `azure-identity` feature only,
  and required by the upstream trait. Correct.
- **Atomic ordering.** No `SeqCst`; nothing to tighten.
- **`azure_identity` subprocess execution.** Credential acquisition is a
  once-per-process cold path; explicitly deprioritised per `docs/performance.md`.
---

## Crate: uniflight

### Summary

`uniflight` coalesces duplicate concurrent async work: N callers asking for the same
key run the work once and all receive a clone of the result. It is the crate in this
cluster with the sharpest gap between stated purpose and implementation, and it
yields the highest-value finding in the whole assignment.

Three problems compound on the single hot function, `Merger::execute`
(`crates/uniflight/src/lib.rs:350-372`):

1. **Every completion takes a DashMap shard *write* lock** (`:370`). When a
   coalesced group of N followers becomes ready — which happens to all of them at
   once, by construction — all N converge on the same shard's write lock
   simultaneously. The crate's headline workload is precisely the workload that
   serialises it.
2. **Every call allocates a `String`** (`:361`), and cold calls allocate two
   (`:409`), despite the crate's own documentation presenting `&str` lookup as the
   allocation-avoiding path.
3. **Every follower constructs and boxes a future it will never poll** (`:366`).
   `func()` is called and `Box::pin`ned by *all* N callers; `get_or_init` then throws
   N−1 of them away. So the coalescing mechanism itself allocates N boxed futures to
   run one.

A single Callgrind benchmark of one uncontended `Merger::execute` would expose
findings **U1**, **U2**, **U3** and **U5** simultaneously.

### Findings

#### U1. Every completion takes a DashMap shard write lock

- **Location:** `crates/uniflight/src/lib.rs:370` (`inner.remove_if(owned_key.borrow(),
  |_, weak| weak.upgrade().is_none())`), within `execute` at `:350-372`.
- **Issue:** After awaiting the result, *every* caller — leader and all followers —
  calls `DashMap::remove_if`. `remove_if` must take the shard's **write** lock,
  because it may mutate the map, even though in the overwhelmingly common case it
  changes nothing (the leader's or another follower's `Arc` is still alive, so
  `weak.upgrade()` succeeds and the entry stays).

  The pathology is that all N followers of a coalesced key become ready *at the same
  instant* — that is what coalescing means — and they all hash to the *same shard*,
  because they share the key. So N threads simultaneously request an exclusive lock
  on one shard, and they serialise. The higher the coalescing factor, the worse it
  gets: exactly inverted from the property the crate exists to provide.

  Worse, `remove_if`'s closure calls `weak.upgrade()`, a compare-and-swap loop on the
  `PanicAwareCell`'s refcount, and then drops the resulting `Arc` — two more atomics
  per follower, executed *inside* the write-locked critical section.
- **Impact:** **High** — this is the top finding for the crate and, arguably, for the
  cluster. It converts the crate's best case (high coalescing) into its worst case
  for lock contention, and it does so on the exit path where the caller is expecting
  to be handed a ready value.
- **Remediation:** Surgical and contained to `execute`. Only the **last** holder
  should attempt removal. The cleanest form: keep the strong `Arc<PanicAwareCell>`,
  and after `drop(cell)` check whether we were the last strong reference — e.g. use
  `Arc::try_unwrap` / `Arc::into_inner` semantics, or track strong count before the
  drop and only call `remove_if` when it was 1. Followers that were not last skip the
  write lock entirely. That reduces write-lock acquisitions from N per group to 1.

  A cheaper variant, if exactness is not required: attempt removal only
  probabilistically or on a periodic sweep, since a stale `Weak` entry is harmless —
  `get_or_create_cell` already handles expired entries (`:397-423`). Entries are
  self-healing, so eager removal is not required for correctness at all, which makes
  this a very low-risk change.
- **Evidence:** inferred from code reading; the DashMap `remove_if` write-lock
  requirement is a documented property of the crate. **Confirming benchmark:** two
  things. (a) `crates/uniflight/benches/performance_cg.rs` (Callgrind, paired with
  the existing `performance.rs`), scenario
  `performance_cg::execute_uncontended_warm` — shows the write lock's cost even
  single-threaded. (b) A multithreaded Criterion group
  `performance/coalesced_followers_{2,8,32,128}` using `bench_on_threadpool()`,
  measuring time-to-last-follower. If U1 is real, per-follower latency grows with N;
  after the fix it should be flat.

#### U2. A `String` is allocated on every call, and twice on cold keys

- **Location:** `crates/uniflight/src/lib.rs:361` (`let owned_key = key.to_owned();`),
  `crates/uniflight/src/lib.rs:409` (`match map.entry(key.to_owned())` inside
  `insert_or_get_existing`). Contradicted documentation at
  `crates/uniflight/src/lib.rs:41-56` and the `execute` doc comment at `:330-349`.
- **Issue:** `execute` takes `key: &Q` where `Q: ToOwned<Owned = K>`, i.e. `&str` for
  a `String`-keyed merger — a signature that promises borrowed lookup. It then
  immediately calls `key.to_owned()` at `:361`, unconditionally, on *every* call
  including the warm hit where the key is already in the map. That is one heap
  allocation plus a memcpy per operation, on a path whose entire purpose is to avoid
  redundant work.

  The owned key exists solely to be passed to `remove_if` at `:370` — i.e. the
  allocation exists to serve the operation identified in **U1**. Fixing **U1** by
  making removal rare would let the `to_owned()` become conditional too, and the two
  fixes reinforce each other.

  On the cold path there is a *second* allocation: `insert_or_get_existing` does
  `map.entry(key.to_owned())` at `:409`. So a first-touch of a key allocates the key
  string twice.
- **Impact:** **High** — `docs/performance.md` states the no-allocation-on-the-hot-path
  rule plainly, and this is an unconditional allocation on the single hottest
  function in the crate. It also makes the crate's documented value proposition
  ("pass a `&str`, avoid the allocation") false as written.
- **Remediation:**
  1. Make the `to_owned()` at `:361` lazy — move it inside the branch that actually
     needs it once **U1** makes removal conditional. This is the ideal fix and
     removes the allocation from the warm path entirely.
  2. Independently, remove the double allocation at `:409` by hoisting the owned key
     from `execute` into `insert_or_get_existing` (pass `K` rather than `&Q`), so a
     cold call allocates once rather than twice.
  Both are contained to `lib.rs` and neither changes the public API.
- **Evidence:** inferred from code reading; both `to_owned()` call sites read
  directly. **Confirming benchmark:**
  `crates/uniflight/benches/performance_cg.rs` scenarios
  `execute_uncontended_warm` (should show zero allocations after the fix) and
  `execute_cold` (should show one, not two). Also worth adding
  `alloc_tracker`-based assertions in the Criterion bench, following the pattern
  already used by `crates/layered/benches/dynamic.rs`, which is the best-designed
  benchmark in this cluster in that respect.

#### U3. Every follower builds and boxes a future that is immediately discarded

- **Location:** `crates/uniflight/src/lib.rs:364-367` (the comment justifying the
  boxing, and `let boxed = Box::pin(func());` at `:366`, followed by
  `cell.get_or_init(boxed).await`).
- **Issue:** All N callers of a coalesced key evaluate `func()` — constructing the
  work future, which may itself capture and allocate — and then `Box::pin` it, one
  heap allocation each. `async_once_cell`'s `get_or_init` then initialises with the
  first arrival's future and **drops the other N−1 boxed futures unpolled**.

  So coalescing N duplicate calls into one execution costs N future constructions and
  N heap allocations, of which N−1 are pure waste. The crate saves the *expensive*
  work (that is the point, and it is a real win), but it does not save the setup, and
  the setup cost is proportional to the coalescing factor.

  The inline comment at `:364-365` justifies the `Box::pin` as keeping the state
  machine small, which is a legitimate and deliberate trade — that part is fine and
  is not being second-guessed here. The issue is that it is paid by callers who will
  never use the result of the boxing.
- **Impact:** **High** — it scales with exactly the parameter the crate is designed
  to make large. At a coalescing factor of 100, 99 heap allocations and 99 future
  constructions are thrown away per group.
- **Remediation:** Check the cell's initialised state *before* calling `func()`. The
  `PanicAwareCell` (`:477-489`) can expose a cheap "already initialised" probe; on a
  hit, the follower awaits the existing value without ever calling `func` or
  allocating. This preserves the leader's boxing (and thus the small-state-machine
  benefit) while removing the followers' waste. It does require care to remain
  race-free — a follower that probes "not initialised" and then loses the race still
  falls back to today's behaviour, which is correct. Contained to `execute` and
  `PanicAwareCell`.
- **Evidence:** inferred from code reading (the `get_or_init` drop-on-loss semantics
  are `async_once_cell`'s documented behaviour). **Confirming benchmark:** the
  multithreaded `performance/coalesced_followers_{2,8,32,128}` group from **U1**,
  with `alloc_tracker` counting total allocations per group — should be O(1) in N
  after the fix and O(N) before it.

#### U4. The key is hashed two to three times per call

- **Location:** `crates/uniflight/src/lib.rs:381-390` (`map.get(key)` in
  `get_or_create_cell`), `:409` (`map.entry(key.to_owned())`), `:370`
  (`inner.remove_if(owned_key.borrow(), …)`).
- **Issue:** A warm call hashes the key twice: once for the `get` and once for the
  `remove_if`. A cold call hashes it three times: `get`, `entry`, `remove_if`. Each
  hash is an `ahash` pass over the key bytes plus a shard-index computation. For long
  keys (URLs, cache keys — the crate's own `cache_population` example uses exactly
  these) the hashing is not negligible.
- **Impact:** **Medium** — `ahash` is fast, so for short keys this is tens of cycles;
  for a 200-byte cache key it is more like hundreds, per call, twice over. It is
  strictly redundant work.
- **Remediation:** Compute the hash once and use DashMap's `_and_hash` / raw-entry
  style APIs (`determine_shard` + `shards()`), which the crate already depends on. Or
  more simply: fixing **U1** removes the `remove_if` from the warm path entirely,
  taking warm calls from two hashes to one. This finding largely dissolves into
  **U1**'s fix, which is a good reason to prioritise **U1**.
- **Evidence:** inferred from code reading. **Confirming benchmark:**
  `performance_cg::execute_uncontended_warm` with a short key and with a 256-byte
  key; the instruction-count delta between them is the hashing cost, and it should
  halve after **U1**.

#### U5. A `thread_aware::Arc` is cloned per call before any work begins

- **Location:** `crates/uniflight/src/lib.rs:359` (`let inner = self.inner.clone();`).
- **Issue:** The first thing `execute` does is clone `self.inner`, a
  `thread_aware::Arc<DashMap<…>, PerProcess>`. Per **TA2** that is **3 atomic
  read-modify-writes** (storage `Arc`, value `Arc`, factory), not one. The clone
  exists so the returned `async` block can own the handle.

  Note the storage strategy is `PerProcess` — i.e. there is exactly one slot, and the
  per-affinity machinery provides no benefit here at all, only cost. A
  `std::sync::Arc` would be 8 bytes and 1 RMW and would behave identically for a
  `PerProcess` cell.
- **Impact:** **Medium** — 3 shared-cache-line atomic RMWs per operation, on a crate
  whose whole value proposition is throughput under concurrent duplicate load. Under
  that load the refcount line is being written by every thread, so these are
  contended RMWs (100–400 cycles each), not the uncontended ~20.
- **Remediation:** Since the strategy is `PerProcess`, replace
  `thread_aware::Arc<_, PerProcess>` with `std::sync::Arc<_>`: 1 RMW instead of 3, 8
  bytes instead of 48. This looks like a straightforwardly correct simplification —
  but note the crate deliberately exposes `thread_aware::cell::storage::Strategy` in
  its `allowed_external_types` list (`crates/uniflight/Cargo.toml:20-25`), so
  per-affinity storage may be an intended future extension point. If so, keep the
  type and pursue **TA2**'s remediation instead.
- **Evidence:** inferred from code reading; RMW counts traced through
  `thread_aware/src/cell/mod.rs:142-150` + `factory.rs:43-52`; sizes empirically
  verified. **Confirming benchmark:** `performance_cg::execute_uncontended_warm` —
  the `lock`-prefixed instruction count is directly readable.

#### U6. `remove_if` performs an upgrade/drop atomic pair inside the write lock

- **Location:** `crates/uniflight/src/lib.rs:370` (the closure
  `|_, weak| weak.upgrade().is_none()`).
- **Issue:** The predicate passed to `remove_if` runs *while the shard write lock is
  held*. `Weak::upgrade` is a compare-and-swap loop on the strong count; the
  resulting `Arc` is then immediately dropped, decrementing it. So each follower
  performs two atomic operations on a contended refcount inside an exclusive
  critical section — lengthening the section that **U1** already identified as the
  serialisation point.
- **Impact:** **Medium** — subordinate to **U1** but worth calling out separately,
  because it means the critical section is not merely "acquire, compare, release" but
  includes a CAS loop that can itself spin under contention. Holding a lock across a
  CAS loop is the shape you least want.
- **Remediation:** Subsumed by **U1**: if only the last holder calls `remove_if`, the
  predicate runs once per group instead of N times. No separate fix needed.
- **Evidence:** inferred from code reading.

#### U7. `PanicAwareCell::get_or_init` clones the result for every caller

- **Location:** `crates/uniflight/src/lib.rs:367` (`cell.get_or_init(boxed).await.clone()`),
  `crates/uniflight/src/lib.rs:477-489` (`PanicAwareCell::get_or_init`).
- **Issue:** Every caller receives `T` by clone. For `T = String` or
  `T = Vec<u8>` — the natural types for the crate's documented cache-population use
  case — that is a heap allocation and a memcpy per caller.
- **Impact:** **Low** — this is *inherent* to the API contract (`execute` returns
  `T`, and N callers each need one), and the `T: Clone` bound makes it explicit. It
  is recorded so that callers understand that coalescing N calls does not mean one
  allocation: for a large `T`, N result clones may dominate everything else in this
  section. **No change recommended**; the mitigation is on the caller's side (use
  `T = Arc<Payload>`), and that is worth a sentence in the crate docs.
- **Remediation:** Documentation only: recommend `Arc<_>` payloads in the crate-level
  docs.
- **Evidence:** inferred from code reading.

#### U8. `catch_unwind` wrapping adds a landing pad per call

- **Location:** `crates/uniflight/src/lib.rs:477-489` (`PanicAwareCell::get_or_init`
  and the `AssertUnwindSafe` / `catch_unwind` wrapping), with `LeaderPanicked`
  defined nearby.
- **Issue:** The leader's future is wrapped in `catch_unwind` so that a panicking
  leader does not poison all followers — a genuinely important defensive behaviour.
  It costs an extra state-machine layer and an unwind landing pad on the leader's
  path.
- **Impact:** **Low** — landing pads cost essentially nothing when no panic occurs
  (they are cold-path metadata), and `docs/performance.md` explicitly says to
  preserve defensive runtime checks. Listed only to record that it was examined and
  is **correctly** left alone. `LeaderPanicked` holds an `Arc<str>`, so the error
  path clone is cheap too — good design.
- **Remediation:** None. Do not remove.
- **Evidence:** inferred from code reading.
- **Philosophy note:** This is the inverse of a conflicting finding — removing this
  would violate the "preserve defensive runtime checks" rule. Recorded so no future
  reviewer proposes it.

### Benchmark coverage

`crates/uniflight/benches/performance.rs` is the **only** multithreaded benchmark in
this entire eight-crate cluster, which is to its credit. But it has serious
methodological problems, and there is **no Callgrind coverage**.

1. **`unique_key()` calls `format!` inside the timed closure.** Every iteration
   allocates and formats a fresh key *inside* the measured region, so a meaningful
   fraction of every sample is `format!`, not `Merger::execute`.
2. **Every iteration uses a fresh key**, so `single_call` *always* takes the cold
   `insert_or_get_existing` path (two `to_owned()`s, three hashes, a fresh
   `Arc<PanicAwareCell>` allocation, a map insert). **The warm fast path — the one
   the crate is optimised for and the one production actually runs — is never
   benchmarked at all.** This is the most consequential gap: findings **U1**, **U2**
   and **U4** are all specifically about the warm path.
3. **`high_contention_100` spawns 100 Tokio tasks inside the timed region**, so the
   sample is dominated by Tokio's spawn and scheduling costs rather than the
   merger's. It also does not sweep the coalescing factor, so the *scaling* behaviour
   that **U1** and **U3** predict (per-follower cost growing with N) cannot be seen
   even in principle — one data point cannot show a trend.
4. **Naming violation.** The benchmarks use bare `bench_function` with no group, so
   the IDs carry no `performance/` prefix, contrary to `docs/naming.md`'s requirement
   that identifiers be prefixed with the benchmark file's basename.
5. **No allocation counting.** `crates/layered/benches/dynamic.rs` demonstrates the
   `alloc_tracker` pattern already available in this workspace; applying it here
   would turn **U2** and **U3** from arguments into measurements.

Recommended additions, in priority order:

- **`crates/uniflight/benches/performance_cg.rs` (Callgrind)** — the single
  highest-value benchmark addition identified anywhere in this assignment. A scenario
  `performance_cg::execute_uncontended_warm` (one merger, one pre-warmed key, one
  `execute` to completion) would expose **U1** (lock acquisition), **U2** (String
  allocation), **U4** (double hashing) and **U5** (three atomic RMWs) *simultaneously*
  and deterministically, in a single-threaded run with no scheduler noise. Pair it
  with the existing `performance.rs` as `docs/naming.md` requires.
- Restructure `performance.rs` into groups `performance/execute_warm`,
  `performance/execute_cold`, `performance/coalesced_followers_{2,8,32,128}`, moving
  key generation out of the timed closure and pre-warming keys for the warm group.
- Add `alloc_tracker` assertions to the warm group.

### Considered and ruled out

- **Atomic ordering.** No `SeqCst` in production code; nothing to tighten.
- **DashMap shard count.** The default (a multiple of the CPU count) is appropriate
  and per-key contention (**U1**) is not fixable by adding shards, since all
  followers of a key share one shard by construction.
- **`ahash` choice.** `ahash` is a good, fast, non-cryptographic hash and is the
  right default here. The issue is hashing *count* (**U4**), not hash quality.
- **`Arc<PanicAwareCell>` allocation on cold keys.** A first-insert cost, explicitly
  deprioritised by `docs/performance.md`.
- **`futures-util` dependency.** Declared `default-features = false` with a minimal
  feature set. Correct.
- **`#[inline]` coverage.** 4 `#[inline]` attributes across 6 public functions —
  the **best** ratio in the cluster by a wide margin. Nothing to add.
- **The `Box::pin` at `:366` in principle.** The boxing is deliberate and documented
  as a state-machine-size trade, and that reasoning is sound. **U3** objects only to
  *followers* paying it, not to the technique.

---

## Crate: layered

### Summary

`layered` provides a `Service`/`Layer` middleware abstraction with three optional
features: `intercept` (before/after hooks), `dynamic-service` (type erasure via a
`plurality` pool) and `tower-service` (tower interop).

The dominant finding is that `DynamicService` — the type-erasure escape hatch —
routes **every request in the process through a single shared `Mutex<Pool>`**. The
inline comment at `crates/layered/src/dynamic.rs:69-72` explicitly justifies the
mutex on the grounds that it is "expected to be uncontended in the thread-isolated
common case", but the type is `Clone + Sync` and there is nothing preventing (or
even discouraging) sharing across threads. A sibling analysis group confirmed this
independently from the consumer side: `crates/fetch/src/tokio.rs:77` selects
`Isolation::Shared`, so in the workspace's own flagship HTTP client **every worker
thread contends on that one mutex, on every request**. The comment's stated
precondition does not hold for the crate's most prominent consumer.

The tower adapter has a separate, compounding problem: it clones the wrapped service
and boxes a future *per adapter layer per request*, and `AdapterLayer` nests
adapters, so an N-layer tower stack costs N clones and N boxes per request.
### Findings

#### L1. `DynamicService` funnels every request through one shared mutex

- **Location:** `crates/layered/src/dynamic.rs:64-92` — the `Mutex::new(Pool::new())`
  at `:73`, the `pool.lock()` at `:80-87`, the comment asserting thread isolation at
  `:69-72`, and the `Clone` impl at `:123-129` that makes sharing trivially possible.
  Consumer confirmation: `crates/fetch/src/tokio.rs:77` (`Isolation::Shared`).
- **Issue:** `DynamicService::new` creates one `Mutex<Pool>` and captures it in the
  `exec` closure, which is then stored behind an `Arc`. Every call to
  `DynamicService::execute` locks that mutex to `alloc_box` the request's future into
  a pool slot. Because the whole `DynamicService` is `Clone + Sync` and stored behind
  an `Arc`, cloning it does **not** produce an independent pool — every clone shares
  the same mutex.

  The comment at `:69-72` reasons that the pool is `Send + !Sync` so a mutex is
  needed, and that it will be uncontended "in the thread-isolated common case". That
  is a strong precondition that the type system does not enforce and that the API
  does not communicate. `crates/fetch` — the workspace's own HTTP client — selects
  `Isolation::Shared`, so on an M-core Tokio runtime, M worker threads serialise on
  one mutex on every single outbound HTTP request.

  The mitigating detail, correctly noted in the comment, is that the critical
  section is short: it only moves the already-constructed future into a pool slot,
  and the future is polled after the lock is released. So this is a short-hold,
  high-frequency lock — which is precisely the profile that produces cache-line
  ping-ponging and, past a threshold, futex convoys.
- **Impact:** **High** — a single process-wide serialisation point on the request
  path of the workspace's HTTP client. Even with a short critical section, the mutex
  cache line is written by every core on every request, so the coherence traffic
  alone caps scaling. This is the same structural failure as **T1** in `tick`,
  arrived at independently, and the two are on the *same* request path in `fetch`.
- **Remediation:** Surgical, in increasing order of ambition:
  1. **Thread-local pools.** Replace `Mutex<Pool>` with a `thread_local!` pool (or a
     `thread_aware::Arc<Pool, PerThread>` — this is precisely what `thread_aware`
     exists for, and would be a good internal use of it). The pool's `!Sync`-ness is
     then satisfied structurally rather than by a lock, and the comment's assumption
     becomes an enforced invariant rather than a hope.
  2. **Shard the pool** by thread ID into a fixed array of mutexes, as a smaller
     change with most of the benefit.
  3. At absolute minimum, **correct the comment** at `:69-72` and document on the
     public API that `DynamicService` should not be shared across threads — which,
     given `fetch` already does exactly that, would be documenting a bug rather than
     fixing one.
  Option 1 is recommended.
- **Evidence:** inferred from code reading, **corroborated independently** by a
  sibling analysis group that reached the same conclusion from the `fetch` consumer
  side (`crates/fetch/src/tokio.rs:77` selecting `Isolation::Shared`). Two
  independent derivations agreeing raises confidence substantially. **Confirming
  benchmark:** extend `crates/layered/benches/dynamic.rs` with a
  `bench_on_threadpool()` group `dynamic/shared_service_contended` at 1/2/4/8/16
  threads, all invoking one shared `DynamicService`. If L1 is real, throughput will
  plateau or regress past 2 threads. The existing single-threaded benchmark cannot
  show this.

#### L2. `DynamicService::execute` clones an `Arc` and pool-allocates per request

- **Location:** `crates/layered/src/dynamic.rs:78` (`let cloned = Arc::clone(&service);`),
  `:80-87` (the `pool.lock().alloc_box(fut)`), `:88` (the `plurality::Box::unsize`
  coercion), `:92` (`Self { exec: Arc::new(exec) }`).
- **Issue:** Per request: one `Arc::clone` of the inner service (an atomic RMW on a
  line shared by every thread), one mutex acquisition (**L1**), one pool slot
  allocation, and one unsizing coercion producing a fat pointer. The `Arc::clone` is
  needed only to give the `async move` block a `'static` handle.
- **Impact:** **Medium** — the `Arc::clone` is a contended atomic RMW per request on
  top of **L1**'s mutex, hitting a second shared cache line. The pool allocation is
  the crate's whole point (it is *cheaper* than a fresh `Box`), so that part is
  working as designed and is a genuine win over naive boxing.
- **Remediation:** The `Arc::clone` can be avoided by having the pooled future borrow
  from the `Arc` held by `exec` rather than owning its own clone — this requires the
  future to be tied to the closure's lifetime, which the pool's handle model may
  already support since the handle "owns its slot". Worth investigating; if it does
  not work out cleanly, leave it, because one RMW is small next to **L1**.
- **Evidence:** inferred from code reading. **Confirming benchmark:**
  `crates/layered/benches/dynamic_cg.rs` (Callgrind), scenario
  `dynamic_cg::execute_dynamic` versus `dynamic_cg::execute_typed` — the delta is
  exactly the erasure overhead this finding describes.

#### L3. The tower adapter clones the wrapped service on every call

- **Location:** `crates/layered/src/tower.rs:88-95` (`Adapter::call`),
  `:100-112` (`Adapter::execute`, with the service clone),
  `:130-149` (`AdapterLayer::layer`, which nests `Adapter` inside `Adapter`).
- **Issue:** `tower::Service::call` requires `&mut self` and a `'static` future, so
  the adapter clones the inner service into the returned future — the standard tower
  pattern, and unavoidable in isolation. The problem is composition: `AdapterLayer`
  wraps an already-adapted service, producing `Adapter<Adapter<…>>`. Each nesting
  level performs its own clone on the way down. An N-layer tower stack therefore
  performs **N service clones per request**, and if any wrapped service is itself
  `Arc`-based (the usual case) that is N atomic RMWs on N different shared cache
  lines.
- **Impact:** **Medium** — tower stacks of 3–6 layers are entirely normal
  (timeout, retry, tracing, auth, rate-limit), so this is 3–6 clones per request on
  top of everything else. The individual clone is cheap only if the service is
  trivially cloneable, which is not guaranteed by the bounds.
- **Remediation:** Flatten the adapter: make `AdapterLayer::layer` detect that its
  inner service is already an `Adapter` and compose the layer *inside* the existing
  adapter rather than nesting a new one. That reduces N clones to 1. Contained to
  `tower.rs`, though it needs care with the type-level composition.
- **Evidence:** inferred from code reading. **Confirming benchmark:**
  `crates/layered/benches/tower_cg.rs`, scenario family
  `tower_cg::call_layers_{1,3,6}` — instruction count should be linear in layer count
  before the fix and near-flat after.

#### L4. Each tower adapter and each intercept boxes a future per request

- **Location:** `crates/layered/src/tower.rs:80-84` (the `Future = Pin<Box<…>>`
  associated type) and `:90-93` (the `Box::pin` in `call`);
  `crates/layered/src/intercept.rs:200-227` (`Intercept`'s per-call path, with the
  `Arc::clone` at `:218`).
- **Issue:** `tower::Service` mandates a named `Future` associated type, so `Adapter`
  declares `Pin<Box<dyn Future + Send>>` and boxes on every `call`. Combined with
  **L3**'s nesting, an N-layer tower stack costs N boxes *and* N clones per request.
  `Intercept::call` similarly boxes and clones an `Arc` per request.

  Note the contrast with the crate's own native path: `layered::Service::execute`
  returns `impl Future`, so the *native* composition is allocation-free and fully
  statically dispatched. That is excellent design. The boxing is entirely an artefact
  of the tower interop boundary.
- **Impact:** **Medium** — allocation per layer per request, on the interop path
  only. Users staying within `layered`'s own `Service`/`Layer` traits pay none of
  this, which is worth stating clearly in the docs so users can choose.
- **Remediation:** The `Box` at the tower boundary is forced by `tower::Service`'s
  design and cannot be removed — this is a legitimate ecosystem constraint. But
  **L3**'s flattening reduces the count from N to 1, which captures most of the win.
  Additionally, document that the tower adapter costs one allocation per boundary so
  users do not sprinkle `AdapterLayer` unnecessarily.
- **Evidence:** inferred from code reading. **Confirming benchmark:** as **L3**,
  plus `alloc_tracker` counts in `tower_cg::call_layers_{1,3,6}`.

#### L5. `InterceptInner` holds four separate `Arc<[T]>` slices

- **Location:** `crates/layered/src/intercept.rs:405-418` (the `InterceptInner`
  struct with four `Arc<[…]>` fields, and `before_execute` at `:417`).
- **Issue:** The intercept state is an `Arc<InterceptInner>` where `InterceptInner`
  itself contains four independent `Arc<[…]>` slices (before/after/error/etc. hook
  lists). That is five heap allocations at build time, and at request time up to five
  pointer chases across five different cache lines, plus four loop preambles — even
  when every list is empty, which is the common case for a partially-configured
  intercept.
- **Impact:** **Low** — the allocations are build-time (deprioritised per
  `docs/performance.md`) and the empty-loop preambles are a handful of instructions.
  The cache-locality cost is the real component: five separate allocations means the
  hook lists are scattered, so a request touching all four lists takes four
  independent cache misses. `before_execute` is correctly marked `#[inline]` at
  `:417`, which helps.
- **Remediation:** Store the four lists in one allocation (a single `Arc<[Hook]>`
  with recorded index ranges, or four `Box<[T]>` inline in the `InterceptInner`
  rather than four `Arc<[T]>` — the inner is already behind an `Arc`, so the
  individual `Arc`s buy nothing unless the lists are shared independently, which they
  are not). The `Arc<[T]>` → `Box<[T]>` change is a small, safe win: it removes four
  refcounts and one indirection level each.
- **Evidence:** inferred from code reading. **Confirming benchmark:**
  `crates/layered/benches/intercept_cg.rs`, scenarios `intercept_cg::execute_no_hooks`
  and `intercept_cg::execute_four_hooks`.

#### L6. Native `Service` composition is allocation-free — recorded as a positive

- **Location:** `crates/layered/src/service.rs:11-35` (the `Service` trait returning
  `impl Future + Send`), `crates/layered/src/layer/tuples.rs:1-90` (tuple-based
  static composition), `crates/layered/src/execute.rs`.
- **Issue:** None. The crate's own composition model uses RPITIT (`fn execute(&self,
  input: In) -> impl Future<Output = Self::Out> + Send`) and tuple-based layer
  stacking, so a fully-composed native `layered` stack is one flat, statically
  dispatched state machine with **zero heap allocations and zero virtual calls per
  request**. The `Box<S>` and `Arc<S>` blanket impls at `service.rs:37-57` forward
  through `(**self)` without adding a layer.
- **Impact:** **Low** (positive) — recorded explicitly so that the boxing findings
  above (**L2**, **L4**) are read correctly: they are the cost of the *escape
  hatches* (`dynamic-service`, `tower-service`), not of the crate's core design. The
  core design is the right one.
- **Remediation:** None. Consider documenting the cost difference between the native
  path and the two erasure paths, so users know what each escape hatch costs.
- **Evidence:** inferred from code reading.

#### L7. Modest `#[inline]` coverage — 2 of 7 public functions

- **Location:** whole crate; `crates/layered/src/intercept.rs:417`
  (`before_execute`, correctly annotated) is one of the two.
- **Issue:** 2 `#[inline]` / 7 `pub fn`. This is the second-best ratio in the cluster
  (after `uniflight`), and the two that *are* annotated are the right ones. Most of
  the remaining public surface is generic and therefore monomorphised into callers
  anyway.
- **Impact:** **Low** — little to gain. Listed for completeness against the
  cluster-wide census.
- **Remediation:** Nothing required. Possibly `DynamicService::execute`'s thin
  forward, but it immediately hits a mutex, so inlining it saves nothing meaningful.
- **Evidence:** empirically verified by grep census (2/7).

### Benchmark coverage

`crates/layered/benches/` contains three files: `dynamic.rs`, `intercept.rs` and
`tower.rs`, each gated behind `required-features`. There is **no Callgrind coverage
for any of them**.

Assessment:

- **`dynamic.rs` is the best-designed benchmark in this entire cluster.** It uses
  `alloc_tracker` to count allocations and `benchmarking::time_sample`, and it
  compares the typed path against the dynamic path — which is precisely the right
  comparison and precisely the discipline the other crates lack. It should be the
  template for the additions recommended elsewhere in this document.
- **But it is single-threaded**, so it cannot see **L1** — the crate's highest-impact
  finding — even in principle. A benchmark that measures a shared mutex from one
  thread measures the uncontended case, which is the case the code was designed for
  and the case that is not the problem. Per `docs/benchmarks.md`, a multithreaded
  benchmark must be written explicitly with `bench_on_threadpool()`; none of the three
  files does.
- **Naming.** The `dynamic.rs` group is named `typed-vs-dynamic`, which does not
  carry the `dynamic/` file-basename prefix required by `docs/naming.md`. Same class
  of issue as `tick`'s `clock_operations` and `uniflight`'s bare `bench_function`
  calls — three of the four benchmarked crates in this cluster violate the naming
  rule, which suggests the rule is not being enforced in review.
- **`tower.rs`** benchmarks the adapter but does not sweep layer depth, so **L3** and
  **L4**'s linear-in-N behaviour is unmeasurable.
- **`intercept.rs`** does not distinguish the no-hooks case from the
  hooks-configured case, so **L5** is unmeasured.

Recommended additions, in priority order:

- `dynamic/shared_service_contended` using `bench_on_threadpool()` at 1/2/4/8/16
  threads → the only way to confirm **L1**. Highest value in this crate.
- `crates/layered/benches/dynamic_cg.rs` (Callgrind, pairing with `dynamic.rs` per
  `docs/naming.md`), scenarios `execute_typed` / `execute_dynamic` → confirms **L2**.
- `crates/layered/benches/tower_cg.rs`, scenario family `call_layers_{1,3,6}` →
  confirms **L3**, **L4**.
- `crates/layered/benches/intercept_cg.rs`, scenarios `execute_no_hooks` /
  `execute_four_hooks` → confirms **L5**.
- Rename existing groups to carry their file-basename prefixes.

### Considered and ruled out

- **Atomic ordering.** The only atomics in the crate are in tests, and they use
  `Relaxed`, which is correct for test counters. No `SeqCst` in production code.
- **`plurality` pool allocator.** The pool is the right idea and demonstrably
  cheaper than per-request `Box::new` — `dynamic.rs`'s own allocation counts show it.
  The mutex around it (**L1**) is the problem, not the pool.
- **Feature gating.** `default = []` with `intercept`, `dynamic-service` and
  `tower-service` all optional is exactly right: a consumer using only the native
  `Service`/`Layer` path pays for none of the erasure machinery, and does not even
  compile `plurality` or `tower-service`. Good design; no finding.
- **`tower-layer` as a non-optional dependency.** It is tiny (a single trait) and is
  needed for the `Layer` interop shape. Acceptable.
- **`Box<S>` / `Arc<S>` blanket `Service` impls** (`service.rs:37-57`) — pure
  `(**self)` forwards, no added indirection beyond the pointer deref that is already
  paid. Nothing to optimise.
- **Layer tuple composition** (`layer/tuples.rs`) — fully static, no allocation,
  monomorphised. Optimal.

---

## Cluster-wide observations

Recorded here rather than in any one crate section, because they are properties of
the cluster as a whole.

1. **The same structural mistake appears three times independently.** `tick`'s global
   `Mutex<Timers>` (**T1**), `layered`'s shared `Mutex<Pool>` (**L1**) and
   `uniflight`'s per-completion shard write lock (**U1**) are the same failure:
   a single serialisation point placed on a per-operation path, in each case
   accompanied by a comment or design note asserting that contention will not
   happen. In `fetch`, **all three are on the same request path simultaneously**.
   That is worth treating as a workspace-level pattern rather than three unrelated
   bugs.

2. **`thread_aware::Arc` is a cluster-wide cost multiplier.** Its 3–4 atomic RMWs per
   clone (**TA2**) and 48-byte size (**TA1**) are paid per operation by `tick`
   (**T4**), `anyspawn` (**AS2**) and `uniflight` (**U5**). Fixing **TA2** alone —
   specifically moving the `Factory` inside the shared allocation — would improve all
   three sibling crates without touching them. It is the highest-leverage single
   change identified in this assignment. Note also that both `uniflight` (`PerProcess`)
   and `anyspawn` (`dyn`, stateless) use the type in configurations where the
   per-affinity machinery provides no benefit at all.

3. **Contention is structurally unmeasured.** Seven of eight crates have zero
   Callgrind coverage. `uniflight`'s `high_contention_100` is the *only* multithreaded
   benchmark in the cluster, and it measures Tokio spawn cost more than it measures
   the merger. Per `docs/benchmarks.md`, Criterion benchmarks are single-threaded
   unless explicitly written with `bench_on_threadpool()`, and none in this cluster
   is. **For a concurrency cluster, every High finding in this document is
   unmeasurable with the current benchmark suite.** That is the headline
   benchmark-coverage conclusion.

4. **Benchmark builds do not resemble production builds, along two axes.**
   `[profile.bench]` sets `lto = "fat"` + `codegen-units = 1` while
   `[profile.release]` sets neither, so every "missing `#[inline]`" finding
   (**T5**, **TA4**, **AS5**, **AZ5** — 74 unannotated public functions across the
   cluster) is invisible to the suite by construction. And `--all-features` plus
   dev-dependency feature unification means every `tick`-touching benchmark measures
   the `test-util` clock (**T7**), not the production one.

5. **Three of four benchmarked crates violate `docs/naming.md`'s group-prefix rule**
   (`tick`'s `clock_operations`, `uniflight`'s bare `bench_function` calls,
   `layered`'s `typed-vs-dynamic`). Only `thread_aware` complies. This suggests the
   rule is documented but not enforced in review.

6. **Where the crates are good, they are very good.** `layered`'s native
   RPITIT-based composition is allocation-free and fully static (**L6**);
   `uniflight`'s `catch_unwind` leader-panic isolation is a correctly-preserved
   defensive check (**U8**); `tick`'s non-`test-util` clock genuinely compiles down
   to a bare `Instant::now()`; `layered/benches/dynamic.rs`'s `alloc_tracker`
   discipline is the model the rest of the workspace should copy; and the derive
   macro emits optimal, allocation-free, statically-dispatched code (**MI3**). The
   findings above are concentrated in the shared-state and type-erasure seams, not in
   the core designs.

## Appendix: finding index

| ID | Crate | Impact | Title |
|---|---|---|---|
| T1 | tick | High | Every delay and timeout registers through a process-wide mutex |
| T2 | tick | High | Wakers invoked while the timers mutex is held |
| T3 | tick | Medium | Redundant `unregister_timer` on the normal fired path |
| T4 | tick | Medium | Whole `Clock` cloned per delay/timeout/periodic timer |
| T5 | tick | Medium | Zero `#[inline]` across 35 public functions |
| T6 | tick | Medium | Idle driver pays RwLock + O(processors) scan per tick |
| T7 | tick | Medium | `test-util` unification makes benchmarks measure the slow clock |
| T8 | tick | Low | `advance_timers` allocates a fresh `BTreeMap` per batch |
| T9 | tick | Low | `PeriodicTimer` re-anchors per tick, accumulating drift |
| T10 | tick | Low | `Clock` is ~56 bytes and copied into every future |
| TA1 | thread_aware | High | `Arc` is 48 bytes (64 for `dyn`) vs 8 |
| TA2 | thread_aware | High | `Arc::clone` is 3–4 atomic RMWs, not 1 |
| TA3 | thread_aware | Medium | `strong_count` locks and scans every slot |
| TA4 | thread_aware | Medium | Zero `#[inline]` across 27 public functions |
| TA5 | thread_aware | Medium | `relocate` takes a write lock for a read-only case |
| TA6 | thread_aware | Low | `Affinity` has no niche |
| TA7 | thread_aware | Low | Erased closures: `Box` inside `Arc`, allocating clone |
| TA8 | thread_aware | Low | `Storage` growth reallocates under the write lock |
| TA9 | thread_aware | Low | `count_where` full scan under the lock |
| MA1 | thread_aware_macros | Low | Facade adds one proc-macro crate (no action) |
| MI1 | thread_aware_macros_impl | Medium | `parse_quote!` re-run per field |
| MI2 | thread_aware_macros_impl | Low | `HashSet` rebuilt per enum variant |
| MI3 | thread_aware_macros_impl | Low | Empty `match` emitted for hook-free enums |
| AS1 | anyspawn | High | Oneshot channel allocated per spawn, even fire-and-forget |
| AS2 | anyspawn | High | `CustomSpawner` uses the 4-RMW `thread_aware::Arc` variant |
| AS3 | anyspawn | Medium | `spawn_anywhere` costs three allocations |
| AS4 | anyspawn | Medium | Each builder layer boxes per spawn |
| AS5 | anyspawn | Low | Zero `#[inline]` across 12 public functions |
| AS6 | anyspawn | Low | `tokio` fast path is opt-in; slow path is the default |
| AZ1 | anyspawn_azure | Medium | `sleep` clones a whole `Clock` and boxes |
| AZ2 | anyspawn_azure | Medium | `yield_now` heap-allocates a one-byte future |
| AZ3 | anyspawn_azure | Medium | `spawn` costs ~four allocations |
| AZ4 | anyspawn_azure | Low | Full `futures` facade for two types |
| AZ5 | anyspawn_azure | Low | No `#[inline]`, no benchmarks, per-request adapter |
| U1 | uniflight | High | Shard **write** lock on every completion |
| U2 | uniflight | High | `String` allocated per call, twice on cold keys |
| U3 | uniflight | High | Every follower builds and boxes a discarded future |
| U4 | uniflight | Medium | Key hashed two to three times per call |
| U5 | uniflight | Medium | `thread_aware::Arc` clone (3 RMWs) per call |
| U6 | uniflight | Medium | Upgrade/drop atomic pair inside the write lock |
| U7 | uniflight | Low | Result cloned per caller (inherent; document only) |
| U8 | uniflight | Low | `catch_unwind` landing pad (correct; do not remove) |
| L1 | layered | High | `DynamicService` funnels every request through one mutex |
| L2 | layered | Medium | `Arc::clone` + pool lock per request |
| L3 | layered | Medium | Tower adapter clones the service per nesting level |
| L4 | layered | Medium | Boxed future per adapter and per intercept, per request |
| L5 | layered | Low | `InterceptInner` holds four separate `Arc<[T]>` |
| L6 | layered | Low | Native composition is allocation-free (positive) |
| L7 | layered | Low | 2 of 7 public functions carry `#[inline]` |

**Totals: 49 findings — 8 High, 20 Medium, 21 Low.**
