# Oxidizer workspace — exhaustive performance analysis

*Analysis-only report. No behavioural code change accompanies it.*

## Scope and method

Nine specialist analysis workers read **every crate in `crates/` (53 crates)** plus the
workspace/build layer (Cargo profiles, `[workspace.dependencies]`, feature flags, benchmark
infrastructure, CI). They produced **309 findings — 39 High, 128 Medium, 140 Low**. This document
is a merged, de-duplicated and heavily condensed presentation of all of them. The unabridged
per-group source documents are preserved verbatim in `docs/perf-findings/*.md`
(`g1-multitude`, `g2-seatbelt`, `g3-bytesbuf-cachet`, `g4-routing-http`, `g5-network`,
`g6-data-interning`, `g7-runtime-concurrency`, `g8-errors-privacy-misc`, `g9-workspace-build`);
every finding here carries its original group ID in parentheses so it can be traced back to the
full-length write-up.

## Environment caveat — read this before acting on any number

The analysis containers had **no network egress to `index.crates.io` / `static.crates.io`**, no
cargo registry cache and no prebuilt `target/`. Consequently `cargo build`, `cargo test`,
`cargo bench`, `cargo clippy`, `cargo metadata --offline`, `cargo tree` and **every `just` recipe
failed at dependency resolution** (`cargo metadata --offline` exits 101 with
`error: no matching package named 'tokio' found ... location searched: crates.io index`).
`--offline` does not help: there is nothing in the local registry to be offline with.

Therefore **nearly every finding in this report is inferred from code reading**, not measured.
No wall-clock or instruction-count number in this document was produced by running this
workspace's benchmarks.

One empirical technique did work and was used: compiling **dependency-free, layout-identical
replicas** of the types under study with plain `rustc -O` in `/tmp` to measure `size_of` /
`align_of` / field offsets, and driving them with a **counting `GlobalAlloc`** to count
allocations. Findings resting on that, or on exact file/lockfile content censuses, are labelled
**EMPIRICAL**. Everything else is labelled **inferred**.

A second, structural caveat compounds the first: **every Criterion and Callgrind number that
already exists in this repository was produced under whole-program fat LTO, a single codegen unit,
`--all-features`, `-C target-cpu=x86-64-v3` and — in 19 bench files — a non-default global
allocator.** Any pre-existing benchmark result cited by a finding inherits all five biases. See
§Workspace, OX-287 and OX-292.

## How to read a finding

Each entry is:

`**<global ID> (<group>-<original ID>)** — title · **Severity** · evidence-label`

- *Loc:* — the file:line citations, reproduced **exactly** as the analysing worker recorded them.
  Line numbers were never normalised, tidied or re-derived.
- *Issue* / *Impact* / *Fix* — condensed. High-severity findings keep separate bullets; Medium and
  Low findings are compressed into a single bullet carrying the same three parts.
- *Philosophy:* — present when the finding interacts with `docs/performance.md`,
  `docs/benchmarks.md`, `docs/callgrind-benchmarks.md`, `docs/naming.md` or `AGENTS.md`. A
  remediation that would violate house philosophy (surgical over architectural; preserve defensive
  runtime checks; idiomatic Rust; deprioritise first-insert/teardown work; no allocation on the hot
  path; justify deviations from ecosystem patterns; judicious `#[inline]`) is marked
  **Conflicting** and is recorded rather than recommended.
- Evidence labels: **EMPIRICAL** (measured replica / exact file census), **EMPIRICAL+INFERRED**
  (measured layout, inferred runtime consequence), *inferred* (code reading only).
- An ellipsis (…) marks prose elided for length; the full text is in `docs/perf-findings/`.

Severity is the analysing worker's judgement of expected impact on a consumer's release build, not
a measured speed-up.

## Executive summary

Ranked by expected impact on a consumer's release build. Each line points at the section holding
the full entry.

**Measurement apparatus — fix these first, because they distort everything else**

1. **OX-287 — `[profile.bench]` (fat LTO, `codegen-units = 1`) diverges from a bare
   `[profile.release]`** (`Cargo.toml:340-341`, `Cargo.toml:343-346`). Benchmarks inline across
   crate boundaries that a consumer's default `lto = false` / `codegen-units = 16` release build
   never inlines, so a missing `#[inline]` is invisible to measurement and costly in production.
   This single asymmetry underwrites the eleven separate zero-`#[inline]` findings below.
   §Workspace.
2. **OX-014 — `Vec::push` inside the timed region of `time_sample_with_inputs`**
   (`crates/benchmarking/src/lib.rs:144-146`; timer starts `:142`, stops `:148`). Every
   allocation-tracked benchmark in the workspace over-reports by a return-value-size-dependent
   amount. §benchmarking.
3. **OX-305 — no benchmark is ever executed in CI, and benches are not even compiled in PR
   CI** (`justfiles/anvil/checks/bench.just:13,16`, `.github/workflows/main.yml`). Nothing would
   detect the reintroduction of any finding in this report. §Workspace.
4. **OX-049 / OX-065 — `cachet`'s benchmarks measure `cachet_tier`'s `MockCache`**
   (`crates/cachet_tier/src/testing.rs:224-277`, used by `crates/cachet/benches/dynamic.rs` and
   `crates/cachet/benches/operations.rs`). Three mutex acquisitions, a key clone and an unbounded
   `Vec` push dominate the published numbers. §cachet, §cachet_tier.

**Per-request hot paths**

5. **OX-234 → OX-220 — the seatbelt breaker chain.** The default breaker ID `format!`s a
   fresh `String` per request (`crates/seatbelt_http/src/breaker.rs:154-159`, installed at
   `:119-123`), which defeats the lock-free engine lookup
   (`crates/seatbelt/src/breaker/engine/engines.rs:35-61`), which then takes one global
   `Mutex<State>` **twice** per request even in the steady closed state
   (`crates/seatbelt/src/breaker/engine/engine_core.rs:34-48`). §seatbelt_http, §seatbelt.
6. **OX-063 / OX-064 — `cachet_tier::CacheTier`'s `&K` signature and `dyn(box)` erasure**
   (`crates/cachet_tier/src/tier.rs:38,46`): one owned-key allocation per lookup plus one boxed
   future per tier per operation. Root cause of OX-037 (`Cache::get` allocates an owned key on
   every lookup). §cachet_tier, §cachet.
7. **OX-039 — unconditional global atomic increment on every cache operation**
   (`crates/cachet/src/telemetry/cache.rs:20`, `crates/cachet/src/telemetry/cache.rs:27-29`, called from seven sites in `cache.rs`).
   One globally shared `lock xadd` line turns a linear-scaling cache flat. §cachet.
8. **OX-031 — `ReadAsFuturesStream::poll_next` heap-allocates a boxed future per stream item**
   (`crates/bytesbuf_io/src/read_futures.rs:75-117`, `Box::pin` at `:85`, `mem::transmute`
   `:87-97`). One malloc per chunk on a byte-stream adapter. §bytesbuf_io.
9. **OX-084 — `Logging::execute` redacts the URL eagerly on every request**
   (`crates/fetch/src/handlers/logging.rs:102-107`, eager call at `:107`), on by default, cost
   scaling with URL length. §fetch.
10. **OX-085 ≡ OX-139 — the `DynamicService` / `Isolation::Shared` contention issue.** Two
    groups found the same defect from opposite sides: `crates/fetch/src/tokio.rs:77`,
    `handlers/transport.rs:15,19`, `pipeline/builder.rs:124,146` on the client side, and the
    `Mutex<Pool>` at `crates/layered/src/dynamic.rs:73-87` on the layer side. **Stated once in
    §layered; §fetch cross-references it.** §fetch, §layered.
11. **OX-279 — every `uniflight` completion takes a DashMap shard write lock**
    (`crates/uniflight/src/lib.rs:370`, within `execute` at `:350-372`): high coalescing, the
    crate's best case, becomes its worst case for lock contention. §uniflight.
12. **OX-269 — every `tick` delay and timeout registers through a process-wide mutex**
    (`crates/tick/src/state.rs:71-90`, `:112-127`, `crates/tick/src/clock.rs:458-472`). §tick.
13. **OX-146 — `multitude`'s `de::Value` / `de::Number` inflated to 32 B / align 16 by the
    `i128`/`u128` variants** (`crates/multitude/src/de/value/number.rs:11-37`,
    `dynamic_value.rs:18-47`, `entry.rs:20-45`) — **empirically verified** by `rustc -O` replica;
    a 33 % increase in bytes touched on the crate's headline workload. §multitude.
14. **OX-070 — `data_privacy` hashes two strings per redaction before doing any redaction
    work** (`crates/data_privacy/src/redaction_engine.rs:113-121` →
    `redaction_engine_inner.rs:31-37`; key type `DataClass` at
    `crates/data_privacy_core/src/data_class.rs:17-21`). §data_privacy, §data_privacy_core.
15. **OX-127 — repeat interns of an already-present string serialise against each other within
    an `internity` shard.** §internity.
16. **OX-250 — `chumsky`, a full parser-combinator framework, is pulled into every consumer's
    build graph by `templated_uri_macros_impl`.** §templated_uri_macros_impl.

**Cross-cutting**

17. **Zero or near-zero `#[inline]` on the highest-fan-in crates** — `ohno` (9 dependents),
    `thread_aware` (8), `tick` (7) have none, while `[profile.release]` leaves LTO off
    (OX-310, OX-309). Echoed per crate by OX-112, OX-089, OX-273, OX-173,
    OX-077, OX-242 and others. All are consequences of OX-287; treat them as one
    programme of work, and follow `docs/performance.md:18-30`'s judicious-`#[inline]` rule rather
    than annotating indiscriminately. §Workspace.
18. **Benchmark naming/pairing violations break the documented discovery mechanism**
    (OX-306, against `docs/naming.md:81-90`). §Benchmark coverage gaps.

## Per-crate findings

All 53 crates in `crates/` have a section. Crates where nothing was found say so
explicitly, together with what was examined. Workspace/build-level findings are in
§Workspace, not here.

### `anyspawn`

*6 findings — 2 High, 2 Medium, 2 Low. Examined:* `anyspawn` abstracts task spawning over Tokio, smol and arbitrary custom spawners.
  …

**OX-001 (g7-AS1)** — Custom spawn allocates a oneshot channel per task, even fire-and-forget · **High** · inferred
- *Loc:* `crates/anyspawn/src/custom.rs:94-101` (`CustomSpawner::spawn`, the `oneshot::channel()` at `:95` and the `Box::pin` at
  `:96`), `crates/anyspawn/src/handle.rs:22-26` (the `JoinHandle` wrapping the receiver), contrasted with the native Tokio path at
  `crates/anyspawn/src/spawner.rs:209-221`, which just hands back Tokio's own handle.
- *Issue:* Every custom spawn does: 1. `oneshot::channel()` — one heap allocation for the shared channel state, plus atomics on
  sender/receiver registration and on `send`. 2. …
- *Impact:* **High** — this is the crate's single per-operation cost and it is roughly 2–3× the allocation count of a …
- *Fix:* Surgical: make the channel lazy. Return a `JoinHandle` enum with a `Detached` variant and only allocate the …
- *Also cited:* `spawner.rs:209-221`

**OX-002 (g7-AS2)** — `CustomSpawner` uses the 4-atomic-RMW `thread_aware::Arc` variant · **High** · EMPIRICAL+INFERRED
- *Loc:* `crates/anyspawn/src/custom.rs:84-87` (the struct: `spawn: thread_aware::Arc<dyn SpawnCustom, PerCore>`), `:90-93` (`new`,
  which calls `thread_aware::Arc::with_clone_fn`), plus `crates/thread_aware/src/cell/clone_fn.rs:70-77`.
- *Issue:* `CustomSpawner::new` builds the cell with `with_clone_fn`, which produces a `Factory::ErasedCloneFn`. …
- *Impact:* **High** — the per-clone cost is 4× what a `std::sync::Arc<dyn SpawnCustom>` would cost, on a type explicitly …
- *Fix:* Two surgical options: 1. If per-core storage of the spawn function is not actually needed — and it is hard to see …

**OX-003 (g7-AS3)** — `spawn_anywhere` on the custom path costs three allocations · **Medium** · inferred
- *Loc:* `crates/anyspawn/src/custom.rs:103-114` (`spawn_anywhere`: the `oneshot::channel()` at `:109` and the
  `Box::new(SpawnAnywhereTask{…})` at `:110`), plus `crates/anyspawn/src/custom.rs:75-81` (`SpawnAnywhereTask::call_once`, which
  does a further `Box::pin`). · `spawn_anywhere` allocates the oneshot channel, then boxes the `SpawnAnywhereTask`, and … **Impact
  Medium:** **Medium** — `spawn_anywhere` is the less common entry … **Fix:** The `Box::new(SpawnAnywhereTask)` at `:110` is
  avoidable — …

**OX-004 (g7-AS4)** — Every builder layer adds a boxed wrapper per spawn · **Medium** · inferred
- *Loc:* `crates/anyspawn/src/builder.rs:56-65` (the `LayeredTask` wrapping in the layer-application loop). · `CustomSpawnerBuilder`
  composes layers by wrapping each task in a `LayeredTask`, boxed. … **Impact Medium:** **Medium** — layers are configured once but
  charged per … **Fix:** Compose the layers into a single boxed closure **at build …

**OX-005 (g7-AS5)** — Zero `#[inline]` across 12 public functions · **Low** · EMPIRICAL+INFERRED
- *Loc:* whole crate; notably `crates/anyspawn/src/handle.rs` (`JoinHandle`'s `poll`, a thin forward to the oneshot receiver) and
  `crates/anyspawn/src/spawner.rs` accessors. · 0 `#[inline]` / 12 `pub fn`. … **Impact Low:** **Low** — mostly mooted by … **Fix:**
  Consider `#[inline]` only on …

**OX-006 (g7-AS6)** — `tokio` backend is an optional feature, so the fast path is opt-in · **Low** · inferred
- *Loc:* `crates/anyspawn/Cargo.toml` (`[features] default = []`, `tokio = ["dep:tokio"]`). · The crate's default feature set is
  empty, so a … **Impact Low:** **Low** — it is a … **Fix:** Document the cost difference …

### `anyspawn_azure`

*5 findings — 3 Medium, 2 Low. Examined:* A small adapter implementing `azure_core`'s async runtime traits on top of `anyspawn` +
  `tick`. …

**OX-007 (g7-AZ1)** — `sleep` clones a whole `Clock` and boxes, per call · **Medium** · inferred
- *Loc:* `crates/anyspawn_azure/src/runtime.rs:75-82`. · The `sleep` implementation clones `self.clock` — which per **T4**/**T10**
  is a ~56-byte … **Impact Medium:** **Medium** — the Azure SDK calls `sleep` on every retry … **Fix:** The `Box::pin` is forced by
  the trait. …

**OX-008 (g7-AZ2)** — `yield_now` heap-allocates a one-byte future per call · **Medium** · inferred
- *Loc:* `crates/anyspawn_azure/src/runtime.rs:84-86`. · `yield_now` returns `Box::pin(YieldNow { yielded: false })`. … **Impact
  Medium:** **Medium** — `yield_now` exists precisely to be called … **Fix:** The `Pin<Box<…>>` return type is mandated by
  `azure_core`, so …

**OX-009 (g7-AZ3)** — `spawn` costs roughly four heap allocations per Azure SDK task · **Medium** · inferred
- *Loc:* `crates/anyspawn_azure/src/runtime.rs:62-73`. · Per spawn: `AbortHandle::new_pair()` (an `Arc<AbortInner>` allocation), the
  `Abortable` … **Impact Medium:** **Medium** — the Azure SDK spawns per long-running … **Fix:** If `azure_core` does not require
  abort semantics on every …

**OX-010 (g7-AZ4)** — Depends on the full `futures` facade for two types · **Low** · inferred
- *Loc:* `crates/anyspawn_azure/Cargo.toml` (`futures = { workspace = true, features = ["std"] }`). · The crate uses `futures` only
  for `AbortHandle` / … **Impact Low:** **Low** — no run-time cost (unused … **Fix:** Replace with `futures-util = { …

**OX-011 (g7-AZ5)** — No `#[inline]`, no benchmarks, on a per-request adapter · **Low** · EMPIRICAL
- *Loc:* whole crate. · 0 `#[inline]` attributes; the crate is a thin … **Impact Low:** **Low** individually — the … **Fix:** Low
  priority. Prioritise the benchmark …

### `automation`

*2 findings — 2 Low. Examined:* Build tooling: `publish = false`, invoked by developers and CI at human timescales, with no runtime
  consumers anywhere in …

**OX-012 (g8-F41)** — `kill_by_pid` spawns an external process instead of signalling directly · **Low** · inferred
- *Loc:* `crates/automation/src/process.rs:100-109`. · The function shells out to `kill` on Unix and … **Impact Low:** Low. This is
  on the timeout path … **Fix:** No action recommended on performance …

**OX-013 (g8-F42)** — Depending on `ohno` with `features = ["app-err"]` turns that feature on workspace-wide · **Low** · inferred
- *Loc:* `crates/automation/Cargo.toml`, the `ohno` dependency entry. · Cargo unifies features across a workspace build … **Impact
  Low:** Low, and bounded to the … **Fix:** No action recommended. …
- *Also cited:* `Cargo.toml:5`

### `benchmarking`

*5 findings — 2 High, 1 Low, 2 —. Examined:* This crate matters more than its size suggests, and it is where this group's **single
  most important finding** lives — not …

**OX-014 (g8-F36)** — `Vec::push` is inside the timed region of `time_sample_with_inputs` · **High** · EMPIRICAL+INFERRED
- *Loc:* `crates/benchmarking/src/lib.rs:144-146`; the timer starts at `:142` and stops at `:148`. The `Vec` is created with
  capacity at `:140`.
- *Issue:* The loop body is `outputs.push(black_box(bench(input)))`. The `push` — not just the benchmarked call — sits between
  `Instant::now()` and `start.elapsed()`. …
- *Impact:* **High — the highest-impact finding in this group.** The absolute cost is small, but it is not the absolute …
- *Fix:* Move the storage out of the timed region. The outputs are collected only to defer their destructors past …
- *Philosophy:* CONFLICTING, for remediation option (c) only. `benchmarking` is currently entirely `unsafe`-free, and introducing
  `unsafe` into it …

**OX-015 (g8-F37)** — `time_sample` and `time_sample_with_inputs` measure different things — one includes destructor cost, the other
  excludes it · **High** · inferred
- *Loc:* `crates/benchmarking/src/lib.rs:50-60` (`time_sample`, with the output dropped at `:57`) versus `:140-152`
  (`time_sample_with_inputs`, which defers all drops until after `start.elapsed()` at `:148`).
- *Issue:* In `time_sample`, the benchmarked call's result is bound to `_` and therefore dropped at the end of the statement —
  **inside** the timed loop. …
- *Impact:* **High.** This is worse than either policy would be on its own, because both helpers are in the same crate …
- *Fix:* Pick one policy, apply it to both, and document it prominently. …

**OX-016 (g8-F38)** — `time_sample_async` constructs the future inside the timed region · **Low** · inferred
- *Loc:* `crates/benchmarking/src/lib.rs:69-83`. · `bench(iteration)` is called inside the timer, so … **Impact Low:** Low. For an
  async benchmark this … **Fix:** No code change. Document that …

**OX-017 (g8-F39)** — POSITIVE — measurement-guard drop ordering is exactly right, and regression-tested · **—** · inferred
- *Loc:* `crates/benchmarking/src/lib.rs:149-151`, with the regression test at `:210-247`. · After `start.elapsed()`, the code drops
  the … **Impact —:** N/A — desired state. …

**OX-018 (g8-F40)** — POSITIVE — zero production footprint · **—** · EMPIRICAL
- *Loc:* `crates/benchmarking/Cargo.toml`. · The crate has **no `[dependencies]` section at …

### `bytesbuf`

*12 findings — 6 Medium, 6 Low. Examined:* `bytesbuf` is the most performance-critical crate in this group and, on the whole, the
  most carefully written. …

**OX-019 (g3-F1)** — Global pool holds the pool mutex across multi-block buffer construction · **Medium** · inferred
- *Loc:* `crates/bytesbuf/src/mem/global.rs:204-234` · `allocate_uniform` has two paths. The single-block path (lines 216-227)
  deliberately … **Impact Medium:** `bytesbuf/AGENTS.md` states the architecture is … **Fix:** Collect the blocks under the lock,
  `drop` the guard (or end …
- *Also cited:* `crates/bytesbuf/src/constants.rs:10-33`

**OX-020 (g3-F2)** — `BlockRef` reference-count `Clone`/`Drop` are not `#[inline]` · **Medium** · EMPIRICAL+INFERRED
- *Loc:* `crates/bytesbuf/src/mem/block_ref.rs:118-127` (`impl Clone`), `crates/bytesbuf/src/mem/block_ref.rs:129-134` (`impl
  Drop`), `crates/bytesbuf/src/mem/block_ref.rs:104-116` (`meta`) · `BlockRef` is the shared-ownership handle for a memory block. …
  **Impact Medium:** a non-inlined call around a single atomic RMW … **Fix:** Add `#[inline]` to `Clone::clone`, `Drop::drop` and
  `meta`.

**OX-021 (g3-F3)** — `bytes_compat::view`'s `Buf` impl has no `#[inline]`, unlike its `BufMut` mirror · **Medium** · EMPIRICAL
- *Loc:* `crates/bytesbuf/src/bytes_compat/view.rs:11-42` — `remaining` (line 13), `chunk` (line 18), `chunks_vectored` (line 24),
  `advance` (line 39) · This is the `bytes::Buf` adapter for `BytesView` — the interop surface every … **Impact Medium:**
  `Buf::advance` and `Buf::chunk` are called in a … **Fix:** Add `#[inline]` to the four `Buf` methods, matching …

**OX-022 (g3-F4)** — `BytesBuf`'s small accessors are not `#[inline]` · **Medium** · EMPIRICAL
- *Loc:* `crates/bytesbuf/src/buf.rs` — `len` (line 487), `is_empty` (line 508), `capacity` (line 543), `remaining_capacity` (line
  583), `consume` (line 631), `first_unfilled_slice` (line 967) · `buf.rs` defines 23 `pub fn` and carries exactly one `#[inline]`.
  … **Impact Medium:** individually tiny, collectively a … **Fix:** Annotate the trivial accessors. Be judicious as …

**OX-023 (g3-F5)** — `BytesView`'s small accessors are not `#[inline]` · **Medium** · EMPIRICAL
- *Loc:* `crates/bytesbuf/src/view.rs` — `len` (line 240), `is_empty` (line 250), `first_slice` (line 587), `advance` (line 730),
  `append` (line 790) · Same as F4 for the read side: `view.rs` has 16 `pub fn` and one `#[inline]`. … **Impact Medium:** same
  reasoning as F4, on the more frequently … **Fix:** Annotate `len`, `is_empty`, `first_slice` and `advance`. …

**OX-024 (g3-F6)** — `BytesBuf::peek` builds a `SmallVec` without a capacity hint · **Low** · inferred
- *Loc:* `crates/bytesbuf/src/buf.rs:437` · `peek()` starts from `SmallVec::new()` and pushes … **Impact Low:** only bites for views
  with … **Fix:** Use `SmallVec::with_capacity(n)` where …

**OX-025 (g3-F7)** — `BytesView::range_checked` performs a two-pass span scan · **Low** · inferred
- *Loc:* `crates/bytesbuf/src/view.rs:377-506` · `range_checked` walks the span list once to … **Impact Low:** the constant is small
  and … **Fix:** No surgical change recommended. …

**OX-026 (g3-F8)** — `NeutralBlock` colocates the atomic refcount with payload bytes · **Low** · EMPIRICAL+INFERRED
- *Loc:* `crates/bytesbuf/src/mem/global.rs:284-320` · `NeutralBlock<SIZE>` places its 16-byte … **Impact Low:**
  `bytesbuf/AGENTS.md` states … **Fix:** None recommended. If a future design …

**OX-027 (g3-F9)** — No `#[inline]` on `lib.rs`'s public surface · **Low** · EMPIRICAL
- *Loc:* `crates/bytesbuf/src/lib.rs` · The single `pub fn` defined in `lib.rs` carries … **Impact Low:** Low. **Fix:** Evaluate
  against rule 1 alongside F4/F5.

**OX-028 (g3-F10)** — `buf_put.rs` annotates less than half its public surface · **Low** · EMPIRICAL
- *Loc:* `crates/bytesbuf/src/buf_put.rs` (7 `pub fn`, 3 `#[inline]`) · The `put_*` family is the write-side mirror of … **Impact
  Low:** Low to Medium — depends on how … **Fix:** Bring the small fixed-width `put_*` …

**OX-029 (g3-F11)** — Buffer `unsafe advance` relies on caller-upheld invariants · **Low** · inferred
- *Loc:* `crates/bytesbuf/src/buf.rs:1054` · Assessed per the task's instruction to justify … **Impact Low:** no action needed.
  **Fix:** None. Retain.

**OX-030 (g3-F12)** — Benchmarks cover neither `bytes_compat` conversion nor >8-span shapes · **Medium** · EMPIRICAL
- *Loc:* `crates/bytesbuf/benches/` (7 files, enumerated below) · See "Benchmark coverage". Summarised here because it interacts
  with F3, F6 and F7: the … **Impact Medium:** coverage gap on the crate's interop surface. **Fix:** See "Benchmark coverage" below.

### `bytesbuf_io`

*6 findings — 1 High, 1 Medium, 4 Low. Examined:* `bytesbuf_io` is a thin async I/O abstraction (1,424 source lines across seven
  files, much of it doc comments and tests). … *Also cited:* `crates/bytesbuf_io/src/read.rs:49`,
  `crates/bytesbuf_io/src/read_ext.rs:16`

**OX-031 (g3-F13)** — `ReadAsFuturesStream::poll_next` heap-allocates a boxed future per stream item · **High** · inferred
- *Loc:* `crates/bytesbuf_io/src/read_futures.rs:75-117`, specifically the `Box::pin(future)` at line 85 and the `mem::transmute` at
  lines 87-97; field declaration at line 35
- *Issue:* Each time the stream needs a new item and `active_read` is `None`, `poll_next` constructs an `async move` block over
  `&mut this.inner` and `Box::pin`s it into `Option<Pin<Box<dyn …
- *Impact:* High — for a byte-stream adapter, "one malloc per chunk" is the dominant cost at small chunk sizes, and small …
- *Fix:* Not surgical, and the obvious fixes each have a cost: (a) store the future inline in the struct as an …
- *Philosophy:* The house rule is "no allocation on the hot path" and this violates it, but every remediation is architectural
  rather than …
- *Philosophy:* Flagged as **conflicting**: the fix is larger than house guidance normally sanctions, so it needs an explicit
  decision rather than a quiet patch. - **Confirming benchmark:** none exists.

**OX-032 (g3-F14)** — `ReadAsFuturesStream::new` boxes the entire stream struct · **Low** · inferred
- *Loc:* `crates/bytesbuf_io/src/read_futures.rs:48-54`, surfaced through the public API at `crates/bytesbuf_io/src/read_ext.rs:103`
  and `:186` · `new` returns `Pin<Box<Self>>` rather than … **Impact Low:** one allocation per stream … **Fix:** Return `Self` and
  let the caller pin …

**OX-033 (g3-F15)** — Conditional-read loop calls `BytesBuf::peek()` on every iteration · **Medium** · inferred
- *Loc:* `crates/bytesbuf_io/src/read_ext.rs:169` (inside the `while into.len() < len` loop at lines 158-180) ·
  `read_at_most_into_while` calls `inspect_fn(into.peek())` once per read chunk. … **Impact Medium:** quadratic in chunk count, and
  the trickle-feed … **Fix:** Two options, both modest: give `peek()` a capacity hint …
- *Also cited:* `crates/bytesbuf/src/buf.rs:437`, `read_ext.rs:520`

**OX-034 (g3-F16)** — `ReadInspectDecision::Failed` boxes on a path shared with the hot decision · **Low** · EMPIRICAL
- *Loc:* `crates/bytesbuf_io/src/read_ext.rs:191-205` · The enum's `Failed` variant carries a `Box<dyn … **Impact Low:** no action
  needed. … **Fix:** None. Retain.

**OX-035 (g3-F17)** — `Error::caused_by` is invoked on every fallible step, including the success path's `map_err` · **Low** ·
  inferred
- *Loc:* `crates/bytesbuf_io/src/read_ext.rs:116`, `:124`, `:162`; `crates/bytesbuf_io/src/error.rs` ·
  `.map_err(crate::Error::caused_by)` appears on … **Impact Low:** no action needed. **Fix:** None. Consider `#[cold]` on …

**OX-036 (g3-F18)** — No `#[inline]` anywhere in the crate · **Low** · EMPIRICAL
- *Loc:* `crates/bytesbuf_io/src/` (all seven files) · The crate contains no `#[inline]` annotations. … **Impact Low:** no action
  needed. **Fix:** None.
- *Also cited:* `read_futures.rs:57-66`

### `cachet`

*15 findings — 4 High, 7 Medium, 4 Low. Examined:* `cachet` is the front-end of the caching stack: `Cache<K, V>` wraps a
  `DynCacheTier`, adds telemetry, request-coalescing … *Also cited:* `crates/cachet/src/cache.rs:228-231`,
  `crates/cachet_tier/src/tier.rs:46`, `crates/cachet_tier/src/tier.rs:38`

**OX-037 (g3-F19)** — `Cache::get` allocates an owned key on every lookup · **High** · inferred
- *Loc:* `crates/cachet/src/cache.rs:228-255`, specifically `let owned = key.to_owned();` at line 237 (coalesced path) and line 246
  (direct path)
- *Issue:* The signature is `pub async fn get<Q>(&self, key: &Q) -> ... where K: Borrow<Q>, Q: Hash + Eq + ToOwned<Owned = K> +
  ?Sized + Send + Sync`. …
- *Impact:* High — one malloc + memcpy + free per cache hit, on the hottest path of a crate whose reason to exist is to …
- *Fix:* The root cause is `CacheTier::get`'s `&K` parameter (see cachet_tier F39). …
- *Philosophy:* **Conflicting.** The house rule is emphatic about allocation on hot paths, but every real remediation is
  architectural — it …

**OX-038 (g3-F20)** — The same `to_owned()` pattern repeats across eight more `Cache` methods · **High** · inferred
- *Loc:* `crates/cachet/src/cache.rs:318` and `:327` (`invalidate`), `:466` (`get_or_insert`), `:557` (`get_or_insert_with`), `:637`
  (`try_get_or_insert_with`), `:714`, `:802`
- *Issue:* Identical to F19 — every method taking a borrowed `&Q` converts to an owned `K` before calling into the tier. …
- *Impact:* High — `get_or_insert_with` is the idiomatic entry point for most cache users, so in practice this path is at …
- *Fix:* Same as F19; a fix there fixes these. If F19 is resolved with a borrowed read path, `get_or_insert*` should call …

**OX-039 (g3-F21)** — An unconditional global atomic increment on every cache operation · **High** · inferred
- *Loc:* `crates/cachet/src/telemetry/cache.rs:20` (the static), `:27-29` (`next_request_id`); called from
  `crates/cachet/src/cache.rs:233`, `:283`, `:314`, `:357`, `:463`, `:711`, `:799`
- *Issue:* Every public `Cache` method begins with `let request_id = next_request_id();`, which is `NEXT_REQUEST_ID.fetch_add(1,
  Ordering::Relaxed)` on a single process-wide `AtomicU64`. …
- *Impact:* High — a `lock xadd` on one globally shared line is one of the few constructs that turns a linear-scaling …
- *Fix:* Surgical: make ID generation lazy. Either (a) skip it when `self.telemetry` has no handler and telemetry features …
- *Philosophy:* none — this is a surgical fix that preserves all observable behaviour.
- *Also cited:* `telemetry/cache.rs:130`

**OX-040 (g3-F22)** — `WithRequestId::poll` writes a thread-local and constructs a drop guard on every poll · **Medium** · inferred
- *Loc:* `crates/cachet/src/telemetry/cache.rs:55-63`, with the guard at `:45-53`; applied at `crates/cachet/src/cache.rs:253`
  (`.with_request_id(...)`) and the equivalent line in each other public method · Every cache operation's future is wrapped in
  `WithRequestId`. … **Impact Medium:** TLS access on most platforms is a few … **Fix:** Gate the wrapper: when `self.telemetry` has
  no handler and …

**OX-041 (g3-F23)** — `record_*` reads the request-ID thread-local before checking whether anyone wants it · **Medium** · inferred
- *Loc:* `crates/cachet/src/telemetry/cache.rs:205-380` — every `record_hit` (`:207`), `record_miss` (`:212`), `record_expired`
  (`:217`), `record_get_error` (`:223`), `record_inserted` (`:234`), `record_insert_error` (`:245`), `record_invalidated` (`:256`),
  `record_invalidate_error` (`:267`), `record_cleared` (`:277`), `record_clear_error` (`:283`), `record_refresh_hit` (`:295`),
  `record_refresh_miss` (`:307`), `record_insert_rejected` (`:321`), `record_eviction` (`:344`), `record_background_expired`
  (`:367`) · Each of these fifteen call sites evaluates `Self::current_request_id()` — a thread-local … **Impact Medium:** one
  wasted TLS read per tier per operation, and … **Fix:** Fully surgical and behaviour-preserving: move the …

**OX-042 (g3-F24)** — Two clock reads per tier per operation · **Medium** · inferred
- *Loc:* `crates/cachet/src/cache.rs:234` (`self.clock.stopwatch()`) and `watch.elapsed()` at `:251`;
  `crates/cachet/src/wrapper.rs:131-134` (`stopwatch()` then `elapsed()`), and again in `insert` (`:149`), `invalidate` (`:157`),
  `clear` (`:167`) · `Cache::get` starts a stopwatch and reads it. The `CacheWrapper` wrapping the tier … **Impact Medium:** Medium
  to High — for an in-memory tier this can be a … **Fix:** Same shape as F23: only start the stopwatch when something …

**OX-043 (g3-F25)** — `TimeToRefresh::try_start_refresh` clones the key before discovering it is already in flight · **Medium** ·
  inferred
- *Loc:* `crates/cachet/src/refresh.rs:79-81` · `self.in_flight.lock().insert(key.clone())` clones `K` unconditionally. … **Impact
  Medium:** one wasted `K` allocation per suppressed … **Fix:** Surgical: `let mut guard = self.in_flight.lock(); if …

**OX-044 (g3-F26)** — Refresh de-duplication uses one global `Mutex<HashSet<K>>` for all keys · **Medium** · inferred
- *Loc:* `crates/cachet/src/refresh.rs:42` (the field), `:67` (construction), `:80` and `:85` (the two lock sites) · Every key's
  refresh bookkeeping goes through a single `Mutex<HashSet<K>>`. … **Impact Medium:** bounded by how often refresh checks happen,
  but … **Fix:** Shard the set (an array of N mutex-guarded sets indexed by …

**OX-045 (g3-F27)** — `do_refresh` clones the key twice more · **Low** · inferred
- *Loc:* `crates/cachet/src/refresh.rs:113-135`, specifically `:121` (`let key = key.clone();` before the spawn) and `:128` (`let
  key = key.clone();` inside the spawned task) · After `try_start_refresh` has already cloned once … **Impact Low:** this is on the
  refresh path … **Fix:** Wrap the key in an `Arc<K>` once at …

**OX-046 (g3-F28)** — `FallbackCache::insert` clones the key when it could move it · **Low** · inferred
- *Loc:* `crates/cachet/src/fallback.rs:141-148`, specifically `:143` and `:144` · `insert` receives `key: K` **by value** and then
  … **Impact Low:** one `K` allocation per … **Fix:** Delete `.clone()` from line 144.

**OX-047 (g3-F29)** — `FallbackCache::get_from_fallback` clones both key and value to promote · **Low** · inferred
- *Loc:* `crates/cachet/src/fallback.rs:105-112`, specifically `:110` · On a fallback hit, the entry is promoted into the … **Impact
  Low:** the comment at lines 106-108 … **Fix:** Consider spawning the promotion …
- *Also cited:* `refresh.rs:125`

**OX-048 (g3-F30)** — The main `cachet` benchmark requires a non-default feature that changes what it measures · **Medium** ·
  EMPIRICAL
- *Loc:* `crates/cachet/benches/operations.rs`, via its `required-features = ["logs", "test-util"]` declaration in
  `crates/cachet/Cargo.toml` · `logs` is **not** a default feature. The crate's primary operations benchmark therefore … **Impact
  Medium:** a benchmark-validity defect rather than a … **Fix:** Split the bench, or parameterise it: run the same scenarios …

**OX-049 (g3-F31)** — `cachet`'s benchmarks measure `MockCache`, not `cachet` · **High** · inferred
- *Loc:* `crates/cachet/benches/dynamic.rs` and `crates/cachet/benches/operations.rs`, both built on
  `crates/cachet_tier/src/testing.rs:227-273`
- *Issue:* See cachet_tier F41 for the full analysis. In short: the storage tier both benchmarks use clones the key, acquires three
  separate mutexes per operation, and appends to an unbounded …
- *Impact:* High (as a benchmark-validity defect) — it means the crate's existing performance numbers cannot be trusted …
- *Fix:* Add a minimal no-op or `HashMap`-backed bench-only tier with no recording and no locking, and use it as the …

**OX-050 (g3-F32)** — `encode` calls `BytesBuf::peek()` and clones the pool per encode · **Low** · inferred
- *Loc:* `crates/cachet/src/serialize/codec.rs:95-100`, specifically `pool.clone()` at `:96` and `writer.into_inner().peek()` at
  `:99` · `pool.clone()` is a `GlobalPool` clone — an `Arc` … **Impact Low:** serialisation cost … **Fix:** None urgent. Fixing
  bytesbuf F6 (`peek` …
- *Also cited:* `crates/bytesbuf/src/buf.rs:437`

**OX-051 (g3-F33)** — `cachet` has zero Callgrind benchmarks · **Medium** · EMPIRICAL
- *Loc:* `crates/cachet/benches/` (three files, all Criterion) · See "Benchmark coverage". Called out as a finding because the
  overheads this crate … **Impact Medium:** blocks confirmation of the crate's most … **Fix:** Add `operations_cg.rs` paired with
  `operations.rs` per …

### `cachet_memory`

*5 findings — 1 High, 2 Medium, 2 Low. Examined:* `cachet_memory` is a thin adapter that presents `moka::future::Cache` as a
  `cachet_tier::CacheTier`. …

**OX-052 (g3-F34)** — `InMemoryCache::insert` clones an owned key · **Medium** · inferred
- *Loc:* `crates/cachet_memory/src/tier.rs:201-204`, specifically `self.inner.insert(key.clone(), entry).await;` at `:202` ·
  `insert` takes `key: K` **by value** and `moka`'s `Cache::insert` also takes `K` by … **Impact Medium:** one full `K` allocation
  (a `String` malloc + … **Fix:** Delete `.clone()`. Zero behaviour change; the borrow checker …
- *Also cited:* `crates/cachet_memory/benches/overhead.rs:108-142`

**OX-053 (g3-F35)** — `CacheTier::get`'s `&K` signature forces `moka`'s owned-key lookup · **High** · inferred
- *Loc:* `crates/cachet_memory/src/tier.rs:197-199`
- *Issue:* `Ok(self.inner.get(key).await)` is correct and allocation-free *here* — `moka::Cache::get` accepts `&Q where K:
  Borrow<Q>`, so `moka` itself supports the borrowed lookup. …
- *Impact:* High — it is the same allocation as F19, but this finding is what makes the fix worth doing: the payoff is …
- *Fix:* See cachet_tier F39. `cachet_memory` needs no change once the trait is generalised — `self.inner.get(key)` …
- *Philosophy:* **Conflicting** for the same reason as F19 — the fix is architectural, not surgical.
- *Also cited:* `crates/cachet_tier/src/tier.rs:46`

**OX-054 (g3-F36)** — The eviction listener runs a dynamic call per registered observer per eviction · **Low** · inferred
- *Loc:* `crates/cachet_memory/src/tier.rs:166-178` · When any listener or observer is registered, the … **Impact Low:** evictions
  are, by … **Fix:** None recommended. The guard already …
- *Also cited:* `notification.rs:27-40`

**OX-055 (g3-F37)** — `EntryExpiry` calls `CacheEntry::ttl()` on every create and update · **Low** · EMPIRICAL
- *Loc:* `crates/cachet_memory/src/tier.rs:221-236` · `moka`'s per-entry `Expiry` trait is invoked on … **Impact Low:** the entry is
  almost … **Fix:** None. Per-entry expiry is the reason …

**OX-056 (g3-F38)** — `cachet_memory` has no Callgrind benchmark and non-conforming group names · **Medium** · EMPIRICAL
- *Loc:* `crates/cachet_memory/benches/overhead.rs:24`, `:72`, `:110` · The Criterion groups are named `"get_hit"`, `"get_miss"` and
  `"insert"` rather than … **Impact Medium:** as a benchmark-validity issue. … **Fix:** Add
  `crates/cachet_memory/benches/overhead_cg.rs` pairing the …

### `cachet_service`

*6 findings — 4 Medium, 2 Low. Examined:* `cachet_service` (648 lines across four files) bridges `cachet_tier::CacheTier` to the
  `layered::Service` abstraction, so a …

**OX-057 (g3-F46)** — `ServiceAdapter::insert` clones a key it already owns · **Medium** · inferred
- *Loc:* `crates/cachet_service/src/adapter.rs:75-81`, specifically `:76` · `async fn insert(&self, key: K, entry: CacheEntry<V>)`
  receives `key` **by value**, and … **Impact Medium:** one full `K` allocation per insert, trivially … **Fix:** Delete `.clone()`
  at line 76. The borrow checker verifies …
- *Also cited:* `crates/cachet_service/src/request.rs:52`, `request.rs:52`

**OX-058 (g3-F47)** — `CacheServiceExt::insert` clones a key it already owns · **Medium** · inferred
- *Loc:* `crates/cachet_service/src/ext.rs:38-44`, specifically `:39` · Identical to F46 in the extension-trait path: `let req =
  InsertRequest { key … **Impact Medium:** same as F46. Recorded separately because the … **Fix:** Delete `.clone()` at line 39,
  i.e. …

**OX-059 (g3-F48)** — The `get` and `invalidate` clones are forced by the owned-request design · **Medium** · inferred
- *Loc:* `crates/cachet_service/src/adapter.rs:68`, `:84`; `crates/cachet_service/src/ext.rs:31`, `:47` · Unlike F46/F47, these four
  clones are **not** redundant: `get` and `invalidate` receive … **Impact Medium:** the third allocation is the one this crate …
  **Fix:** Make the request types borrow: `GetRequest<'a, K> { key: &'a …
- *Philosophy:* **Conflicting** if pursued — changing the public request types to borrowed forms is architectural, and `Cow<K>` in a
  public API is …
- *Also cited:* `cache.rs:237`

**OX-060 (g3-F49)** — `cachet_service` has no benchmarks at all · **Medium** · EMPIRICAL
- *Loc:* `crates/cachet_service/` — no `benches/` directory, and no `[[bench]]` entries in `Cargo.toml` · No Criterion and no
  Callgrind coverage. The crate's entire value proposition is that … **Impact Medium:** a coverage gap on an abstraction whose
  overhead … **Fix:** Add `crates/cachet_service/benches/adapter.rs` (Criterion) …

**OX-061 (g3-F50)** — Two parallel implementations of the same conversion · **Low** · inferred
- *Loc:* `crates/cachet_service/src/adapter.rs:61-100` and `crates/cachet_service/src/ext.rs:24-59` · `ServiceAdapter`'s `CacheTier`
  impl and … **Impact Low:** code-size and i-cache … **Fix:** Have `ServiceAdapter`'s `CacheTier` …

**OX-062 (g3-F51)** — `CacheOperation<K, V>` is sized by its largest variant · **Low** · EMPIRICAL
- *Loc:* `crates/cachet_service/src/request.rs:14-24` · The enum has four variants: `Get(GetRequest<K>)` … **Impact Low:** 80 bytes
  is one cache line … **Fix:** None recommended without the benchmark …

### `cachet_tier`

*7 findings — 3 High, 3 Medium, 1 Low. Examined:* `cachet_tier` is small (1,054 lines across six files) but it is the most
  consequential crate in this group, because it …

**OX-063 (g3-F39)** — `CacheTier::get` and `invalidate` take `&K`, not a borrowed form · **High** · inferred
- *Loc:* `crates/cachet_tier/src/tier.rs:46` (`get`) and `:54` (`invalidate`)
- *Issue:* `fn get(&self, key: &K) -> impl Future<...>`. Because the parameter is `&K` rather than a `&Q where K: Borrow<Q>`, every
  caller that holds a borrowed key of a …
- *Impact:* High — this is the root cause of the group's top finding. One heap allocation per cache hit for the standard …
- *Fix:* Generalising to `fn get<Q>(&self, key: &Q) where K: Borrow<Q>` is the correct signature but is **not …
- *Philosophy:* **Conflicting.** `docs/performance.md` states that allocation is the root of all evil and that nothing should
  allocate on the hot …
- *Also cited:* `crates/cachet/src/cache.rs:237`, `crates/cachet_memory/src/tier.rs:197`

**OX-064 (g3-F40)** — `dyn(box)` erasure boxes a future per tier per operation · **High** · inferred
- *Loc:* `crates/cachet_tier/src/tier.rs:38` — `#[dynosaur::dynosaur(pub(crate) DynCacheTier = dyn(box) CacheTier, bridge(none))]`
- *Issue:* The `CacheTier` trait uses RPITIT (`-> impl Future<Output = ...> + Send`), which is allocation-free when used
  generically. …
- *Impact:* High — a second unavoidable allocation on the hottest path, and it scales with stack depth. …
- *Fix:* The genuine fix is to stop erasing: make `cachet::Cache` generic over `CT: CacheTier<K, V>` so the RPITIT future …
- *Philosophy:* **Conflicting**, as F39 — the correct fix is architectural and source-breaking.
- *Also cited:* `src/dynamic.rs:26`

**OX-065 (g3-F41)** — `MockCache` is used as the storage backend for `cachet`'s benchmarks · **High** · inferred
- *Loc:* `crates/cachet_tier/src/testing.rs:224-277`, in particular `get` at `:229-236`; used by `crates/cachet/benches/dynamic.rs`
  and `crates/cachet/benches/operations.rs`
- *Issue:* `MockCache::get` does all of the following per call: 1. `key.clone()` to build a `CacheOp::Get` (line 230) — a heap
  allocation for `String` keys; 2. …
- *Impact:* High **as a benchmark-validity defect**. `MockCache` itself is a fine test double and its cost is irrelevant …
- *Fix:* Add a bench-only no-op or `HashMap`-backed tier with no recording and no locking, and use it as the storage for …

**OX-066 (g3-F42)** — `CacheEntry<V>` carries 32 bytes of cold metadata regardless of `V` · **Medium** · EMPIRICAL
- *Loc:* `crates/cachet_tier/src/entry.rs:38-43` · The struct is `{ value: V, cached_at: Option<SystemTime>, ttl: Option<Duration>
  }`. … **Impact Medium:** for small values this materially reduces the … **Fix:** The metadata could be compressed to 12 bytes — a
  `u64` of …
- *Philosophy:* **Conflicting.** `docs/performance.md` requires staying idiomatic and justifying deviations from ecosystem patterns.
  …
- *Also cited:* `cachet_memory/src/tier.rs:221-236`

**OX-067 (g3-F43)** — `DynamicCache` implements `CacheTier`, so nesting it double-boxes · **Medium** · inferred
- *Loc:* `crates/cachet_tier/src/dynamic.rs:49-68` · `DynamicCache<K, V>` wraps `Arc<DynCacheTier<'static, K, V>>` — an
  already-erased … **Impact Medium:** doubles the F40 cost for a configuration that … **Fix:** Either document the hazard
  prominently on `DynamicCache`, or …

**OX-068 (g3-F44)** — `cachet_tier` has no benchmarks at all · **Medium** · EMPIRICAL
- *Loc:* `crates/cachet_tier/` — no `benches/` directory · The crate that defines the trait whose shape imposes the group's two
  largest costs (F39 … **Impact Medium:** a coverage gap that directly blocks the two … **Fix:** Add
  `crates/cachet_tier/benches/erasure.rs` (Criterion) and …

**OX-069 (g3-F45)** — `CacheEntry`'s ten small public accessors carry no `#[inline]` · **Low** · inferred
- *Loc:* `crates/cachet_tier/src/entry.rs:49`, `:61`, `:74`, `:87`, `:96`, `:107`, `:112`, `:118`, `:124`, `:133` · `cached_at()`,
  `ttl()`, `value()` … **Impact Low:** and this needs an important … **Fix:** None recommended without measurement.
- *Also cited:* `cachet_memory/src/tier.rs:224`

### `data_privacy`

*8 findings — 4 Medium, 4 Low. Examined:* This is the group's most performance-sensitive **product** code. Redaction runs on the
  telemetry path, which means it runs …

**OX-070 (g8-F14)** — Every redaction hashes two strings before doing any redaction work · **Medium** · EMPIRICAL+INFERRED
- *Loc:* `crates/data_privacy/src/redaction_engine.rs:113-121` (`Redactor::redact` for `RedactionEngine`) delegating to
  `crates/data_privacy/src/redaction_engine_inner.rs:31-37` (`resolve`); the key type is `DataClass` at
  `crates/data_privacy_core/src/data_class.rs:17-21`. · The policy lookup is an `FxHashMap<DataClass, RedactionPolicy>::get`. …
  **Impact Medium:** Medium, and the highest-impact product-code finding in … **Fix:** Intern the class identity so the map key is a
  machine word …
- *Also cited:* `crates/data_privacy_macros_impl/src/taxonomy.rs:95`

**OX-071 (g8-F15)** — `redacts()` and `redact()` each perform an independent lookup, so a guarded call hashes twice · **Medium** ·
  inferred
- *Loc:* `crates/data_privacy/src/redaction_engine.rs:109-111` (`redacts`) and `:113-121` (`redact`). · Both methods call `resolve`
  independently. The natural defensive call site — `if … **Impact Medium:** Medium, conditional on call sites actually using the …
  **Fix:** Offer a combined entry point that resolves once and returns …

**OX-072 (g8-F16)** — Redaction formats the payload in full and then discards it, under the default fallback policy · **Medium** ·
  EMPIRICAL+INFERRED
- *Loc:* `crates/data_privacy/src/sensitive.rs:81-137` (the `Display` and `Debug` impls for `Sensitive<T>`), the identical generated
  code at `crates/data_privacy_macros_impl/src/classified.rs:104-156`, the default fallback at
  `crates/data_privacy/src/redaction_engine_inner.rs:76` (`SimpleRedactorMode::Erase`), and the erase implementation at
  `crates/data_privacy/src/redactors/simple_redactor.rs:78-81`. · Neither `Sensitive`'s formatting impls nor the generated
  `#[classified]` … **Impact Medium:** Medium. This is the default configuration, so it is what … **Fix:** Query the policy before
  formatting. …

**OX-073 (g8-F17)** — 128 stack bytes are zero-initialised on every redacted format call, then immediately overwritten · **Low** ·
  inferred
- *Loc:* `crates/data_privacy/src/sensitive.rs:84` and `:117` (`let mut local_buf = [0u8; STACK_BUFFER_SIZE];`, with
  `STACK_BUFFER_SIZE = 128` at `crates/data_privacy/src/sensitive.rs:9`); the same pattern is generated at
  `crates/data_privacy_macros_impl/src/classified.rs:113` and `:140`. · The buffer is created zeroed and then written … **Impact
  Low:** Low to Medium. 128 bytes of … **Fix:** Use `MaybeUninit<[u8; 128]>` with the …
- *Philosophy:* Mildly conflicting — the philosophy prefers idiomatic Rust and surgical changes, and reaching for `MaybeUninit` is a
  step away …
- *Also cited:* `crates/data_privacy/src/sensitive.rs:96`, `crates/data_privacy_macros_impl/src/classified.rs:123`

**OX-074 (g8-F18)** — `Sensitive<T>` embeds a 48-byte `DataClass` by value · **Medium** · EMPIRICAL
- *Loc:* `crates/data_privacy/src/sensitive.rs:16-19`. · The wrapper stores a `DataClass` inline. Since `DataClass` is two `Cow`s,
  `Sensitive<T>` … **Impact Medium:** Medium. `Sensitive<u8>` measured **56 bytes** — seven … **Fix:** Store `&'static DataClass` (8
  bytes) instead of `DataClass`. …
- *Also cited:* `crates/data_privacy_macros_impl/src/taxonomy.rs:95`

**OX-075 (g8-F19)** — Redactors route single-string writes through the full formatting machinery · **Low** · inferred
- *Loc:* `crates/data_privacy/src/redactors/simple_redactor.rs:88`, `:99`, `:123`;
  `crates/data_privacy/src/redactors/xxh3_redactor.rs:60`; `crates/data_privacy/src/redactors/rapidhash_redactor.rs:39`. · These
  sites use `write!(output, "{}", s)` where … **Impact Low:** Low individually — LLVM optimises … **Fix:** Replace `write!(output,
  "{}", s)` with …

**OX-076 (g8-F20)** — Off-by-one boundary sends exactly-`ASTERISKS.len()` values down the slow path · **Low** · inferred
- *Loc:* `crates/data_privacy/src/redactors/simple_redactor.rs:98` and `:111`. · The fast path is guarded by `len < … **Impact
  Low:** it affects one input length … **Fix:** Change `<` to `<=` at both sites, and …
- *Also cited:* `crates/data_privacy/src/sensitive.rs:190`

**OX-077 (g8-F21)** — Only one `#[inline]` in the crate, and it is not on the hot path · **Low** · inferred
- *Loc:* the sole `#[inline]` is at `crates/data_privacy/src/redactors/mod.rs:13` on `u64_to_hex_array`. Nothing on
  `RedactionEngine::redact`, `RedactionEngine::redacts`, `Sensitive::data_class`, or the `Redactor` trait's small methods. ·
  `redact` and `redacts` are non-generic exported … **Impact Low:** Low. The bodies are not trivial … **Fix:** Narrow set only: the
  trivial accessors …
- *Philosophy:* Same tension as F9 — cannot be validated by a benchmark in this repository because `[profile.bench]` uses fat LTO.

### `data_privacy_core`

*3 findings — 1 Medium, 2 Low. Examined:* A small crate holding `DataClass`, `Redacted*` traits and `Classified`. …

**OX-078 (g8-F22)** — `DataClass` is a 48-byte two-`Cow` struct used as a hash-map key on the hot path · **Medium** ·
  EMPIRICAL+INFERRED
- *Loc:* `crates/data_privacy_core/src/data_class.rs:17-21` (the struct and its derived `Hash` / `PartialEq` / `Eq`). · The type
  models a class identity as two owned-or-borrowed strings. … **Impact Medium:** it is the root cause of the group's top … **Fix:**
  Add a cached hash computed in the `const fn` constructor, so …

**OX-079 (g8-F23)** — `DataClass::clone` allocates twice when the class was deserialised · **Low** · EMPIRICAL
- *Loc:* `crates/data_privacy_core/src/data_class.rs:17-21` (derived `Clone`). · Cloning a `DataClass` whose `Cow`s are `Borrowed` …
  **Impact Low:** Low, and conditional: it only … **Fix:** No code change. Document the …

**OX-080 (g8-F24)** — No `#[inline]` on the accessors that the redaction path calls · **Low** · inferred
- *Loc:* `crates/data_privacy_core/src/data_class.rs:44` (`taxonomy()`) and `:50` (`name()`); also `Classified::data_class` in
  `crates/data_privacy_core/src/classified.rs`. The crate contains **zero** `#[inline]` annotations. · These are trivial non-generic
  exported functions … **Impact Low:** Low, but this is the textbook case … **Fix:** Annotate `taxonomy()`, `name()` and …
- *Philosophy:* Same benchmark-blindness caveat as F9 and F21 — the repository's `[profile.bench]` fat LTO means no benchmark here
  can demonstrate …

### `data_privacy_macros`

**No performance issues identified.**

A 47-line re-export shim: it declares the proc-macro entry points and forwards every one of them to …

### `data_privacy_macros_impl`

*3 findings — 1 Medium, 2 Low. Examined:* Generates the `#[classified]`, `#[taxonomy]` and derive machinery. …

**OX-081 (g8-F25)** — Generated `#[classified]` formatting duplicates the 128-byte zeroed-buffer pattern · **Medium** · inferred
- *Loc:* `crates/data_privacy_macros_impl/src/classified.rs:104-156`, in particular the buffer declarations at `:113` and `:140` and
  the `from_utf8_unchecked` calls at `:123` and `:150`. · The generated `RedactedDisplay` / `RedactedDebug` impls are a textual copy
  of … **Impact Medium:** it multiplies F16 and F17 across every … **Fix:** Extract the shared formatting body into a public helper
  …
- *Also cited:* `crates/data_privacy/src/sensitive.rs:81-137`

**OX-082 (g8-F26)** — `#[taxonomy]`-generated `data_class()` returns a 48-byte `DataClass` by value while a zero-cost `&'static` is
  right there · **Low** · EMPIRICAL+INFERRED
- *Loc:* `crates/data_privacy_macros_impl/src/taxonomy.rs:95` (the `AsRef<DataClass>` arms, which correctly emit `const {
  &DataClass::new(...) }`), `:109-111` (the `classify_*` helpers) and `:123` (`data_class()`). · The macro emits *both* a zero-cost
  `&'static … **Impact Low:** Low to Medium. In many cases LLVM … **Fix:** Steer callers to the `&'static` path …

**OX-083 (g8-F27)** — Generated code is emitted per type rather than delegating to shared helpers · **Low** · inferred
- *Loc:* `crates/data_privacy_macros_impl/src/classified.rs:104-156` and `crates/data_privacy_macros_impl/src/taxonomy.rs:95-125`. ·
  Both macros emit substantial inline bodies per … **Impact Low:** Low. Recorded because compile-time … **Fix:** As F25 — emit calls
  to shared helpers.

### `fetch`

*10 findings — 2 High, 5 Medium, 3 Low. Examined:* `fetch` is the front door of the HTTP stack: it owns the client, the builder, the
  layered pipeline assembly and the standard …

**OX-084 (g5-F1)** — `Logging::execute` redacts the URL on every request before the await · **High** · inferred
- *Loc:* `crates/fetch/src/handlers/logging.rs:102-107` (the eager call is line 107), consumed at `:122` and `:139`; helper
  `redacted_path_and_query` at `:151-159`.
- *Issue:* `execute` computes `let redacted_path_and_query = redacted_path_and_query(&input, &self.redaction_engine);` *before* the
  `async move` block that starts on line 109. …
- *Impact:* High — every request through the standard pipeline pays it; it is on by default; and the cost scales with URL …
- *Fix:* Gate the computation on whether anything will consume it. The naive `tracing::enabled!(Level::DEBUG)` gate is …

**OX-085 (g5-F2)** — Default Tokio client erases the pipeline twice and shares one mutex across all threads · **High** · inferred
- *Loc:* `crates/fetch/src/handlers/transport.rs:15,19`; `crates/fetch/src/pipeline/builder.rs:124,146`;
  `crates/fetch/src/tokio.rs:77`; `crates/fetch/src/client_builder.rs:445-447`; `crates/fetch/src/client.rs:373-377`; cross-crate
  evidence in `crates/layered/src/dynamic.rs:70-92`.
- *Issue:* There are **two** independent `layered::DynamicService` type erasures on the per-request path. The inner one is
  `TransportHandler`, which is declared as a newtype around …
- *Impact:* High — it is a contention point, not a constant cost, so its severity grows with core count and concurrency …
- *Fix:* Two independent, separately shippable steps. (a) Surgical: the `Isolated` variant already exists and routes …
- *Also cited:* `transport.rs:15`, `pipeline/builder.rs:124`, `dynamic.rs:78`, `dynamic.rs:70-72`, `tokio.rs:77`,
  `client_builder.rs:445-447`, `crates/layered/src/dynamic.rs:73-87`
- *Cross-reference:* **the same underlying contention issue** as OX-139 / OX-140 in §layered
  (`crates/layered/src/dynamic.rs:73-87`), seen from the client side. Stated in full in §layered; do not count it twice.

**OX-086 (g5-F3)** — Two `Metrics` layers each allocate a `String` for the host per request · **Medium** · inferred
- *Loc:* `crates/fetch/src/handlers/metrics.rs:312` (and `fill_error_attributes` at `:335`);
  `crates/fetch/src/pipeline/standard.rs:95,111`. · `metrics.rs:312` does `val.host().to_string()` to populate the `server.address`
  telemetry … **Impact Medium:** two to N allocations per request … **Fix:** Attribute values in the OpenTelemetry-style `KeyValue`
  model …

**OX-087 (g5-F4)** — Every request is wrapped in a timeout future even when no timeout was requested · **Medium** · inferred
- *Loc:* `crates/fetch/src/client.rs:345-370` (the wrap is line 363; the default is established at `:346-349`);
  `crates/tick/src/future_ext.rs:33-38`; `crates/tick/src/delay.rs:72-78` and `:99-105`; `crates/tick/src/clock.rs:176-180`. ·
  `Client::execute` reads the `ResponseTimeout` extension and falls back to `Duration::MAX` … **Impact Medium:** small per-request
  constant, but unconditional … **Fix:** Branch on `timeout == Duration::MAX` and select between the …
- *Also cited:* `delay.rs:72-78`, `delay.rs:99-105`

**OX-088 (g5-F5)** — Telemetry attribute SmallVec inflates the pipeline future · **Low** · EMPIRICAL
- *Loc:* `crates/fetch/src/telemetry.rs:44`; `crates/fetch/src/handlers/metrics.rs:340-385`. · `TelemetryAttributes` is … **Impact
  Low:** this is a size/copy cost … **Fix:** Consider whether nine inline slots is …

**OX-089 (g5-F6)** — Zero `#[inline]` across 62 public functions · **Medium** · inferred
- *Loc:* crate-wide; census of `crates/fetch/src`. · `fetch` exports 62 public functions and carries **zero** `#[inline]`
  attributes. … **Impact Medium:** diffuse rather than concentrated, but it … **Fix:** Annotate the small, hot, non-generic public
  functions: the …

**OX-090 (g5-F7)** — TLS feature flags: enabling both backends silently links a dead one · **Medium** · inferred
- *Loc:* `crates/fetch/Cargo.toml:54-79`; `crates/fetch/src/tokio.rs:135-165`. · `default = []`, `tls = ["rustls"]`, and the `tokio`
  feature force-enables … **Impact Medium:** build-time and binary-size cost rather than … **Fix:** Emit a `compile_error!` (or at
  minimum a …

**OX-091 (g5-F8)** — Router clone inserted into extensions per request when alternatives exist · **Low** · inferred
- *Loc:* `crates/fetch/src/client.rs:354-356`. · When `has_alternatives()` is true, `execute` does … **Impact Low:** only on clients
  configured … **Fix:** If the router is already `Arc`-backed …

**OX-092 (g5-F9)** — Convenience API re-parses the URI on every call · **Low** · inferred
- *Loc:* `crates/fetch/src/client.rs:98-104` (and the same pattern at `:135`, `:168`, `:195`, `:223`, `:256`, `:287`); documented at
  `:68-71` and `:110-113`. · The request-construction methods take `impl … **Impact Low:** per-request URI parsing is … **Fix:** No
  code change proposed; this is a …

**OX-093 (g5-F10)** — Pooled dispatch: boxed selector closure plus a globally shared atomic and two divisions per request ·
  **Medium** · inferred
- *Loc:* `crates/fetch/src/handlers/dispatch.rs:65,71,92,116-130`; `crates/fetch_options/src/pooling.rs:407,425-447`. · In
  `DispatchMode::Pooled`, the pool selector is a `Box<dyn Fn>` (`PoolSelector` … **Impact Medium:** dynamic dispatch plus a
  contended atomic plus … **Fix:** Replace the shared counter with a per-thread counter (the …
- *Also cited:* `dispatch.rs:65,71`, `pooling.rs:425-447`, `pooling.rs:407`

### `fetch_azure`

*5 findings — 3 Medium, 2 Low. Examined:* `fetch_azure` adapts `fetch` to the `azure_core::HttpClient` trait. …

**OX-094 (g5-F19)** — Response header conversion allocates a HashMap and two Strings per header · **Medium** · inferred
- *Loc:* `crates/fetch_azure/src/client.rs:138-155`. · `to_headers` builds a `HashMap`, then for each response header inserts …
  **Impact Medium:** Medium (arguably High for header-heavy responses) — it … **Fix:** Check whether `azure_core::Headers` can be
  constructed from …

**OX-095 (g5-F20)** — Response body converted with `BytesView::to_bytes`, which the source crate documents as slow · **Medium** ·
  inferred
- *Loc:* `crates/fetch_azure/src/client.rs:~131` (`.map_ok(|view| view.to_bytes())`); the implementation and its warning at
  `crates/bytesbuf/src/bytes_compat/to_bytes.rs:34-45` (doc) and `:55-82` (code). · Every body chunk is passed through
  `BytesView::to_bytes()`. … **Impact Medium:** potentially a full copy of the response body … **Fix:** The
  `azure_core::PinnedStream` signature requires `Bytes`, so …

**OX-096 (g5-F21)** — Method and URL passed as `&str`, forcing a re-parse per request · **Medium** · inferred
- *Loc:* `crates/fetch_azure/src/client.rs:~55`. · The adapter passes the method and URL into `client.request(...)` as string
  slices. … **Impact Medium:** per request, unconditional, and entirely … **Fix:** Construct the `http::Uri` from the parsed `Url`'s
  components …

**OX-097 (g5-F22)** — `#[async_trait]` boxes a future per request · **Low** · inferred
- *Loc:* `crates/fetch_azure/src/client.rs` — the `execute_request` implementation. · `#[async_trait]` desugars to `Pin<Box<dyn
  Future … **Impact Low:** one allocation per request … **Fix:** None available locally. …

**OX-098 (g5-F23)** — `Box::pin` on the response body per response · **Low** · inferred
- *Loc:* `crates/fetch_azure/src/client.rs` — the response construction path. · The body stream is `Box::pin`ned to satisfy …
  **Impact Low:** one allocation per response … **Fix:** None available locally.

### `fetch_hyper`

*4 findings — 1 Medium, 3 Low. Examined:* `fetch_hyper` is the hyper-backed transport. The good news dominates: the actual
  per-request path, `HyperHandler::execute` …

**OX-099 (g5-F11)** — Two full clones of the connect input per connection, plus histogram and clock clones · **Low** · inferred
- *Loc:* `crates/fetch_hyper/src/connection/client_connector.rs:106-107` (the two `input.clone()` calls), `:110` and `:121`
  (`Histogram::clone()`), `:131` (`clock.clone()`). · `client_connector` clones the connect input twice … **Impact Low:**
  per-connection, not … **Fix:** Restructure to clone once and share, or …
- *Philosophy:* **Conflicting.** `docs/performance.md` explicitly deprioritises first-insert and setup costs, and connection
  establishment is …

**OX-100 (g5-F12)** — Connection telemetry allocates a heap Vec and two Strings per connect · **Low** · inferred
- *Loc:* `crates/fetch_hyper/src/telemetry.rs:23-28` and `:35-40`. · Both attribute-building functions construct a … **Impact Low:**
  per-connection. … **Fix:** Use `SmallVec` for consistency with …

**OX-101 (g5-F13)** — Zero `#[inline]` across 17 public functions · **Medium** · inferred
- *Loc:* crate-wide; census of `crates/fetch_hyper/src`. · Same structural issue as F6: no `#[inline]` anywhere, and
  `[profile.release]` provides no … **Impact Medium:** this is the one finding in this crate that … **Fix:** Annotate the small
  delegating functions on the transport …

**OX-102 (g5-F14)** — TLS connector cloned wholesale and boxed per connection · **Low** · inferred
- *Loc:* `crates/fetch_hyper/src/tls/connector.rs:168`, `:177` (`let mut c = c.clone();`), `:171` and `:180` (`Box::pin(s) as
  Pin<Box<dyn HyperIo>>`). · Each connection attempt clones the entire … **Impact Low:** per-connection, and the … **Fix:** The
  stream erasure could be replaced by …

### `fetch_options`

*2 findings — 2 Medium. Examined:* `fetch_options` is a pure configuration crate (about 1,470 lines of `src`) holding the option
  types for pooling, timeouts …

**OX-103 (g5-F17)** — Trivial `Copy` accessors on the per-request path are not `#[inline]` · **Medium** · inferred
- *Loc:* `crates/fetch_options/src/connection_info.rs:71` (`age`), `:77` (`pool_index`), `:83` (`is_poisoned`), `:101` (`max_age`),
  `:111` (`is_expired`); `crates/fetch_options/src/pooling.rs:278` (`resolve`), `:325` (`index`). Census: 25 public functions, zero
  `#[inline]`. · These are one-line accessors returning `Copy` values, called from `fetch` and … **Impact Medium:** individually
  tiny, but `is_expired` and … **Fix:** Add `#[inline]` to the listed accessors. …

**OX-104 (g5-F18)** — Pool selection performs a shared atomic RMW and two runtime divisions per request · **Medium** · inferred
- *Loc:* `crates/fetch_options/src/pooling.rs:407` (the shared `AtomicU32`), `:425-447` (`PoolSelectionStrategy::select`). · See F10
  for the full analysis — this is the same defect viewed from the crate that owns … **Impact Medium:** cross-referenced with F10;
  counted once for … **Fix:** Per-thread counters via the workspace's `thread_aware`; mask …

### `fetch_tls`

*2 findings — 2 Low. Examined:* `fetch_tls` is a configuration crate: it builds TLS backends, maps ALPN protocols and loads client
  identities. …

**OX-105 (g5-F15)** — Zero `#[inline]` across 21 public functions · **Low** · inferred
- *Loc:* crate-wide; census of `crates/fetch_tls/src`. Representative: `crates/fetch_tls/src/alpn.rs:12-24` (`map_to_alpn`). · No
  `#[inline]` anywhere. `map_to_alpn` is a tiny … **Impact Low:** most of this crate's surface … **Fix:** Annotate only the small
  accessors that …

**OX-106 (g5-F16)** — `write_pem_block` base64-encodes into a temporary String then copies into the output buffer · **Low** ·
  inferred
- *Loc:* `crates/fetch_tls/src/client_identity.rs:138-143`. · The function allocates a fresh `String` for the … **Impact Low:** this
  runs once at … **Fix:** Encode directly into `out`. …
- *Philosophy:* **Conflicting.** This is squarely a setup cost, which `docs/performance.md` tells us to deprioritise. …

### `fetch_winhttp`

**No performance issues identified.**

`crates/fetch_winhttp` is a 36-line design-only placeholder. There is no implementation: no request path, no connection …

### `fundle`

*1 finding — 1 Low. Examined:* The runtime crate is 147 lines and is essentially documentation plus re-exports — there is no logic
  in it. …

**OX-107 (g8-F28)** — The crate documents itself as providing "zero-cost abstractions" while two of its three macros generate deep
  clones · **Low** · inferred
- *Loc:* `crates/fundle/src/lib.rs:9` (the claim); the generated clones are at `crates/fundle_macros_impl/src/deps.rs:69` and
  `crates/fundle_macros_impl/src/newtype.rs:64`; the claim is repeated in the macro documentation at
  `crates/fundle_macros/src/lib.rs:174-175` and `:236`. · `#[bundle]` genuinely is zero-cost (see the … **Impact Low:** Low as a
  runtime cost — the clones … **Fix:** Either narrow the documentation claim …

### `fundle_macros`

**No performance issues identified.**

The proc-macro entry-point shim for `fundle_macros_impl`, structurally identical to `data_privacy_macros`. …

### `fundle_macros_impl`

*4 findings — 3 Low, 1 —. Examined:* Where fundle's real codegen lives. `#[bundle]` is a genuine success — the generated `AsRef`
  impls return references and the …

**OX-108 (g8-F29)** — `#[fundle::deps]` deep-clones every field it extracts · **Low** · inferred
- *Loc:* `crates/fundle_macros_impl/src/deps.rs:69`. · For each field the macro emits `<T as … **Impact Low:** Low to Medium
  depending on what … **Fix:** Emit borrows where the target type …

**OX-109 (g8-F30)** — `#[newtype]` clones the wrapped value on construction · **Low** · inferred
- *Loc:* `crates/fundle_macros_impl/src/newtype.rs:64`. · Emits `Self(x.as_ref().clone())`. … **Impact Low:** construction path,
  same … **Fix:** Provide an owning constructor alongside …

**OX-110 (g8-F31)** — `syn` with `extra-traits` in `[dependencies]` · **—** · inferred
- *Loc:* `crates/fundle_macros_impl/Cargo.toml`. · identical to **F10** in `ohno_macros` — …

**OX-111 (g8-F32)** — Generated bundle plumbing is O(N) impls each carrying N type parameters · **Low** · inferred
- *Loc:* `crates/fundle_macros_impl/src/bundle.rs:540-620` (`generate_select_macro` and `generate_builder_export_impls`). · For an
  N-field bundle, the macro emits on the … **Impact Low:** Low, and purely compile-time — … **Fix:** No action recommended without a
  …

### `http_extensions`

*10 findings — 6 Medium, 4 Low. Examined:* This is the weakest crate in the scope on both axes. **Inlining:** 114 public functions,
  **zero** `#[inline]` annotations …

**OX-112 (g4-H1)** — Zero `#[inline]` across 114 public functions, including per-request accessors · **Medium** · EMPIRICAL+INFERRED
- *Loc:* crate-wide. Representative sites: `crates/http_extensions/src/extensions/header_map_ext.rs` (all methods),
  `crates/http_extensions/src/extensions/status_ext.rs` (`ensure_success`, `recovery`),
  `crates/http_extensions/src/extensions/request_ext.rs:40-55`, `crates/http_extensions/src/uri_template_label.rs` (`as_str`),
  `crates/http_extensions/src/routing/router_context.rs` (all getters), `crates/http_extensions/src/body/mod.rs` (`content_length`,
  `is_empty`) · Every one of these is a small, non-generic, exported function on a per-request path — the … **Impact Medium:**
  to-High in aggregate — individually trivial … **Fix:** Add `#[inline]` to the small non-generic public accessors. …
- *Philosophy:* none — this finding *aligns* with `docs/performance.md` rule 1. The conflict is between the repository's benchmark
  profile and its …

**OX-113 (g4-H2)** — `Router::resolve_request_uri` performs a `Uri` clone that is dead on the common path · **Medium** · EMPIRICAL
- *Loc:* `crates/http_extensions/src/routing/router.rs:284-326`, specifically `:294` · The function clones a `Uri` at `:291`
  (`uris.original().clone()`), `:292` … **Impact Medium:** `http::Uri` is `Bytes`-backed so the clone is … **Fix:** Surgical.
  Restructure so `original` is moved into the …

**OX-114 (g4-H3)** — `Router::resolve_request_uri` clones the resolved `Uri` for hand-built requests · **Low** · inferred
- *Loc:* `crates/http_extensions/src/routing/router.rs:306-307` · `resolved.clone()` on the branch taken when the … **Impact Low:**
  one atomic increment, on the … **Fix:** Return a borrow, or restructure to …

**OX-115 (g4-H4)** — A `BaseUri` is cloned per request by the fixed resolver and by the fallback closure · **Low** · inferred
- *Loc:* `crates/http_extensions/src/routing/router.rs:332` (`Resolver:: Fixed(base_uri) => Some(base_uri.clone())`) and `:162` (the
  `fallback` closure's capture clone) · `BaseUri` is `{ origin: Origin { scheme … **Impact Low:** three atomics per request. …
  **Fix:** Have `Resolver::Fixed` return …

**OX-116 (g4-H5)** — `ExtensionsExt::uri_template_label` allocates a `String` per call for non-templated URIs · **Medium** ·
  inferred
- *Loc:* `crates/http_extensions/src/extensions/extensions_ext.rs:22-33`, specifically `:28`; root cause at
  `crates/templated_uri/src/path_and_query.rs:71-76` · The method returns an owned `UriTemplateLabel`. … **Impact Medium:** a
  per-request allocation on the observability … **Fix:** Two options, both contained. (a) Have `template()` return …
- *Also cited:* `templated_uri/src/path_and_query.rs:82-86`

**OX-117 (g4-H6)** — `RequestExt::resolve_uri` clones a `Uri` per call with no borrowing alternative in the public API · **Medium**
  · inferred
- *Loc:* `crates/http_extensions/src/extensions/request_ext.rs:40-55` · `resolve_uri` returns an owned `Uri`, cloning per call. …
  **Impact Medium:** one `Bytes` refcount increment per call, and … **Fix:** Add a borrowing variant. `Uri` is not trivially
  borrowable as …

**OX-118 (g4-H7)** — `HttpRequestBuilder::build` performs three separate `Extensions::insert` calls plus a path render · **Medium**
  · EMPIRICAL
- *Loc:* `crates/http_extensions/src/http_request_builder.rs:359-391`, specifically `:378-379` and `:382`, `:384`, `:387` · `build`
  calls `uri.to_path_and_query()` at `:378` (renders a string) … **Impact Medium:** this is once per outbound request, not per …
  **Fix:** (a) Bundle the three extension values into a single struct …

**OX-119 (g4-H8)** — `collect_with_limit` accumulates into an uncapacitied `Vec` and has no single-fragment fast path · **Medium** ·
  inferred
- *Loc:* `crates/http_extensions/src/body/mod.rs:593-622`, specifically `:599` and `:606` · The fragment accumulator starts as
  `Vec::new()` at `:599` and grows by doubling as frames … **Impact Medium:** body collection is per-response and this is the …
  **Fix:** Peek the first frame; if the body then reports complete …

**OX-120 (g4-H9)** — Streaming bodies are `Pin<Box<dyn Body>>`, adding an allocation and a virtual call per poll · **Low** ·
  inferred
- *Loc:* `crates/http_extensions/src/body/mod.rs:572-581`, specifically the `Kind::Body(Pin<Box<dyn Body<…>>>, HttpBodyOptions)`
  variant at `:580` · Every streaming body is boxed and polled through … **Impact Low:** one allocation at body … **Fix:** **None
  recommended.** Making `HttpBody` …
- *Philosophy:* this finding is reported but explicitly **not actionable** under house philosophy — it is architectural and it
  matches ecosystem …

**OX-121 (g4-H10)** — `tick` is a non-optional dependency pulled with the `fmt` feature · **Low** · EMPIRICAL
- *Loc:* `crates/http_extensions/Cargo.toml` · `default = []` is good — the crate defaults to … **Impact Low:** compile time and
  binary size … **Fix:** Check whether `tick/fmt` is used …

### `http_path_template`

*5 findings — 5 Low. Examined:* A small, well-built crate. `ParseError` is a model of how to keep an error type off the hot path (16
  bytes, backtrace boxed …

**OX-122 (g4-P1)** — Zero `#[inline]` across 19 public functions · **Low** · EMPIRICAL
- *Loc:* crate-wide. Representative sites: `crates/http_path_template/src/path_template.rs` (`segments`, `verb`),
  `crates/http_path_template/src/variable.rs` (`name`, `field_path`, `sub`), `crates/http_path_template/src/error.rs` (accessors) ·
  Same category as H1 — small, non-generic … **Impact Low:** to-Medium — lower than H1 … **Fix:** Annotate the trivial accessors. …

**OX-123 (g4-P2)** — `PathTemplate::parse` makes three full passes over the template, one of which exists only to size a `Vec` ·
  **Low** · EMPIRICAL
- *Loc:* `crates/http_path_template/src/path_template.rs:82-111`, with `split_verb` at `:236-277`, `segment_count_hint` at
  `:319-331`, and the real segmentation at `:285-313` · `parse` calls `split_verb` at `:94` (a full byte … **Impact Low:**
  to-Medium. Templates are … **Fix:** Fuse the segment count into …

**OX-124 (g4-P3)** — `Variable::segments()` scans the sub-template eagerly on every call · **Low** · inferred
- *Loc:* `crates/http_path_template/src/variable.rs:89-95` · The iterator constructor computes … **Impact Low:** sub-templates are
  short and … **Fix:** Either compute `remaining` lazily (only …

**OX-125 (g4-P4)** — `Segment` is 56 bytes, sized by its widest variant · **Low** · EMPIRICAL
- *Loc:* `crates/http_path_template/src/path_template.rs` (`Segment` definition) · `Segment` is **56 bytes** (empirically verified)
  … **Impact Low:** templates are small and … **Fix:** **Not recommended.** Boxing the `Affix` …
- *Philosophy:* the obvious "fix" (offset-based segments) conflicts with the house preference for idiomatic Rust over hand-rolled
  layout. …

**OX-126 (g4-P5)** — Affix parsing scans the segment twice · **Low** · inferred
- *Loc:* `crates/http_path_template/src/path_template.rs:377-400` · The affix parser locates the brace delimiters and … **Impact
  Low:** only on the extended grammar … **Fix:** Capture both brace offsets in the …

### `internity`

*9 findings — 1 High, 3 Medium, 5 Low. Examined:* `internity` is a **fill-then-freeze** string/value interner. A `Sym` is a 4-byte
  `NonZeroU32` niche (verified: `Sym` = 4 … *Also cited:* `crates/internity/src/storage.rs:50-59`,
  `crates/internity/src/threaded_lexicon.rs:292-296`

**OX-127 (g6-I1)** — Repeat interns of an already-present string serialise against each other within a shard · **High** · inferred
- *Loc:* `crates/internity/src/shard.rs:67-79` (`Shard::intern`) and `crates/internity/src/shard.rs:89-104` (`Shard::intern_bytes`);
  the trade-off is documented at `crates/internity/src/shard.rs:13-22` and restated at
  `crates/internity/src/threaded_lexicon.rs:42-45`.
- *Issue:* Both functions open with `self.state.upgradable_read()` and then probe the dedup table. On a **hit** — the overwhelmingly
  common case for an interner in steady state — the guard is …
- *Impact:* High — this is the crate's primary steady-state operation, under the crate's primary concurrency story, and …
- *Fix:* Surgical: make the hit path a **double-checked read**. Acquire a plain `read()` guard first and probe the dedup …
- *Also cited:* `shard.rs:13-22`, `crates/internity/benches/internity_compare.rs:520-557`

**OX-128 (g6-I2)** — The serde path cannot reach `intern_bytes`, so every deserialised symbol is UTF-8-validated even on a dedup hit
  · **Medium** · EMPIRICAL
- *Loc:* `crates/internity/src/de/impls.rs:30-45` (`SymVisitor`) and `crates/internity/src/de/impls.rs:47-53` (`DeserializeIn for
  Sym`, which calls `deserializer.deserialize_str(...)`). The bypassed fast path is `Lexicon::intern_bytes`
  (`crates/internity/src/lexicon.rs:65`), implemented at `crates/internity/src/shard.rs:89-104`. · `SymVisitor` implements only
  `visit_str` and `visit_string`, and `DeserializeIn for Sym` … **Impact Medium:** one redundant UTF-8 scan per symbol per …
  **Fix:** Surgical, and confined to one file. …
- *Also cited:* `lexicon.rs:65`

**OX-129 (g6-I3)** — `intern_bytes` — a headline feature — has no benchmark at all · **Medium** · EMPIRICAL
- *Loc:* `crates/internity/benches/` (whole directory); the unmeasured code is `crates/internity/src/shard.rs:89-104` and
  `crates/internity/src/shard_write.rs:125-131` (`get_bytes`). · `internity_compare.rs`, `internity_compare_cg.rs` and
  `internity_mem.rs` exercise only … **Impact Medium:** a performance feature with no performance … **Fix:** Add `intern_bytes`
  variants to the existing `insert` and …

**OX-130 (g6-I4)** — `SymMap` / `SymSet` / `SymBuildHasher` have no benchmark · **Medium** · EMPIRICAL
- *Loc:* `crates/internity/src/symbol_map.rs` (whole file); `crates/internity/benches/` (absent). · `symbol_map.rs` ships a bespoke
  `Hasher` for `Sym` keys — a multiply-mix over the 32-bit … **Impact Medium:** an unmeasured performance-only API surface. …
  **Fix:** Add a `benches/sym_map.rs` + `benches/sym_map_cg.rs` pair …
- *Also cited:* `docs/naming.md:81-90`

**OX-131 (g6-I5)** — `ThreadedLexicon::len` / `is_empty` acquire all 64 shard locks · **Low** · inferred
- *Loc:* `crates/internity/src/threaded_lexicon.rs:342-345` (`ThreadedLexiconInner::len`), reached from `is_empty` and from the
  `Debug` impl. · `len()` sums each shard's length, taking a read … **Impact Low:** neither is on a documented … **Fix:** Surgical:
  give `is_empty` its own …

**OX-132 (g6-I6)** — An empty `ThreadedLexicon` costs 8 KiB and touches 64 cache lines · **Low** · EMPIRICAL
- *Loc:* `crates/internity/src/shard.rs` (`#[repr(align(128))] struct Shard`), `crates/internity/src/threaded_lexicon.rs`
  (`ThreadedLexiconInner` holding `[Shard; SHARD_COUNT]` inline, `SHARD_COUNT = 64`). · The 64 shards are stored **inline** in the
  `Arc` … **Impact Low:** the alignment is *correct* … **Fix:** No change recommended to the default. …
- *Philosophy:* **Conflicting.** `docs/performance.md` deprioritises first-insert and construction costs; this finding is a
  construction cost. …

**OX-133 (g6-I7)** — `ThreadedLexicon::deserialize_in` clones the `Arc` on every call · **Low** · inferred
- *Loc:* `crates/internity/src/de/inherent.rs:105` — `T::deserialize_in(&mut self.clone(), ...)`. · The inherent
  `deserialize_in(&self, ...)` cannot … **Impact Low:** amortised over an entire … **Fix:** Surgical: since `ThreadedLexicon` …
- *Also cited:* `crates/internity/src/lexicon.rs:47`

**OX-134 (g6-I8)** — `Reader::iter` returns a boxed trait-object iterator · **Low** · inferred
- *Loc:* `crates/internity/src/reader.rs` (the `iter` method on the `Reader` trait), implemented in
  `crates/internity/src/flat_reader.rs`, `crates/internity/src/shard_reader.rs`, `crates/internity/src/sharded_reader.rs`; consumed
  at `crates/internity/src/serde_impls.rs:42` (`collect_seq(self.iter()...)`). · Returning `Box<dyn Iterator<Item = ...> + '_>` …
  **Impact Low:** Medium. Low because iteration … **Fix:** The clean fix is an associated type …
- *Philosophy:* **Partially conflicting.** The trait-level fix is architectural and may break object safety, which
  `docs/performance.md`'s …
- *Also cited:* `serde_impls.rs:42`

**OX-135 (g6-I9)** — `freeze()` and the reader-construction paths are unbenchmarked · **Low** · EMPIRICAL
- *Loc:* `crates/internity/src/threaded_lexicon.rs:364-368` (`build_reader`), and the `freeze()` entry points on both lexicons. ·
  The crate offers two distinct freeze paths — a … **Impact Low:** freezing is a … **Fix:** Add a `freeze` group to …

### `internity_macros`

**No performance issues identified.**

`crates/internity_macros/src/lib.rs` is 95 lines and is a pure `proc_macro` facade: it re-exports the derive entry points …

### `internity_macros_impl`

*3 findings — 3 Low. Examined:* The expansion crate behind `internity_macros`: 3437 lines across `attrs.rs` (925), `deserialize.rs`
  (673), `serialize.rs` … *Also cited:* `crates/internity_macros_impl/src/serialize.rs:164-169`, `serialize.rs:96-104`,
  `serialize.rs:131-142`

**OX-136 (g6-IM1)** — Hygiene analysis attempts to parse every string literal in the input as a path · **Low** · EMPIRICAL
- *Loc:* `crates/internity_macros_impl/src/hygiene.rs:18-36` (`collect_ident_strings`), called from
  `crates/internity_macros_impl/src/hygiene.rs:39-43` (`used_identifiers`). · To guarantee generated helper names cannot … **Impact
  Low:** bounded by the size of the … **Fix:** Surgical: skip literals that cannot be …
- *Also cited:* `hygiene.rs:18-36`

**OX-137 (g6-IM2)** — Fresh-identifier generation allocates a `String` per generated name and probes linearly · **Low** · inferred
- *Loc:* `crates/internity_macros_impl/src/hygiene.rs:48-56` (`fresh_ident`); called at
  `crates/internity_macros_impl/src/serialize.rs:108`, `:112`, `:121`, `:122`, `:186-193` and the corresponding sites in
  `deserialize.rs`. · Each call site builds the base name with … **Impact Low:** a few hundred short-string … **Fix:** No action
  recommended. …

**OX-138 (g6-IM3)** — Field types and identifiers are deep-cloned into the plan structures · **Low** · inferred
- *Loc:* `crates/internity_macros_impl/src/serialize.rs:91-124` (`plans.push(NamedPlan { ident: fident, ty: field.ty.clone(), ...
  })`) and `crates/internity_macros_impl/src/serialize.rs:183-201` for the tuple case; the same pattern appears in `deserialize.rs`.
  · `field.ty.clone()` deep-copies a `syn::Type` … **Impact Low:** the ownership makes the … **Fix:** No action recommended. …

### `layered`

*7 findings — 1 High, 3 Medium, 3 Low. Examined:* `layered` provides a `Service`/`Layer` middleware abstraction with three optional
  features: `intercept` (before/after … *Also cited:* `crates/layered/src/dynamic.rs:69-72`, `crates/fetch/src/tokio.rs:77`

**OX-139 (g7-L1)** — `DynamicService` funnels every request through one shared mutex · **High** · inferred
- *Loc:* `crates/layered/src/dynamic.rs:64-92` — the `Mutex::new(Pool::new())` at `:73`, the `pool.lock()` at `:80-87`, the comment
  asserting thread isolation at `:69-72`, and the `Clone` impl at `:123-129` that makes sharing trivially possible. Consumer
  confirmation: `crates/fetch/src/tokio.rs:77` (`Isolation::Shared`).
- *Issue:* `DynamicService::new` creates one `Mutex<Pool>` and captures it in the `exec` closure, which is then stored behind an
  `Arc`. …
- *Impact:* **High** — a single process-wide serialisation point on the request path of the workspace's HTTP client. …
- *Fix:* Surgical, in increasing order of ambition: 1. **Thread-local pools.** Replace `Mutex<Pool>` with a `thread_local!` …
- *Cross-reference:* **the same underlying contention issue** as OX-085 in §fetch (`crates/fetch/src/tokio.rs:77`,
  `handlers/transport.rs:15,19`, `pipeline/builder.rs:124,146`). This is the canonical statement of it.

**OX-140 (g7-L2)** — `DynamicService::execute` clones an `Arc` and pool-allocates per request · **Medium** · inferred
- *Loc:* `crates/layered/src/dynamic.rs:78` (`let cloned = Arc::clone(&service);`), `:80-87` (the `pool.lock().alloc_box(fut)`),
  `:88` (the `plurality::Box::unsize` coercion), `:92` (`Self { exec: Arc::new(exec) }`). · Per request: one `Arc::clone` of the
  inner service (an atomic RMW on a line shared by … **Impact Medium:** **Medium** — the `Arc::clone` is a contended atomic RMW …
  **Fix:** The `Arc::clone` can be avoided by having the pooled future …
- *Cross-reference:* second half of the OX-085 / OX-139 contention issue.

**OX-141 (g7-L3)** — The tower adapter clones the wrapped service on every call · **Medium** · inferred
- *Loc:* `crates/layered/src/tower.rs:88-95` (`Adapter::call`), `:100-112` (`Adapter::execute`, with the service clone), `:130-149`
  (`AdapterLayer::layer`, which nests `Adapter` inside `Adapter`). · `tower::Service::call` requires `&mut self` and a `'static`
  future, so the adapter clones … **Impact Medium:** **Medium** — tower stacks of 3–6 layers are entirely … **Fix:** Flatten the
  adapter: make `AdapterLayer::layer` detect that …

**OX-142 (g7-L4)** — Each tower adapter and each intercept boxes a future per request · **Medium** · inferred
- *Loc:* `crates/layered/src/tower.rs:80-84` (the `Future = Pin<Box<…>>` associated type) and `:90-93` (the `Box::pin` in `call`);
  `crates/layered/src/intercept.rs:200-227` (`Intercept`'s per-call path, with the `Arc::clone` at `:218`). · `tower::Service`
  mandates a named `Future` associated type, so `Adapter` declares … **Impact Medium:** **Medium** — allocation per layer per
  request, on the … **Fix:** The `Box` at the tower boundary is forced by …

**OX-143 (g7-L5)** — `InterceptInner` holds four separate `Arc<[T]>` slices · **Low** · inferred
- *Loc:* `crates/layered/src/intercept.rs:405-418` (the `InterceptInner` struct with four `Arc<[…]>` fields, and `before_execute` at
  `:417`). · The intercept state is an `Arc<InterceptInner>` … **Impact Low:** **Low** — the allocations are … **Fix:** Store the
  four lists in one allocation …

**OX-144 (g7-L6)** — Native `Service` composition is allocation-free — recorded as a positive · **Low** · inferred
- *Loc:* `crates/layered/src/service.rs:11-35` (the `Service` trait returning `impl Future + Send`),
  `crates/layered/src/layer/tuples.rs:1-90` (tuple-based static composition), `crates/layered/src/execute.rs`. · None. The crate's
  own composition model uses … **Impact Low:** **Low** (positive) — recorded … **Fix:** None. Consider documenting the cost …
- *Also cited:* `service.rs:37-57`

**OX-145 (g7-L7)** — Modest `#[inline]` coverage — 2 of 7 public functions · **Low** · EMPIRICAL
- *Loc:* whole crate; `crates/layered/src/intercept.rs:417` (`before_execute`, correctly annotated) is one of the two. · 2
  `#[inline]` / 7 `pub fn`. … **Impact Low:** **Low** — little to gain. … **Fix:** Nothing required. Possibly …

### `multitude`

*15 findings — 1 High, 7 Medium, 7 Low. Examined:* `multitude` is the workspace's arena allocator: a bump allocator over 64
  KiB-aligned chunks, with thin (single-pointer … *Also cited:* `chunk_mutator.rs:152-186`, `arena/mod.rs:41`

**OX-146 (g1-F1)** — `de::Value` and `de::Number` are inflated to 32 bytes / align 16 by the `i128`/`u128` variants · **High** ·
  EMPIRICAL+INFERRED
- *Loc:* `crates/multitude/src/de/value/number.rs:11-37`, `crates/multitude/src/de/value/dynamic_value.rs:18-47`,
  `crates/multitude/src/de/value/entry.rs:20-45`
- *Issue:* `Number` carries `I128(i128)` and `U128(u128)`. Those two variants force `align_of::<Number>() == 16` and therefore
  `size_of::<Number>() == 32`. …
- *Impact:* High — this is the crate's headline "dynamic value into arena" workload (`multitude_serde/dynamic` …
- *Fix:* Two options, in increasing order of disruption. (a) Surgical and layout-optimal: change …
- *Philosophy:* `docs/performance.md` says optimisations must be motivated by a real user-facing scenario and prefers surgical
  changes over …
- *Also cited:* `benches/multitude_serde_cg.rs:112`

**OX-147 (g1-F2)** — `ChunkProvider::reserve_bytes` runs a CAS retry loop on every chunk acquisition even when the byte budget is
  unbounded · **Medium** · inferred
- *Loc:* `crates/multitude/src/internal/chunk_provider.rs:469-489`; default budget set at `crates/multitude/src/arena/mod.rs:298` ·
  `reserve_bytes` is called for every chunk taken from the backing allocator. … **Impact Medium:** this is on the chunk-refill path,
  not the … **Fix:** Surgical: add an unbounded fast path. …
- *Also cited:* `benches/criterion_alloc.rs:23-27`

**OX-148 (g1-F3)** — Atomic orderings stronger than the data flow requires on pure counters · **Low** · inferred
- *Loc:* `crates/multitude/src/internal/chunk_provider.rs:491-499` (`release_bytes`, `release_reservation`), `:615-619`
  (`record_allocation`) · `release_bytes` and `release_reservation` do … **Impact Low:** Low on x86-64, Medium on AArch64 — …
  **Fix:** Downgrade the pure counters to …

**OX-149 (g1-F4)** — Missing cold-path split on the `Arc`/`Rc`/`Box` and `Allocator::allocate` allocation entry points, unlike the
  `Alloc` path · **Medium** · EMPIRICAL
- *Loc:* `crates/multitude/src/arena/alloc_value.rs:899-920` (`alloc_smart_prefixed_with_raw`), `:933-960`
  (`impl_alloc_smart_with`), `crates/multitude/src/allocator_impl.rs:31-74` (`<&Arena<A> as Allocator>::allocate`); contrast with
  the correct shape at `crates/multitude/src/arena/alloc_value.rs:809-818` (`alloc_value_with_raw` delegating to a `#[cold]
  #[inline(never)]` `alloc_value_refill_with`) · The `Alloc` scalar path gets it right: try the bump once, and on failure tail-call
  into a … **Impact Medium:** code size and icache rather than instruction … **Fix:** Mirror the `alloc_value_with_raw` shape:
  extract the …
- *Philosophy:* `#[inline(always)]` on `alloc_smart_prefixed_with_raw` is exactly the "advanced tuning knob" `docs/performance.md`
  rule 3 says not …
- *Also cited:* `allocator_impl.rs:31-74`, `benches/criterion_arc_array_cg.rs:44`, `strings/string.rs:505-509`

**OX-150 (g1-F5)** — `Vec::retain_mut` uses bounds-checked indexing and a 3-move `swap` where a 1-move copy suffices · **Medium** ·
  inferred
- *Loc:* `crates/multitude/src/vec/mutate.rs:382-396` · ```rust for read in 0..len { let keep = f(&mut slice[read]); if keep { if
  write != read { … **Impact Medium:** `retain`/`retain_mut` is a natural fit for the … **Fix:** Adopt the `std` shape: iterate with
  raw pointers, use …

**OX-151 (g1-F6)** — `Vec::try_resize` / `try_resize_with` push element-at-a-time through a fallible, panicking `push_within_cap` ·
  **Medium** · inferred
- *Loc:* `crates/multitude/src/vec/mutate.rs:637-648` (`try_resize`), `:695-712` (`try_resize_with`) · Both functions reserve the
  full capacity up front (correct), then fill with ```rust for _ … **Impact Medium:** `resize`/`resize_with` are the canonical way
  to … **Fix:** Keep the `ResizeGuard` (it is the defensive panic-safety …

**OX-152 (g1-F7)** — UTF-16 transcoding from `&str` walks the input twice · **Medium** · inferred
- *Loc:* `crates/multitude/src/arena/alloc_utf16.rs:396-402` (`impl_alloc_utf16_prefixed_from_str`), `:454-460`
  (`alloc_utf16_prefixed_from_str_raw`), and the one-`u16`-at-a-time inner loop at `:493-505` (`transcode_utf16_into`) · Every
  `alloc_utf16_str_{arc,rc,box}_from_str` first computes the exact output length with … **Impact Medium:** halves the input scan for
  what is a … **Fix:** Reserve `s.len()` units, transcode single-pass while …
- *Philosophy:* This is closer to an architectural change than a surgical one (it reorders prefix/payload initialisation). …
- *Also cited:* `allocator_impl.rs:143-149`

**OX-153 (g1-F8)** — `ChunkProvider` interleaves the owner-read configuration with the cross-thread-written cache head and stats
  counters on the same cache lines · **Medium** · EMPIRICAL+INFERRED
- *Loc:* `crates/multitude/src/internal/chunk_provider.rs:126-181` · Field order is `allocator: Arc<A>` (offset 0), `config:
  ChunkProviderConfig` (8) … **Impact Medium:** Medium in the multi-threaded escape scenario the design … **Fix:** Surgical and
  non-behavioural: reorder the struct so the …

**OX-154 (g1-F9)** — `Chunk`'s cross-thread refcount shares a cache line with the owner-read capacity · **Low** · EMPIRICAL+INFERRED
- *Loc:* `crates/multitude/src/internal/chunk.rs:42-61` · `#[repr(C)] struct Chunk<A> { allocator: Arc<A> … **Impact Low:** real but
  rare, and the fix … **Fix:** Recommend **no change**. …

**OX-155 (g1-F10)** — `Chunk::teardown_and_release` performs a `Weak::upgrade` per chunk release · **Low** · inferred
- *Loc:* `crates/multitude/src/internal/chunk.rs:412-421` · Releasing a chunk calls `self.provider.upgrade()` … **Impact Low:**
  to-Medium — chunk cadence, and … **Fix:** If measurement justifies it, hold the …
- *Philosophy:* `docs/performance.md` says to deprioritise teardown optimisations. Flagged: this finding is deliberately low
  priority and is …

**OX-156 (g1-F11)** — `ArenaBuf`'s `freeze_prefix: bool` costs 8 bytes of padding in every arena `Vec` and `String` · **Low** ·
  EMPIRICAL+INFERRED
- *Loc:* `crates/multitude/src/internal/arena_buf.rs:29-40` · `ArenaBuf<T, A>` is `{ ptr: NonNull<T>, len … **Impact Low:**
  to-Medium — `Vec` is usually a … **Fix:** Encode the flag in a spare low bit of …
- *Philosophy:* Bit-packing a `bool` into a pointer or capacity is a deviation from idiomatic Rust that `docs/performance.md` asks
  be justified. …
- *Also cited:* `benches/criterion_alloc.rs:441`

**OX-157 (g1-F12)** — `cow.rs` has no `#[inline]` anywhere and `to_mut` recurses instead of matching · **Low** · inferred
- *Loc:* `crates/multitude/src/cow.rs:80-106` (`to_mut`, `try_to_mut`); whole file for the `#[inline]` observation (20 public
  functions, 0 `#[inline]` attributes — the only such file in the crate's public surface) · Two separate observations. … **Impact
  Low:** the recursion is very likely … **Fix:** For (a), restructure to the `std` …
- *Philosophy:* part (b) explicitly does *not* meet the bar in `docs/performance.md` rule 2 (no measurement showing the default
  inlining decision …

**OX-158 (g1-F13)** — `<&Arena<A> as Allocator>` has no `#[inline]` while its own forwarding shims do · **Low** · EMPIRICAL
- *Loc:* `crates/multitude/src/allocator_impl.rs:30-168` (the `allocator_api2` 0.4 impl: `allocate` `:31`, `deallocate` `:76`,
  `grow` `:89`, `grow_zeroed` `:118`, `shrink` `:143` — none annotated) versus `:170-230` (the `allocator_api2_02` bridge, where all
  five forwarding one-liners carry `#[inline]`) · The file contains exactly five `#[inline]` … **Impact Low:** to-Medium, and
  genuinely … **Fix:** Consider `#[inline]` on `deallocate` …
- *Philosophy:* These functions are generic (`impl<A: Allocator + Clone>`), so `docs/performance.md` rule 1 does *not* apply and
  rule 2 requires …
- *Also cited:* `benches/criterion_arena_vs_allocator.rs:81`

**OX-159 (g1-F14)** — `stats` is a default-off feature whose cost is nevertheless in every published benchmark number · **Medium** ·
  EMPIRICAL+INFERRED
- *Loc:* `crates/multitude/Cargo.toml` (feature `stats`); counters at `crates/multitude/src/internal/chunk_provider.rs:143-181`,
  `:600-620`; arena-side counters in `crates/multitude/src/arena/mod.rs`; bench-side conditional at
  `crates/multitude/benches/multitude_record_batch.rs:15,25,164-176` · With `stats` on, `ChunkProvider` grows from 56 to 152 bytes
  (layout probe), the `Chunk` … **Impact Medium:** this is a measurement-validity problem rather … **Fix:** Benchmark `multitude`
  with its default feature set (or add an …
- *Also cited:* `chunk_provider.rs:615-619`

**OX-160 (g1-F15)** — Generic `impl AsRef<...>` parameters on high-arity public APIs multiply monomorphisations · **Low** · inferred
- *Loc:* `crates/multitude/src/strings/string.rs:493` (`push_str(&mut self, s: impl AsRef<str>)`),
  `crates/multitude/src/arena/alloc_utf16.rs:40,67,88,112,145,169,193,220,241,265,283,307` (twelve `impl
  AsRef<widestring::Utf16Str>` / `impl AsRef<str>` entry points) · `impl AsRef<T>` in argument position is ergonomic … **Impact
  Low:** compile time and binary … **Fix:** No change recommended for the existing …

### `multitude_macros`

*1 finding — 1 Low. Examined:* `multitude_macros` is a 21-line `proc-macro` shim (`crates/multitude_macros/src/lib.rs`). …

**OX-161 (g1-F16)** — `syn` is declared with the full heavy feature set, including `extra-traits`, for a 21-line shim that uses
  `Path` and `parse_quote` only · **Low** · inferred
- *Loc:* `crates/multitude_macros/Cargo.toml:27` (`syn = { workspace = true, features = ["full", "derive", "printing", "parsing",
  "extra-traits", "proc-macro", "clone-impls"] }`); the crate's entire use of `syn` is `crates/multitude_macros/src/lib.rs:13,17`
  (`use syn::{Path, parse_quote};` and one `parse_quote!(::multitude::de)`) · The workspace declares `syn = { version = … **Impact
  Low:** no runtime cost; downstream … **Fix:** Reduce to what the file uses: `features …
- *Also cited:* `Cargo.toml:207`

### `multitude_macros_impl`

*3 findings — 3 Low. Examined:* `multitude_macros_impl` (1908 lines in `src/lib.rs`, 642 in `src/attrs.rs`) contains the real
  `DeserializeIn` derive …

**OX-162 (g1-F17)** — `syn`'s `extra-traits` feature is enabled but nothing in the crate needs it · **Low** · inferred
- *Loc:* `crates/multitude_macros_impl/Cargo.toml:36` · `extra-traits` exists to provide … **Impact Low:** to-Medium on downstream …
  **Fix:** Drop `extra-traits` (and probably …
- *Also cited:* `attrs.rs:7,13,25,105`, `lib.rs:41,1228`, `lib.rs:273`

**OX-163 (g1-F18)** — Type-identity deduplication for where-clause synthesis renders each type to a `String` · **Low** · inferred
- *Loc:* `crates/multitude_macros_impl/src/lib.rs:267-299` (the `seen: HashSet<(String, &str)>` loop; keys built at `:273` and
  `:281`) · For every field, the code does `let key = … **Impact Low:** bounded by field count, and … **Fix:** Compute the key once
  per field and …

**OX-164 (g1-F19)** — Generated code volume per derived type is substantial, and there is no `#[inline]` guidance on the generated
  visitor methods · **Low** · inferred
- *Loc:* generated by `crates/multitude_macros_impl/src/lib.rs:537-680` (`field_enum`), `:680-960` (`named_visitor`), `:1265-1400`
  (the enum equivalents) · Each derived type emits: a `Field0..FieldN + … **Impact Low:** and genuinely uncertain in … **Fix:** No
  change recommended without …
- *Philosophy:* Adding `#[inline]` here would violate `docs/performance.md` rule 2 (no measurement showing the default is wrong). …
- *Also cited:* `benches/multitude_serde_cg.rs:108`

### `ohno`

*9 findings — 5 Medium, 4 Low. Examined:* `ohno` is the workspace's error type. Its central design decision is excellent and should
  be protected: `OhnoCore` is a …

**OX-165 (g8-F1)** — Every error construction allocates a `Box`, then immediately re-allocates it as an `Arc` and frees the `Box` ·
  **Medium** · EMPIRICAL
- *Loc:* `crates/ohno/src/core.rs:185`, `crates/ohno/src/core.rs:187`; the receiving field is `Source` at
  `crates/ohno/src/source.rs`, and the same double conversion is reachable via `OhnoCore::new_from` at `crates/ohno/src/core.rs:65`.
  · The `From<E> for OhnoCore` impl writes `error.into().into()`. … **Impact Medium:** one avoidable allocation, one deallocation
  and … **Fix:** Have the conversion produce the `Arc` directly rather than …

**OX-166 (g8-F2)** — `OhnoCore: Clone` deep-copies the entire error including the enrichment `Vec` · **Medium** · EMPIRICAL
- *Loc:* `crates/ohno/src/core.rs:42-45` (the `#[derive(Clone)]` on `OhnoCore`), over `Inner` at `crates/ohno/src/core.rs:14-19`. ·
  `OhnoCore` is `Box<Inner>`, so cloning it clones `Inner`, which clones the `Source` … **Impact Medium:** Medium. Cloning an error
  is not rare: it happens … **Fix:** The clean fix is `Box<Inner>` → `Arc<Inner>`, making `Clone` …
- *Philosophy:* CONFLICTING. `docs/performance.md` prefers surgical interventions over architectural rewrites, and this is
  unambiguously an …

**OX-167 (g8-F3)** — Derived `Display` allocates a `String` on every format · **Medium** · EMPIRICAL
- *Loc:* `crates/ohno_macros/src/derive_error/display.rs:108-114` (`generate_display_expression`), consumed by
  `crates/ohno_macros/src/derive_error/mod.rs:112-139` (`generate_display_impl`). Listed under `ohno` as well as `ohno_macros`
  because the cost lands in every `ohno`-derived error type in the workspace. · For an error variant carrying `#[display("...")]`,
  the macro emits … **Impact Medium:** Medium. `Display` on an error is called by every logging … **Fix:** Emit `write!(f, "...")`
  directly against the formatter …
- *Also cited:* `display.rs:108-114`

**OX-168 (g8-F4)** — `ErrorExt::message()` allocates twice per call · **Medium** · EMPIRICAL+INFERRED
- *Loc:* `crates/ohno_macros/src/derive_error/mod.rs:236-248` (`generate_error_ext_impl`), which routes through `format_message` at
  `crates/ohno/src/core.rs:118-153`. · `message()` builds a `MessageFormatter` and calls `.to_string()` on it. … **Impact Medium:**
  `message()` is the idiomatic way to get an … **Fix:** Fixing F3 removes the inner allocation. …

**OX-169 (g8-F5)** — No build-time opt-out for backtrace capture; enabling backtraces costs 12–26x the allocated bytes per error ·
  **Medium** · EMPIRICAL
- *Loc:* `crates/ohno/src/backtrace.rs:37-40`; feature list in `crates/ohno/Cargo.toml` (the only features are `app-err` and
  `test-util`). · Error construction always calls `StdBacktrace::capture()`. … **Impact Medium:** Medium. Not High, because the
  default (env unset) is … **Fix:** Add a `backtrace` Cargo feature, default-on to preserve …

**OX-170 (g8-F6)** — `ErrorLabel::from_error_chain` allocates a `String` even for a single-label chain · **Low** · inferred
- *Loc:* `crates/ohno/src/error_label.rs:152-161` (`from_error_chain`), calling `from_parts` at
  `crates/ohno/src/error_label.rs:118-130`. · When the error has a source, `from_error_chain` … **Impact Low:** Low. Error labels
  are typically … **Fix:** Short-circuit in `from_error_chain`: if …

**OX-171 (g8-F7)** — `app_err!` with a literal message allocates through `format!` for no reason · **Low** · inferred
- *Loc:* `crates/ohno/src/app/macros.rs:45-47`. · `app_err!("some message")` expands to … **Impact Low:** error path, one small …
  **Fix:** Give the macro two arms: a …

**OX-172 (g8-F8)** — Eager `to_string()` in the non-lazy `IntoAppErr` enrichment methods · **Low** · inferred
- *Loc:* `crates/ohno/src/app/into_app_err.rs:41`, `:62`, `:75`, `:84`, `:95`, `:113`. · The eager variants call `msg.to_string()`
  to … **Impact Low:** Low. The `_with` closure-taking … **Fix:** No code change. Document in the trait's …

**OX-173 (g8-F9)** — Zero `#[inline]` annotations across 25 public functions · **Low** · inferred
- *Loc:* crate-wide; `crates/ohno/src/` contains no `#[inline]` outside `test_util.rs` (which has 3, in code that is behind the
  `test-util` feature). The affected accessors include `OhnoCore::backtrace`, `OhnoCore::source`, `OhnoCore::enrichments`,
  `ErrorLabel::as_str`, and the `AppError` accessors. · These are small, non-generic, exported functions. … **Impact Low:**
  individually each is a … **Fix:** Be judicious, per the philosophy …
- *Philosophy:* Partially conflicting. `docs/performance.md` is deliberately restrained about `#[inline]`, and rule 1's
  applicability is in …

### `ohno_macros`

*4 findings — 1 Medium, 2 Low, 1 —. Examined:* For a proc-macro crate, "performance" has two distinct meanings: the compile time it
  imposes on every downstream crate, and …

**OX-174 (g8-F10)** — `syn` is enabled with `extra-traits` in `[dependencies]`, inflating downstream compile time · **Medium** ·
  EMPIRICAL+INFERRED
- *Loc:* `crates/ohno_macros/Cargo.toml`, the `syn` entry in `[dependencies]`, which enables the `extra-traits` feature. Compare the
  workspace declaration at root `Cargo.toml:207`: `syn = { version = "3.0.2", default-features = false }`. · `extra-traits` makes
  `syn` derive `Debug`, `Eq`, `PartialEq` and `Hash` for its entire … **Impact Medium:** Medium on compile time, zero at runtime. …
  **Fix:** Move `extra-traits` to `[dev-dependencies]` (a separate `syn` …

**OX-175 (g8-F11)** — Generated `Display` materialises a `String` instead of writing to the formatter · **—** · inferred
- *Loc:* `crates/ohno_macros/src/derive_error/display.rs:108-114`. · see **F3** above. Recorded here so that a reader …

**OX-176 (g8-F12)** — `#[enrich_err]` on an `async fn` nests an async closure inside the function's future · **Low** · EMPIRICAL
- *Loc:* `crates/ohno_macros/src/enrich_err/mod.rs:47-60`, in particular the emitted shape at `:52-60`. · The macro rewrites the
  function body to … **Impact Low:** Low to Medium, and genuinely … **Fix:** Emit the body inline and apply the …

**OX-177 (g8-F13)** — `#[derive(Error)]` emits eight impl blocks per error type · **Low** · inferred
- *Loc:* `crates/ohno_macros/src/derive_error/mod.rs` — the top-level expansion assembles `Display`, `Debug`, `std::error::Error`,
  `ErrorExt`, the `From` impls, the constructors, and the `Enrichable` plumbing. · Each derived error type expands to a substantial
  … **Impact Low:** Low. This is what a derive macro … **Fix:** No action recommended on the codegen …

### `plurality`

*7 findings — 3 Medium, 4 Low. Examined:* `plurality` is not a collections crate — it is a **pooling memory allocator**
  (`crates/plurality/Cargo.toml` description: "A … *Also cited:* `crates/plurality/src/slot.rs:60-80`,
  `crates/plurality/src/pool.rs:851-919`

**OX-178 (g6-P1)** — Immutable pool geometry shares cache lines with the two hottest atomics · **Medium** · inferred
- *Loc:* `crates/plurality/src/pool.rs:37-44` (`PoolCore`) and `crates/plurality/src/pool.rs:82-110` (`PoolInner`), used from
  `crates/plurality/src/pool.rs:807-825` (`alloc_slot`) and `crates/plurality/src/pool.rs:967-985` (`push_free`). · `PoolInner<T,
  A>` is `#[repr(C)]` with `PoolCore` first — the source comment at … **Impact Medium:** only in the producer/consumer pattern the
  crate … **Fix:** Surgical. Either (a) reorder `PoolInner` so the read-only …
- *Also cited:* `pool.rs:80-81`, `crates/internity/benches/internity_compare.rs:520-557`

**OX-179 (g6-P2)** — `Arc`/`Rc` clone and drop pay a metadata load before they can find the refcount, for unsized `T` · **Medium** ·
  EMPIRICAL+INFERRED
- *Loc:* `crates/plurality/src/pool.rs:1125-1140` (`refcount_ptr`); callers in `crates/plurality/src/sync.rs` (`Arc::clone`,
  `Arc::drop`) and `crates/plurality/src/rc.rs`. · Because `SlotCell` puts the value at field 0 and the refcount *after* it,
  `refcount_ptr` … **Impact Medium:** Low — zero cost for sized `T` (the common case … **Fix:** There is no surgical fix. Putting
  the refcount *before* the …
- *Philosophy:* **Conflicting.** Any real fix here is architectural, and `docs/performance.md` prefers surgical interventions. …

**OX-180 (g6-P3)** — Free-list pop uses a stronger success ordering than the protocol requires · **Low** · inferred
- *Loc:* `crates/plurality/src/pool.rs:807-825` (`alloc_slot`). · The pop is a `load(Acquire)` followed by … **Impact Low:**
  architecture-specific, one … **Fix:** Change the success ordering to …
- *Also cited:* `crates/plurality/src/pool.rs:967-985`

**OX-181 (g6-P4)** — Cheap public accessors are not `#[inline]` · **Low** · EMPIRICAL
- *Loc:* `crates/plurality/src/pool.rs:180-262` — `chunk_size`, `max_chunks`, `max_capacity`, `chunks_allocated`, `len`, `capacity`,
  `available`, `is_empty`, `stats`. · These are one-or-two-instruction field reads that … **Impact Low:** and arguably **not a
  defect … **Fix:** No action recommended. …

**OX-182 (g6-P5)** — The crate's only concurrency has no benchmark · **Medium** · EMPIRICAL
- *Loc:* `crates/plurality/benches/` (whole directory); the unmeasured code is `crates/plurality/src/pool.rs:967-985` (`push_free`)
  and the cross-thread `Arc::drop` path in `crates/plurality/src/sync.rs`. · `plurality`'s central design claim is that the pool is
  `Send + !Sync` — allocation … **Impact Medium:** this is the highest-value gap in the crate. … **Fix:** Add a multithreaded
  Criterion benchmark using …
- *Also cited:* `crates/internity/benches/internity_compare.rs:520-557`

**OX-183 (g6-P6)** — Benchmark layout does not satisfy the mandatory Callgrind/Criterion pairing rule · **Low** · EMPIRICAL
- *Loc:* `crates/plurality/benches/gungraun/main.rs`, `crates/plurality/benches/gungraun/linux.rs`,
  `crates/plurality/benches/pool_comparison/main.rs`, `crates/plurality/benches/pool_comparison/linux.rs`,
  `crates/plurality/benches/criterion/main.rs`, `crates/plurality/benches/graph_churn.rs`; rule at `docs/naming.md:81-90`. · Three
  separate deviations. 1. … **Impact Low:** Low for runtime performance … **Fix:** Either bring the layout into line with …

**OX-184 (g6-P7)** — Correction: the `stats` feature does **not** contaminate `plurality`'s steady-state benchmark numbers · **Low**
  · EMPIRICAL
- *Loc:* `crates/plurality/src/pool.rs:907` (the only accounting site), inside `grow()` at `crates/plurality/src/pool.rs:851-919`;
  `crates/plurality/src/pool.rs:28, 100, 217-218`; `crates/plurality/src/builder.rs:124`; `crates/plurality/src/pool_stats.rs`. ·
  The cross-group round-1 note warns that because … **Impact Low:** but recording it matters … **Fix:** No code change. Note in …
- *Also cited:* `pool.rs:907`, `crates/plurality/src/builder.rs:18`, `pool.rs:100`

### `recoverable`

*1 finding — 1 Low. Examined:* The cleanest crate in the group. `RecoveryInfo` is a small POD-like struct, all eight public
  functions are `const fn` …

**OX-185 (g8-F33)** — All eight public `const fn` accessors lack `#[inline]` · **Low** · inferred
- *Loc:* `crates/recoverable/src/lib.rs:186`, `:220`, `:248`, `:279`, `:317`, `:341`, `:386`, `:485`. The crate contains **zero**
  `#[inline]` annotations. · These are the archetypal rule-1 case: tiny … **Impact Low:** Low in absolute terms — a handful …
  **Fix:** Annotate all eight. They are `const fn` …
- *Philosophy:* Same benchmark-blindness caveat as F9, F21 and F24 — the repository's fat-LTO bench profile cannot demonstrate the
  effect. …

### `rest_over_grpc`

*12 findings — 9 Medium, 3 Low. Examined:* `rest_over_grpc` is the largest crate in this group (about 13,134 lines) and the one
  whose hot path is most clearly …

**OX-186 (g5-F24)** — `decode_flat` builds a deduplication `HashSet` against a body that is always empty for GET · **Medium** ·
  inferred
- *Loc:* `crates/rest_over_grpc/src/transcode/overlay.rs:94-114`; the wasted work is lines 105-106, with the relevant `body_entries`
  initialisation at line 102. · `decode_flat` constructs a `HashSet<Cow<str>>` containing every query key (line 105) and … **Impact
  Medium:** one allocation plus k hash computations per … **Fix:** Guard both lines on `!body_entries.is_empty()`. …

**OX-187 (g5-F25)** — `encode_response` serialises into a zero-capacity `Vec` · **Medium** · inferred
- *Loc:* `crates/rest_over_grpc/src/transcode/mod.rs:140-144`; contrast with `crates/rest_over_grpc/src/stream.rs:~208-215`
  (`serialize_framed_item`). · `encode_response` starts from `Vec::new()` — capacity zero — and hands it to serde_json. … **Impact
  Medium:** per response, proportional to response size … **Fix:** Start from `Vec::with_capacity(n)` with a sensible default …

**OX-188 (g5-F26)** — The buffer-reusing encode entry point exists but is `pub(crate)`, so generated code cannot use it · **Medium**
  · inferred
- *Loc:* `crates/rest_over_grpc/src/transcode/mod.rs:146-160` (`encode_response_into(&mut Vec<u8>)`); the generated call site is
  produced by `crates/rest_over_grpc/src/build/service_definition.rs:941`. · The crate already has the right primitive —
  `encode_response_into`, which serialises into … **Impact Medium:** this is what turns F25 from "one suboptimal … **Fix:** Make
  `encode_response_into` public (or expose it via a …
- *Also cited:* `service_definition.rs:941`

**OX-189 (g5-F27)** — Three nested `Box::pin` layers on the streaming response path · **Medium** · EMPIRICAL
- *Loc:* `crates/rest_over_grpc/src/transcode_response.rs:62` (`ResponseStream<T> = Pin<Box<dyn Stream>>`) and `:116` (`frames:
  Box::pin(frames)`); `crates/rest_over_grpc/src/stream.rs:~290-320` (`FrameState`, `stream: Box::pin(items)`). · A streaming
  response passes through three independent boxing layers: the item stream is … **Impact Medium:** per frame on every streaming
  response. … **Fix:** Honest assessment: this is **not purely surgical**. …

**OX-190 (g5-F28)** — Per-request query helpers called by generated code are not `#[inline]` · **Medium** · inferred
- *Loc:* `crates/rest_over_grpc/src/path.rs:28-33` (`split_query`), `:49-77` (`parse_query`), `:145-154` (`QueryPairs::{as_slice,
  iter, len, is_empty}`). · These functions are called **per request** by code generated into the user's own crate … **Impact
  Medium:** small per call, but these are on the … **Fix:** Add `#[inline]` to the accessors and to `split_query`. …

**OX-191 (g5-F29)** — Body field decoding allocates a String per field plus a HashMap for duplicate detection · **Medium** ·
  inferred
- *Loc:* `crates/rest_over_grpc/src/transcode/overlay.rs:169-176` (`BodyTop::visit_map`). · For each field of the request body,
  `visit_map` does three allocating things: it … **Impact Medium:** proportional to field count on every request … **Fix:** Three
  independent steps, in increasing order of effort. …

**OX-192 (g5-F30)** — Query string scanned twice for two different character classes · **Low** · EMPIRICAL
- *Loc:* `crates/rest_over_grpc/src/transcode/overlay.rs:25-55`; the double scan is at line 50. · `try_decode_overlay` calls
  `all_flat(query)` … **Impact Low:** query strings are short and … **Fix:** Fuse into a single pass returning both …

**OX-193 (g5-F31)** — `Status` always heap-allocates its message, including for `&'static str` · **Medium** · EMPIRICAL+INFERRED
- *Loc:* `crates/rest_over_grpc/src/status.rs:42-46` (the struct) and `:61-67` (`Status::new`). · `Status` is `{ code, message:
  String, details: Vec<Value> }` and `Status::new(impl … **Impact Medium:** one avoidable allocation per error response … **Fix:**
  Change the field to `Cow<'static, str>` and take `impl …

**OX-194 (g5-F32)** — `TranscodeError::from_source` formats, boxes and captures a backtrace eagerly · **Low** · EMPIRICAL+INFERRED
- *Loc:* `crates/rest_over_grpc/src/transcode/error.rs:80-87`. · `from_source` performs `source.to_string()` … **Impact Low:** this
  is the error path, and … **Fix:** Consider dropping the eager …
- *Philosophy:* **Partially conflicting.** Trimming error-path work runs against "preserve defensive runtime checks" and the general
  …

**OX-195 (g5-F33)** — Static error bodies are copied with `to_vec()` on every invocation · **Low** · inferred
- *Loc:* `crates/rest_over_grpc/src/serving.rs:224-238` (`body_read_failed`, `body_too_large`). · Both helpers call `.to_vec()` on a
  `&'static … **Impact Low:** cold path (these fire only … **Fix:** `Bytes::from_static(...)` is …

**OX-196 (g5-F34)** — Zero `#[inline]` across 118 public functions · **Medium** · inferred
- *Loc:* crate-wide; census of `crates/rest_over_grpc/src`. · The largest crate in the group, with by far the most public surface
  called from generated … **Impact Medium:** Medium. **Fix:** Prioritise the functions that generated code calls per …

**OX-197 (g5-F35)** — Zero benchmarks in a 13,134-line crate · **Medium** · inferred
- *Loc:* `crates/rest_over_grpc` — no `benches/` directory. · The crate with the group's most complex per-request work has no
  benchmark of its own. … **Impact Medium:** not a runtime cost, but it is why F24 through … **Fix:** Add Criterion benchmarks.
  Whether they live here or in …

### `rest_over_grpc_examples`

*1 finding — 1 Low. Examined:* An examples crate. It contains no library code on any hot path, so there are no runtime findings. …

**OX-198 (g5-F38)** — Examples exercise realistic performance-relevant paths but host no benchmarks · **Low** · inferred
- *Loc:* `crates/rest_over_grpc_examples/examples/serving/streaming_response.rs`,
  `crates/rest_over_grpc_examples/examples/transcoding/basic_transcode.rs`,
  `crates/rest_over_grpc_examples/examples/handling/client_streaming_upload.rs`; no `benches/` directory in the crate. · The
  examples under … **Impact Low:** no runtime cost; this is a … **Fix:** Either add `benches/` here reusing the …

### `rest_over_grpc_tests`

*2 findings — 2 Medium. Examined:* A test-support crate that hosts generated services and the only benchmarks that touch
  `rest_over_grpc`. …

**OX-199 (g5-F36)** — Callgrind benchmarks with no paired Criterion files — violates `docs/naming.md:81-90` · **Medium** · inferred
- *Loc:* `crates/rest_over_grpc_tests/benches/rog_router_cg.rs`, `crates/rest_over_grpc_tests/benches/rog_transcode_cg.rs`; the
  `[[bench]]` registrations at the bottom of `crates/rest_over_grpc_tests/Cargo.toml` list only these two. · `docs/naming.md:81-90`
  states the pairing rule: a Criterion benchmark may exist without a … **Impact Medium:** a process/coverage defect rather than a
  runtime … **Fix:** Add `benches/rog_router.rs` and `benches/rog_transcode.rs` …

**OX-200 (g5-F37)** — The existing benchmarks are structurally blind to the transcode path's dominant cost · **Medium** · EMPIRICAL
- *Loc:* `crates/rest_over_grpc_tests/benches/rog_transcode_cg.rs` (in relation to `crates/rest_over_grpc/src/transcode/`). · Every
  significant finding against `rest_over_grpc` is an allocation finding (F24, F25 … **Impact Medium:** it means the safety net that
  exists gives false … **Fix:** The Criterion files required by F36 should use …

### `routerama`

*11 findings — 4 Medium, 7 Low. Examined:* `routerama` is the strongest crate in this scope by a wide margin: 48 of 60 public
  functions carry `#[inline]`, every …

**OX-201 (g4-R1)** — `resolve_scanned_checked` re-scans the whole path and heap-allocates on the >16-segment spill path · **Medium**
  · inferred
- *Loc:* `crates/routerama/src/raw_resolver.rs:158-202` · The function declares two fixed `[0usize; 16]` scratch arrays
  (`:158-159`), scans the … **Impact Medium:** bounded to deep paths, which are uncommon in … **Fix:** Surgical. Have the scanner
  report the required capacity …
- *Also cited:* `codegen_helpers/scanned_path.rs:149-173`

**OX-202 (g4-R2)** — 256 bytes of stack scratch are zero-initialised on every resolve regardless of table depth · **Low** ·
  EMPIRICAL+INFERRED
- *Loc:* `crates/routerama/src/raw_resolver.rs:158-161` · `let mut starts = [0_usize; 16]; let mut ends = … **Impact Low:** 256
  bytes of stack touch per … **Fix:** Use `[MaybeUninit<usize>; 16]` (the …

**OX-203 (g4-R3)** — Literal-edge lookup is a linear scan over an 88-byte stride to compare a 16-byte key · **Medium** ·
  EMPIRICAL+INFERRED
- *Loc:* `crates/routerama/src/walk.rs:49-52`; layout at `crates/routerama/src/rt_node.rs:17-24` · `descend_iterative` finds the
  matching literal child with `node.literals.iter().find(…)`. … **Impact Medium:** this is the innermost loop of every dynamic …
  **Fix:** Split the array of structures into two parallel arrays …
- *Also cited:* `rt_node.rs:161-179`

**OX-204 (g4-R4)** — The affix-edge predicate is evaluated twice for the selected edge · **Low** · inferred
- *Loc:* `crates/routerama/src/walk.rs:53-55` and `:77-84` · `descend_iterative` first locates the first … **Impact Low:** only on
  paths with affix … **Fix:** Have the `.find` at `:53` return the …

**OX-205 (g4-R5)** — Method dispatch is a linear scan of 104-byte leaves comparing heap `String`s · **Medium** · EMPIRICAL+INFERRED
- *Loc:* `crates/routerama/src/walk.rs:134-138`; `Leaf` layout in `crates/routerama/src/rt_node.rs` · `dispatch` selects the
  matching leaf with `leaves.iter().find(|leaf| leaf.method == … **Impact Medium:** one pointer chase and `memcmp` per leaf per …
  **Fix:** Store `HttpMethod` (or a `u8` discriminant for the nine …
- *Also cited:* `crates/routerama/src/http_method.rs:81-94`

**OX-206 (g4-R6)** — `Leaf` retains `Vec`/`String` capacity fields that are dead after trie construction · **Low** · EMPIRICAL
- *Loc:* `crates/routerama/src/rt_node.rs` (`Leaf` definition); `crates/routerama/src/raw_match.rs:52-56` (consumer) · `Leaf` is
  built once at trie-compile time and … **Impact Low:** a locality improvement … **Fix:** Change the field types at trie-compile …

**OX-207 (g4-R7)** — `RawMatch::capture` is a linear scan with string comparison · **Low** · EMPIRICAL+INFERRED
- *Loc:* `crates/routerama/src/raw_match.rs:52-56` · `capture(name)` does … **Impact Low:** routes rarely have more than … **Fix:**
  Leave as is for small `vars`. …

**OX-208 (g4-R8)** — `split_verb` is a scalar reverse byte scan, executed as a separate pass from the SIMD segment scan · **Low** ·
  inferred
- *Loc:* `crates/routerama/src/codegen_helpers/scan.rs:28-43` (scalar `split_verb`) vs `:73-98` and `:181+` (SSE2 / NEON
  `scan_segments`) · `split_verb` walks the path backwards with … **Impact Low:** one extra linear pass over a … **Fix:** Extend the
  SIMD scanner to report the …

**OX-209 (g4-R9)** — Path-capture percent-decoding uses a scalar search and allocates per escaped capture, while the crate has SIMD
  helpers it does not reuse · **Low** · inferred
- *Loc:* `crates/routerama/src/decode.rs:15-38` · The decode fast path uses `str::find('%')` … **Impact Low:** the no-escape fast
  path (the … **Fix:** Reuse `query::scan`'s `find_byte` for …

**OX-210 (g4-R10)** — The SIMD query parser makes up to four passes per pair where the scalar parser fuses into one · **Medium** ·
  inferred
- *Loc:* `crates/routerama/src/query/parser.rs:56-92` (SIMD) vs `:99-141` and `:147-173` (scalar); threshold at
  `crates/routerama/src/query/scan.rs:10` · `next_pair_simd` performs, per pair: a `find_byte` for the pair delimiter `&` (`:69`), a
  … **Impact Medium:** this is the query-parsing hot path and the two … **Fix:** Write a fused SIMD `scan_pair` that computes the
  `&` offset …

**OX-211 (g4-R11)** — `ToQuery::to_query_string_with` starts from an uncapacitied `String` · **Low** · inferred
- *Loc:* `crates/routerama/src/query/to_query.rs:65-69` · `let mut output = String::new();` followed by … **Impact Low:**
  client-side request … **Fix:** Add a `size_hint`-style method to …

### `routerama_build`

*8 findings — 1 High, 5 Medium, 2 Low. Examined:* For a build-time crate, "performance" has two faces: the compile-time cost it
  imposes on every downstream crate, and the …

**OX-212 (g4-B1)** — Mixed static+dynamic resolvers scan the request path twice on every dynamic hit · **High** · EMPIRICAL
- *Loc:* `crates/routerama_build/src/macro_impl/resolver.rs:367-397`; consumer at `crates/routerama/src/raw_resolver.rs:145-187`
- *Issue:* The generated `Resolver::resolve` for a table containing both static and dynamic routes emits (at `:375`) a call to
  `__static_resolve`, which performs a full `split_verb` plus a …
- *Impact:* **High** — this is the per-request cost of every dynamic route in any application that mixes static and …
- *Fix:* Surgical, and the pieces already exist. Add `RawResolver::resolve_prescanned(&self, scanned: &ScannedPath, verb …
- *Also cited:* `raw_resolver.rs:156`, `raw_resolver.rs:110-135`, `resolver.rs:367-397`,
  `crates/routerama/benches/routerama_mixed.rs:19-21`

**OX-213 (g4-B2)** — The same request path is scanned a third time when dynamic routes use `:verb` · **Medium** · EMPIRICAL
- *Loc:* `crates/routerama_build/src/macro_impl/resolver.rs:367-376` · The branch guarded by `has_dynamic && has_static &&
  !static_any_verb` emits … **Impact Medium:** narrower than B1 because it requires a specific … **Fix:** Falls out of B1's fix for
  free: once the verb split is …

**OX-214 (g4-B3)** — Generated `resolve` and `__resolve_checked` are unconditionally `#[inline]`, regardless of route-table size ·
  **Medium** · EMPIRICAL
- *Loc:* `crates/routerama_build/src/codegen.rs:223` and `:235`; also `crates/routerama_build/src/macro_impl/resolver.rs:440` and
  `:457` · The generator emits `#[inline]` on the resolve entry points with no regard for how large … **Impact Medium:** harmless
  for the small tables the benchmarks … **Fix:** Have the generator count emitted routes (it already knows …

**OX-215 (g4-B4)** — `emit_node` inlines the entire route trie into one function body via unbounded recursion · **Medium** ·
  EMPIRICAL
- *Loc:* `crates/routerama_build/src/codegen.rs:427-521` · `emit_node` recurses over the trie and emits each child's dispatch inline
  into the … **Impact Medium:** compile-time cost is paid by every downstream … **Fix:** Emit a separate `#[inline(never)]`-free
  helper function per …

**OX-216 (g4-B5)** — Generated literal dispatch is a linear byte-string `match` chain · **Medium** · inferred
- *Loc:* `crates/routerama_build/src/codegen.rs:444-459` · For each trie node the generator emits `match __seg { b"foo" => …, b"bar"
  => …, … }` over … **Impact Medium:** Medium for wide tables, negligible for narrow ones. … **Fix:** For nodes above a sibling
  threshold, emit a `match …
- *Also cited:* `rt_node.rs:161-179`

**OX-217 (g4-B6)** — `syn` is pulled with `full` + `visit` + `derive` as a *default* feature, imposing the heaviest configuration on
  every downstream compile · **Medium** · EMPIRICAL
- *Loc:* `crates/routerama_build/Cargo.toml` (the `syn` dependency and the `codegen` default feature); usage at
  `crates/routerama_build/src/macro_impl/field.rs:9,91-112` and `crates/routerama_build/src/macro_impl/query.rs:822-823` · The `syn`
  feature list is `["clone-impls", "parsing", "proc-macro", "printing", "derive" … **Impact Medium:** Medium (compile time) — `syn`
  with `full` is one of the … **Fix:** Audit whether `full` is reachable. …

**OX-218 (g4-B7)** — Generated query `decode_field` is `#[inline(always)]` over a linear key-match chain · **Low** · inferred
- *Loc:* `crates/routerama_build/src/macro_impl/query.rs` (the `decode_field` emission, ~`:750-780`) · The generated field decoder
  is `#[inline(always)] … **Impact Low:** query schemas are typically … **Fix:** None recommended at current schema …

**OX-219 (g4-B8)** — Per-node `Vec` allocation and sort in `affix_edges_in_match_order` · **Low** · inferred
- *Loc:* `crates/routerama_build/src/trie.rs:304-313`; related build-time work at `:210-231` (`check_bucket` builds a `BTreeMap` and
  `format!`s diagnostics) and `crates/routerama_build/src/codegen.rs` `emit_leaves` (quadratic group `find`) ·
  `affix_edges_in_match_order` allocates a `Vec` … **Impact Low:** all of this is build-time … **Fix:** Only if
  `generator_scaling.rs` shows …

### `routerama_macros`

**No performance issues identified.**

`crates/routerama_macros/src/lib.rs` is 187 lines and contains exactly three public functions, each a `#[proc_macro]` / …

### `seatbelt`

*14 findings — 2 High, 7 Medium, 5 Low. Examined:* `seatbelt` is a well-built resilience middleware crate and the hot paths have
  clearly been thought about … *Also cited:* `engines.rs:35-41`, `engine_core.rs:38`, `health.rs:110-113`, `utils/mod.rs:24-53`,
  `rnd.rs:15-22`, `utils/telemetry_helper.rs:4-14`, `engine/engine_telemetry.rs:17-25`

**OX-220 (g2-F1)** — The breaker engine takes a single global `Mutex<State>` twice on every request, even in the steady closed state
  · **High** · EMPIRICAL
- *Loc:* `crates/seatbelt/src/breaker/engine/engine_core.rs:34-48`; the state it guards at `:50-55`; the `Closed` arm of
  `State::enter` at `:58-62`.
- *Issue:* `EngineCore::enter()` (`:35-40`) does `self.clock.instant()` and then `self.state.lock()`. `EngineCore::exit()`
  (`:42-47`) does the same. …
- *Impact:* **High** — this is a scalability ceiling rather than a fixed latency cost. …
- *Fix:* add an `AtomicU8` state summary alongside the mutex in `EngineCore`, written under the lock whenever `State` …
- *Also cited:* `engines.rs:16-17`

**OX-221 (g2-F2)** — `State` is a 264-byte enum whose largest variant is only live during recovery · **Medium** · EMPIRICAL+INFERRED
- *Loc:* `crates/seatbelt/src/breaker/engine/engine_core.rs:50-55`; payload types at `engine_core.rs:176-187` (`Stats`),
  `health.rs:106-115` (`HealthMetrics`), `probing/probes.rs:12-15` (`Probes`), `probing/health_probe.rs:10-17` (`HealthProbe`). ·
  measured sizes (x86-64): | type | size | align | |---|---|---| | `ExecutionInfo` | 12 | 4 … **Impact Medium:** **Medium** — the
  per-engine memory cost is trivial in … **Fix:** box the recovery-only payload — `HalfOpen { probes …
- *Also cited:* `benches/breaker.rs:73`

**OX-222 (g2-F3)** — Hedging allocates a `FuturesUnordered` on every request even when no hedge can ever fire · **Medium** ·
  inferred
- *Loc:* `crates/seatbelt/src/hedging/service.rs:126-148`, specifically `:141-142`. · `run_hedging` unconditionally does
  `FuturesUnordered::new()` and pushes the primary … **Impact Medium:** **Medium-High** — this is on the *design-target* case …
  **Fix:** when `total_attempts == 1` (i.e. `max_hedged_attempts == 0` …
- *Also cited:* `benches/hedging.rs:50-68`

**OX-223 (g2-F4)** — Retry clones the input on the final attempt and then throws the clone away · **High** · inferred
- *Loc:* `crates/seatbelt/src/retry/service.rs:92-118`, specifically `:102`; the loop-exit logic in `evaluate_attempt`;
  `Attempt::first` in `crates/seatbelt/src/attempt.rs:156-158`.
- *Issue:* the retry loop calls `self.shared.clone_input(input, attempt, ...)` at the top of *every* iteration, including the last
  one. …
- *Impact:* **High for `seatbelt_http` consumers, Medium in the abstract.** For a generic `In` the clone may be cheap. …
- *Fix:* the callback is supplied by the consumer, and `retry` already handles `None` as "no clone available" …
- *Philosophy:* **conflicting** if implemented inside `seatbelt`. Suppressing the `clone_input` callback on the last attempt removes
  an observable …
- *Also cited:* `hedging/service.rs:134`, `crates/seatbelt_http/src/retry.rs:120-122`, `http_clone.rs:58-59`, `benches/retry.rs:80`

**OX-224 (g2-F5)** — Retry builds a backoff-delay iterator on every request, including requests that never retry · **Low** ·
  inferred
- *Loc:* `crates/seatbelt/src/retry/service.rs:98` (`let mut delays = self.shared.backoff.delays();`);
  `crates/seatbelt/src/retry/backoff.rs`. · `delays()` clones `BackoffOptions` into a fresh … **Impact Low:** **Low** — a stack
  copy, not an … **Fix:** make `delays` lazily initialised …

**OX-225 (g2-F6)** — `TelemetryHelper` is deep-cloned once per hedging attempt, including the primary attempt · **Medium** ·
  EMPIRICAL
- *Loc:* `crates/seatbelt/src/hedging/service.rs:266-273` (`create_guard`, the clone at `:271`); call sites at `:140` (primary
  attempt — i.e. **every request**) and `:254` (each hedge); the type at `crates/seatbelt/src/utils/telemetry_helper.rs:4-14`;
  `TelemetryString = Cow<'static, str>` at `crates/seatbelt/src/lib.rs:282`. · `TelemetryHelper` holds two `Cow<'static, str>` (24
  bytes each), an … **Impact Medium:** **Medium** — the atomic pair is unconditional whenever … **Fix:** store
  `Arc<TelemetryHelper>` in `HedgingShared` and clone the …
- *Also cited:* `hedging/service.rs:71-76`, `benches/hedging.rs:14-15`, `Cargo.toml:182-185`

**OX-226 (g2-F7)** — Metric attribute arrays are rebuilt per emission although they are constant per engine · **Medium** · EMPIRICAL
- *Loc:* `crates/seatbelt/src/breaker/engine/engine_telemetry.rs:50-58` (rejection), `:79-88` (probe), `:117-125` and onward
  (`report_state_change`). · each emission constructs a fresh 5- or 6-element `[KeyValue]` array in which four of the … **Impact
  Medium:** **Medium**, and specifically on the *worst possible* … **Fix:** precompute the invariant attribute sets in …
- *Also cited:* `chaos/injection/service.rs:198`, `benches/breaker.rs:38`, `breaker_cg.rs:65`

**OX-227 (g2-F8)** — `ProbesOptions::probes()` heap-clones a `Vec` while the engine mutex is held · **Medium** · inferred
- *Loc:* `crates/seatbelt/src/breaker/engine/probing/options.rs:83-85` (`self.probes.clone().into_iter()`); consumed by
  `crates/seatbelt/src/breaker/engine/probing/probes.rs:18-26`; called from `State::enter`'s `Open → HalfOpen` transition at
  `crates/seatbelt/src/breaker/engine/engine_core.rs:65`, which runs **inside** `self.state.lock()` (`engine_core.rs:39`). · the
  `Vec<ProbeOptions>` is cloned (heap allocation + element copy; `ProbeOptions` is 40 … **Impact Medium:** **Medium** — the
  transition itself is rare, but it … **Fix:** store the probe list as `Arc<[ProbeOptions]>` in …
- *Also cited:* `engine_core.rs:38`

**OX-228 (g2-F9)** — `Engines::get_engine` clones the `BreakerId` on the insert path while holding the write lock · **Low** ·
  inferred
- *Loc:* `crates/seatbelt/src/breaker/engine/engines.rs:54-60`, specifically `map.entry(key.clone())` at `:57` and the
  `create_engine` call at `:58`. · the write-lock arm clones the `BreakerId` (a … **Impact Low:** **Low-Medium** — this is a …
  **Fix:** use `if let Some(e) = map.get(key) { …
- *Also cited:* `benches/breaker_cg.rs:118-120`

**OX-229 (g2-F10)** — The crate has 2 `#[inline]` attributes across 128 public functions · **Medium** · EMPIRICAL+INFERRED
- *Loc:* workspace-wide; the only two are `crates/seatbelt/src/retry/backoff.rs:115` and `:147`. Counted with `grep -rn '#\[inline'
  crates/seatbelt/src` (2 hits) versus `grep -rn 'pub fn '` (128 hits). · `docs/performance.md` rule 1 says `#[inline]` should be
  applied to non-generic exported … **Impact Medium:** **Medium** — invisible in this repo's own measurements … **Fix:** audit the
  small, non-generic, public, per-request-path …
- *Also cited:* `Cargo.toml:340-346`

**OX-230 (g2-F11)** — Chaos strategies draw a random number unconditionally, even when the injection rate is zero · **Low** ·
  EMPIRICAL
- *Loc:* `crates/seatbelt/src/chaos/injection/service.rs:190-193`; the same shape in `crates/seatbelt/src/chaos/latency/service.rs`.
  · `should_inject` evaluates the (user-supplied … **Impact Low:** **Low-Medium** — but note the … **Fix:** `if rate <= 0.0 { return
  false; }` …
- *Also cited:* `Cargo.toml:162-210`

**OX-231 (g2-F12)** — Exponential backoff uses `f64::powi` · **Low** · inferred
- *Loc:* `crates/seatbelt/src/retry/backoff.rs:105-108` (`duration_mul_pow2`). · the backoff multiplier is computed with a …
  **Impact Low:** **Low** — this is on the … **Fix:** none recommended. If touched at all …

**OX-232 (g2-F13)** — The entire `tower-service` surface boxes its future on every call · **Medium** · inferred
- *Loc:* `crates/seatbelt/src/breaker/service.rs:152,162,176`; `crates/seatbelt/src/timeout/service.rs:167,175`;
  `crates/seatbelt/src/chaos/injection/service.rs:183-185`; `crates/seatbelt/src/hedging/service.rs:276-280` (`HedgingFuture` is
  literally `Pin<Box<dyn Future + Send>>`); the equivalent in `fallback` and `chaos/latency`. · `tower_service::Service` has an
  associated `Future` type that must be nameable, and … **Impact Medium:** **Medium** for `tower` consumers, **zero** for `layered`
  … **Fix:** none that is both surgical and safe. …
- *Philosophy:* **conflicting** — the only real fix is architectural, is explicitly discouraged by `docs/performance.md`'s "surgical
  over …

**OX-233 (g2-F14)** — `ExitCircuitResult` is a 64-byte return value produced on every request · **Low** · EMPIRICAL+INFERRED
- *Loc:* `crates/seatbelt/src/breaker/engine/engine_core.rs:42-47` (`exit` returns it); the type in
  `crates/seatbelt/src/breaker/engine/mod.rs`; the `Stats` payload at `engine_core.rs:176-187`. · `ExitCircuitResult::Closed(Stats)`
  forces the … **Impact Low:** **Low** — return-slot writes for … **Fix:** `Closed(Box<Stats>)`, allocating only …
- *Also cited:* `engine_telemetry.rs:104-108`

### `seatbelt_http`

*8 findings — 4 High, 4 Low. Examined:* `seatbelt_http` is a thin adapter: five public functions, four feature-gated modules, and no
  benchmarks. …

**OX-234 (g2-H1)** — The default breaker ID formats a fresh `String` per request — and thereby defeats seatbelt's lock-free fast
  path · **High** · EMPIRICAL
- *Loc:* `crates/seatbelt_http/src/breaker.rs:154-159` (`create_breaker_id`); installed by `http_configure_defaults` at `:119-123`,
  specifically the `.breaker_id(|req: &HttpRequest| create_breaker_id(req.uri()))` on line `:122`.
- *Issue:* two compounding costs.
- *Impact:* **High** — it is per-request, it is on the path the crate's own documentation recommends, and it converts an …
- *Fix:* add a `BreakerId` constructor that stores `http`'s refcounted `Scheme` and `Authority` (or their `Bytes` buffers) …
- *Also cited:* `crates/seatbelt/src/breaker/engine/engines.rs:35-61`, `crates/seatbelt/src/breaker/service.rs:48`,
  `crates/seatbelt_http/Cargo.toml:47-53`, `crates/seatbelt/benches/breaker.rs:92`

**OX-235 (g2-H2)** — The default clone strategy deep-clones the whole `HttpRequest` per request · **High** · inferred
- *Loc:* `crates/seatbelt_http/src/http_clone.rs:43-65` (`HttpClone::try_clone`), specifically `request.try_clone()` at `:50`;
  installed by `HttpRetryLayerExt::http_configure_defaults` (`crates/seatbelt_http/src/retry.rs:114-118`, via `http_clone` at
  `:120-122`) and `HttpHedgingLayerExt::http_configure_defaults` (`crates/seatbelt_http/src/hedging.rs:100-102`, via `:104-106`).
  The underlying clone is `crates/http_extensions/src/extensions/http_request_ext.rs:21-31`.
- *Issue:* `HttpRequest::try_clone` clones the body, clones the `HeaderMap` (one heap allocation plus per-header value clones), and
  clones the `Extensions` map (a boxed `AnyMap` allocation …
- *Impact:* **High** — it is unconditional on the recommended configuration, and by far the largest per-request cost in …
- *Fix:* the clone itself is necessary for multi-attempt configurations and cannot be removed without changing semantics. …
- *Also cited:* `http_request_ext.rs:21-31`

**OX-236 (g2-H3)** — The clone on the final attempt is always discarded — and skipping it also unlocks seatbelt's hedging
  early-return · **High** · inferred
- *Loc:* `crates/seatbelt_http/src/retry.rs:120-122` and `crates/seatbelt_http/src/hedging.rs:104-106` (the two `http_clone`
  installers); the consuming logic at `crates/seatbelt/src/retry/service.rs:102-116` and
  `crates/seatbelt/src/hedging/service.rs:130-142`.
- *Issue:* both installers call `clone_strategy.try_clone(request, args.attempt(), ...)` unconditionally. On the last attempt the
  retry loop always breaks (`retry/service.rs:108-116`), so the …
- *Impact:* **High** — a two-line change in each installer that removes 4-8 heap allocations plus a `HeaderMap` and …
- *Fix:* in `HttpClone::try_clone` (`http_clone.rs:43-65`), skip the `request.try_clone()` at `:50` when …
- *Philosophy:* **conflicting only in its rejected variant.** Implementing this inside `seatbelt` — by having
  `RetryShared::clone_input` not …
- *Also cited:* `hedging/service.rs:130`, `retry.rs:130-132`

**OX-237 (g2-H4)** — `attach_attempt` forces an `Extensions` allocation on requests that have none · **Low** · inferred
- *Loc:* `crates/seatbelt_http/src/http_clone.rs:106-108`; called from `try_clone` at `:58`. ·
  `request.extensions_mut().insert(attempt)` runs … **Impact Low:** **Low-Medium** — one allocation … **Fix:** none that is clearly
  correct. …

**OX-238 (g2-H5)** — `update_request_uri` performs a typed extension lookup on every request · **Low** · inferred
- *Loc:* `crates/seatbelt_http/src/http_clone.rs:78-96`, specifically the `request.extensions().get::<Router>()` at `:84`; called
  unconditionally from `try_clone` at `:59`. · every request through a default retry/hedging … **Impact Low:** **Low** — a hash
  lookup with a … **Fix:** reorder the condition so the cheap …
- *Also cited:* `Cargo.toml:47-53`

**OX-239 (g2-H6)** — `seatbelt_http` has no benchmarks of any kind · **High** · EMPIRICAL
- *Loc:* `crates/seatbelt_http/Cargo.toml` — no `[[bench]]` section (contrast `crates/seatbelt/Cargo.toml:162-210`), no `benches/`
  directory, and dev-dependencies (`:47-53`) containing no `criterion`, no `gungraun`, no `alloc_tracker`, no `benchmarking`.
- *Issue:* the crate whose default configuration installs the most expensive per-request callbacks in the whole `seatbelt` family
  (H1, H2) has zero performance measurement. …
- *Impact:* **High** (as a process finding).
- *Fix:* add `crates/seatbelt_http/benches/{breaker,retry,hedging}.rs` plus `*_cg.rs` pairs, following `docs/naming.md` and …
- *Also cited:* `crates/seatbelt/benches/breaker.rs:92`, `benches/retry.rs:80`

**OX-240 (g2-H7)** — The crate declares no `default` feature, so `cargo add seatbelt_http` yields an empty library · **Low** ·
  EMPIRICAL
- *Loc:* `crates/seatbelt_http/Cargo.toml:34-38` — the `[features]` table lists `timeout`, `retry`, `hedging`, `breaker` and no
  `default` key at all; the modules are gated in `crates/seatbelt_http/src/lib.rs:52-72`. (`crates/seatbelt` at least states
  `default = []` explicitly, `Cargo.toml:48`.) · a user who adds the crate without features … **Impact Low:** **Low** — a
  build/ergonomics … **Fix:** add an explicit `default = []` for …

**OX-241 (g2-H8)** — Feature-gated `Box`-free design is preserved, but every default helper installs an `Arc<dyn Fn>` indirection ·
  **Low** · inferred
- *Loc:* `crates/seatbelt_http/src/http_recovery.rs:91-96` (`CustomDelegate = Arc<dyn Fn(&HttpResponse, &Clock) -> RecoveryInfo +
  Send + Sync>`, `Inner::Default | Inner::Custom`); `HttpRecovery::recovery` at `:64-69`; the closures installed by `http_recovery`
  in `retry.rs:124-128`, `hedging.rs:108-112` and `breaker.rs:125-129`. · `HttpRecovery` itself is well designed — … **Impact Low:**
  **Low** — one predictable indirect … **Fix:** none recommended. Recorded to close out …
- *Also cited:* `crates/seatbelt/src/utils/define_fn_wrapper.rs:29,95`

### `templated_uri`

*8 findings — 1 High, 4 Medium, 3 Low. Examined:* `templated_uri` performs RFC 6570-style URI template expansion: a derive on a
  struct produces a `PathAndQueryTemplate` … *Also cited:* `crates/templated_uri/src/base_path.rs:66-93`,
  `crates/templated_uri/src/base_path.rs:119-146`, `crates/templated_uri/src/escaped.rs:265`,
  `crates/templated_uri/src/path_and_query.rs:62-67`

**OX-242 (g6-T1)** — The crate has exactly one `#[inline]` attribute across ~43 public non-generic functions · **High** · EMPIRICAL
- *Loc:* whole of `crates/templated_uri/src/`. The single `#[inline]` is on `EscapedString::from_static` in
  `crates/templated_uri/src/escaped.rs`. Public non-generic `pub fn` counts per file: `base_uri.rs` 15, `origin.rs` 9, `uri.rs` 9,
  `path_and_query.rs` 5, `escaped.rs` 3, `base_path.rs` 2.
- *Issue:* `docs/performance.md` rule 1 says `#[inline]` should be applied to **non-generic exported functions on a hot path**, on
  knowledge alone, without requiring measurement — precisely …
- *Impact:* High — it applies to essentially every accessor on the crate's public surface, on the documented hot path, in …
- *Fix:* Surgical and mechanical: add `#[inline]` to the small non-generic public accessors on `Uri`, `BaseUri`, `Origin` …

**OX-243 (g6-T2)** — `Uri::to_string` allocates twice and renders through `core::fmt` · **Medium** · inferred
- *Loc:* `crates/templated_uri/src/uri.rs:200-211`. · The `Display` implementation formats the base into a `String` (via … **Impact
  Medium:** URI stringification is what happens on every … **Fix:** Surgical: implement `Display for Uri` to write the origin and …
- *Also cited:* `crates/templated_uri/src/path_and_query.rs:53-58`, `crates/templated_uri/src/path_and_query.rs:62-67`,
  `crates/templated_uri/src/base_path.rs:125-143`, `base_path.rs:125-143`

**OX-244 (g6-T3)** — Redacted formatting allocates a `String` per call, and the two impls are exact duplicates · **Low** · inferred
- *Loc:* `crates/templated_uri/src/uri.rs:214-227` (`RedactedDisplay for Uri`) and `crates/templated_uri/src/uri.rs:229-245`
  (`RedactedDebug for Uri`). · Both implementations are, to the byte, the same … **Impact Low:** one allocation per redacted …
  **Fix:** Two parts, both worth doing …

**OX-245 (g6-T4)** — Integer, `IpAddr` and `Uuid` template parameters render through `core::fmt` and allocate · **Medium** ·
  inferred
- *Loc:* `crates/templated_uri/src/escape.rs:33-38` (the default `escape_into` / `raw_into` bodies, `write!(out, "{}",
  self.escape())`) and the impls for integers / `IpAddr` / `Uuid` that do not override them. · `EscapedString` overrides
  `escape_into` / `raw_into` to a direct `push_str`. … **Impact Medium:** integer path parameters are the single most … **Fix:**
  Surgical: override `escape_into` / `raw_into` for the integer …

**OX-246 (g6-T5)** — `Sensitive<T>` does not forward the fast render path to its inner type · **Medium** · inferred
- *Loc:* `crates/templated_uri/src/escape.rs:128-144` (the `Escape` / `Raw` implementations for `Sensitive<T>`). · `Sensitive<T>`
  implements the escape traits but inherits the default `escape_into` / … **Impact Medium:** it is a silent, invisible performance
  cliff … **Fix:** Surgical and small: implement `escape_into` and `raw_into` on …
- *Also cited:* `escape.rs:33-38`

**OX-247 (g6-T6)** — `Origin::fmt` uses four `write!` invocations where `write_str` would do · **Low** · inferred
- *Loc:* `crates/templated_uri/src/origin.rs:268-277`. · The `Display` implementation is a sequence of … **Impact Low:** Medium —
  small per call, but … **Fix:** Surgical …

**OX-248 (g6-T7)** — `percent_encode` emits each escape byte as a `char` · **Low** · inferred
- *Loc:* `crates/templated_uri/src/escaped.rs:283-315`. · The escape emission is `out.push('%')` followed … **Impact Low:** three
  branches per escaped … **Fix:** Surgical: build the three bytes in a …
- *Also cited:* `crates/templated_uri/src/escaped.rs:265`

**OX-249 (g6-T8)** — Benchmark group names are not file-prefixed, and the `Display`/redaction surface is unbenchmarked · **Medium**
  · EMPIRICAL
- *Loc:* `crates/templated_uri/benches/escaped_string.rs:56` (group `escaped_construct`), `:81` (group `request_construct`);
  `crates/templated_uri/benches/routing_rerender.rs:116` (group `route_materialize`), `:150` (`per_send`), `:194`
  (`per_send_hedged_x3`); rule at `docs/benchmarks.md` and `docs/naming.md`. · Two things. 1. `docs/benchmarks.md` requires
  Criterion group names to be prefixed with … **Impact Medium:** for the ability to detect regressions in … **Fix:** Rename the five
  groups to `escaped_string/...` and …
- *Also cited:* `escaped_string_cg.rs:114`, `routing_rerender_cg.rs:128`

### `templated_uri_macros`

**No performance issues identified.**

`crates/templated_uri_macros/src/lib.rs` is 35 lines: a `proc-macro = true` shim exporting the `templated` attribute macro …

### `templated_uri_macros_impl`

*4 findings — 1 High, 2 Medium, 1 Low. Examined:* The expansion crate behind `templated_uri_macros`: 2139 lines across
  `template_parser.rs`, `struct_template.rs` … *Also cited:* `struct_template.rs:138-190`

**OX-250 (g6-U1)** — `chumsky` — a full parser-combinator framework — is pulled into every consumer's build graph to parse a small
  fixed grammar · **High** · EMPIRICAL
- *Loc:* `crates/templated_uri_macros_impl/Cargo.toml` (dependency `chumsky = { workspace = true, features = ["std"] }`);
  `crates/templated_uri_macros_impl/src/template_parser.rs:6` (`use chumsky::prelude::*`).
- *Issue:* The URI template grammar this crate parses is small and fixed: literal segments, `{name}` placeholders, and a handful of
  operators. …
- *Impact:* High — this is compile-time cost, paid by every developer and every CI run of every crate that depends on …
- *Fix:* Replace the `chumsky` grammar in `template_parser.rs` with a hand-written scanner. …

**OX-251 (g6-U2)** — `darling` is pulled in for a handful of attribute fields · **Medium** · EMPIRICAL
- *Loc:* `crates/templated_uri_macros_impl/Cargo.toml` (dependency `darling`); used in
  `crates/templated_uri_macros_impl/src/struct_template.rs` for attribute parsing. · `darling` is a derive-helper framework whose
  transitive closure is **7 packages** … **Impact Medium:** smaller than U1 but the same kind of cost, on … **Fix:** Parse the
  attributes directly with `syn`'s …

**OX-252 (g6-U3)** — `ohno` — a runtime error framework — is a build dependency of a proc-macro crate · **Medium** · EMPIRICAL
- *Loc:* `crates/templated_uri_macros_impl/Cargo.toml` (dependency `ohno`); `crates/templated_uri_macros_impl/src/error.rs` (17
  lines, one `#[ohno::error]` type). · `ohno` is one of the workspace's own runtime crates. … **Impact Medium:** same critical-path
  compile-time cost as U1 and … **Fix:** Replace the `#[ohno::error]` type in `error.rs` with a plain …

**OX-253 (g6-U4)** — Nothing guards the compile-time cost of any macro crate · **Low** · EMPIRICAL
- *Loc:* `crates/templated_uri_macros_impl/` (no benches, no compile-time test); workspace-wide. · U1, U2 and U3 each entered the
  codebase silently … **Impact Low:** Low as a runtime matter; Medium as … **Fix:** Add a cheap test asserting that each …

### `testing_aids`

*2 findings — 1 Low, 1 —. Examined:* The question this crate had to answer is "does any of it reach a release build", and the answer
  is a clean **no**. …

**OX-254 (g8-F34)** — Heavy real `[dependencies]` inflate test build time (but not release builds) · **Low** · inferred
- *Loc:* `crates/testing_aids/Cargo.toml` — the `[dependencies]` section carries `opentelemetry_sdk`, `tracing-subscriber` and
  `futures`. · These are substantial dependency trees. … **Impact Low:** Low, and compile-time only. … **Fix:** No action
  recommended. …

**OX-255 (g8-F35)** — POSITIVE — zero release-path leakage, verified across all eight consumers · **—** · EMPIRICAL
- *Loc:* the eight consuming manifests, each listing `testing_aids` under `[dev-dependencies]`: `crates/seatbelt/Cargo.toml:95`,
  `crates/fetch/Cargo.toml:135`, `crates/ohno/Cargo.toml:39`, `crates/bytesbuf_io/Cargo.toml:42`,
  `crates/recoverable/Cargo.toml:29`, `crates/bytesbuf/Cargo.toml:58`, `crates/cachet/Cargo.toml:83`,
  `crates/fetch_hyper/Cargo.toml:109`. Also declared at root `Cargo.toml:211`. · Not one consumer lists it under `[dependencies]`. …
  **Impact —:** N/A — this is the desired state. …
- *Also cited:* `Cargo.toml:5`, `crates/testing_aids/src/lib.rs:8-9`

### `thread_aware`

*9 findings — 2 High, 3 Medium, 4 Low. Examined:* `thread_aware` provides `ThreadAware` (a trait for types that must be told when
  they move between processors) and a …

**OX-256 (g7-TA1)** — `thread_aware::Arc` is 48 bytes (64 for `dyn`), six to eight times `std::sync::Arc` · **High** · EMPIRICAL
- *Loc:* `crates/thread_aware/src/cell/mod.rs:109-114` (the struct), `crates/thread_aware/src/cell/factory.rs:18-30` (the `Factory`
  enum), `crates/thread_aware/src/cell/clone_fn.rs:19-23` (`ErasedCloneFn`), `crates/thread_aware/src/affinity.rs` (`Affinity`).
- *Issue:* The struct carries a `std::sync::Arc<RwLock<Storage<…>>>` (8), a `Factory<T>` (32) and the cached value/affinity fields.
  …
- *Impact:* **High** — not because 48 bytes is inherently expensive, but because the type is *presented as an `Arc`* and …
- *Fix:* The surgical, non-architectural mitigation is **not** to redesign `thread_aware::Arc` but to stop using it where …
- *Philosophy:* **Conflicting.** `docs/performance.md` prefers surgical interventions over architectural rewrites. …

**OX-257 (g7-TA2)** — `Arc::clone` performs 3 atomic RMWs (4 for the `ErasedCloneFn` factory), not 1 · **High** · EMPIRICAL+INFERRED
- *Loc:* `crates/thread_aware/src/cell/mod.rs:142-150` (`impl Clone`), `crates/thread_aware/src/cell/factory.rs:43-52`
  (`Factory::clone`), `crates/thread_aware/src/cell/clone_fn.rs:70-77` (`ErasedCloneFn::clone`, which clones **two** inner `Arc`s).
- *Issue:* `Arc::clone` clones (1) the storage `Arc<RwLock<Storage<…>>>`, (2) the cached value `Arc<T>`, and (3) the `Factory`. …
- *Impact:* **High** — an uncontended atomic RMW is ~20 cycles; a *contended* one (the same line being written by another …
- *Fix:* Surgical options that do not change the design: 1. Store the `Factory` *inside* the already-shared …
- *Also cited:* `crates/anyspawn/src/custom.rs:92`

**OX-258 (g7-TA3)** — `strong_count` takes a read lock and scans every storage slot · **Medium** · inferred
- *Loc:* `crates/thread_aware/src/cell/mod.rs:571-576`, `crates/thread_aware/src/cell/storage.rs:70-85`. · `strong_count` acquires
  the storage `RwLock` for reading and then walks the entire slot … **Impact Medium:** **Medium** — it is not itself a hot path
  *within* … **Fix:** Either (a) document loudly on the method that it is …

**OX-259 (g7-TA4)** — Zero `#[inline]` across 27 public functions · **Medium** · EMPIRICAL+INFERRED
- *Loc:* whole crate. Hot ones: `crates/thread_aware/src/cell/mod.rs` (`Deref::deref`, `as_ref`),
  `crates/thread_aware/src/affinity.rs` (the `u16` accessors — `processor_id`, `memory_region_id` etc.),
  `crates/thread_aware/src/cell/storage.rs` (`get_clone`). · 0 `#[inline]` attributes against 27 public functions. … **Impact
  Medium:** **Medium** — `deref` frequency is essentially "every … **Fix:** Add `#[inline]` to `Deref::deref`, `AsRef::as_ref`, the
  …

**OX-260 (g7-TA5)** — `relocate` takes the storage write lock even when only a read is needed · **Medium** · inferred
- *Loc:* `crates/thread_aware/src/cell/mod.rs:585-648`, specifically the unconditional write-lock acquisition at `:587` and the
  early already-present check at `:589-600`. · `relocate` acquires the storage `RwLock` for **writing** at the top of the function,
  then … **Impact Medium:** **Medium** — `relocate` is called whenever a … **Fix:** Double-checked locking: take the read lock,
  check whether the …

**OX-261 (g7-TA6)** — `Affinity` has no niche, inflating `Option<Affinity>` and `Factory` · **Low** · EMPIRICAL
- *Loc:* `crates/thread_aware/src/affinity.rs` (the four `u16` fields), `crates/thread_aware/src/cell/factory.rs:18-30`. ·
  `Affinity` is four plain `u16`s, so every bit … **Impact Low:** **Low** — and, importantly, fixing … **Fix:** Not worth doing on
  its own. …

**OX-262 (g7-TA7)** — Erased closures cost a `Box` inside an `Arc` — double indirection per relocation · **Low** · inferred
- *Loc:* `crates/thread_aware/src/closure/erased.rs:14-16` (`ErasedClosureOnce` holds `Box<dyn Erased<T>>`), `:52-58` (`Clone` calls
  `clone_boxed`), `:78-82` (`clone_boxed` heap-allocates a fresh `Box`), `:46-50` (`relocate` is a virtual call through the box). ·
  Cloning an `ErasedClosureOnce` **allocates** — … **Impact Low:** **Low** — this is on the … **Fix:** None recommended. If ever
  needed …
- *Also cited:* `erased.rs:79`

**OX-263 (g7-TA8)** — `Storage` grows with `resize_with`, reallocating the slot vector · **Low** · inferred
- *Loc:* `crates/thread_aware/src/cell/storage.rs` (the `resize_with` / slot-growth path). · Slots are added lazily as new
  processors touch … **Impact Low:** **Low** — bounded by processor … **Fix:** Pre-size the vector to the …

**OX-264 (g7-TA9)** — `count_where` iterates every slot under the lock · **Low** · inferred
- *Loc:* `crates/thread_aware/src/cell/storage.rs` (`count_where`), called from `crates/thread_aware/src/cell/mod.rs:571-576`. ·
  Takes a predicate by value and performs a full … **Impact Low:** **Low** — subsumed by **TA3** … **Fix:** See **TA3**.

### `thread_aware_macros`

*1 finding — 1 Low. Examined:* A 27-line facade crate that re-exports the derive macro from `thread_aware_macros_impl`. …

**OX-265 (g7-MA1)** — Facade adds one proc-macro crate to the build graph · **Low** · inferred
- *Loc:* `crates/thread_aware_macros/src/lib.rs:1-27`. · The crate exists only to forward … **Impact Low:** **Low** — this is the
  deliberate … **Fix:** None. Deviating from the ecosystem …

### `thread_aware_macros_impl`

*3 findings — 1 Medium, 2 Low. Examined:* The real derive implementation: `struct_gen.rs` and `enum_gen.rs` generate
  `ThreadAware::relocate` bodies. …

**OX-266 (g7-MI1)** — `parse_quote!` and a path clone are executed once per field, inside the loop · **Medium** · inferred
- *Loc:* `crates/thread_aware_macros_impl/src/struct_gen.rs:17-18` and `:30-31`;
  `crates/thread_aware_macros_impl/src/enum_gen.rs:25-26` and `:40-41`. · Inside the per-field loop, each generator performs
  `root_path.clone()` and … **Impact Medium:** **Medium** (compile-time) — proc-macro expansion time is … **Fix:** Hoist both out of
  the loop: ```rust let trait_path: syn::Path …

**OX-267 (g7-MI2)** — `collect_generics_in_fields` rebuilds its `HashSet` per enum variant · **Low** · inferred
- *Loc:* `crates/thread_aware_macros_impl/src/lib.rs:82-116` (`add_bounds`), with the per-variant rebuild at `:87-93` and the call
  site at `:121-128`. · `add_bounds` constructs a `HashSet` of the type's … **Impact Low:** **Low** (compile-time) — enums …
  **Fix:** Build the `HashSet` once in the enum …

**OX-268 (g7-MI3)** — Generated code is sound; one minor emission redundancy · **Low** · inferred
- *Loc:* `crates/thread_aware_macros_impl/src/enum_gen.rs:20-60`. · The enum generator always emits a `match self { … … **Impact
  Low:** **Low** — an empty `match` on a … **Fix:** Emit an empty body when no variant has …

### `tick`

*10 findings — 2 High, 5 Medium, 3 Low. Examined:* `tick` is the clock abstraction: `Clock` (system time, monotonic instants,
  delays, timeouts, periodic timers, stopwatches) …

**OX-269 (g7-T1)** — Every delay and timeout registers through a process-wide mutex · **High** · inferred
- *Loc:* `crates/tick/src/state.rs:71-90` (the `SynchronizedTimers` enum and the comment claiming this is not a hot path),
  `crates/tick/src/state.rs:112-127` (`with_timers`), `crates/tick/src/clock.rs:458-472` (`register_timer` / `unregister_timer`),
  `crates/tick/src/delay.rs:88-113`.
- *Issue:* `SynchronizedTimers` has two variants. The `Isolated` variant stores the timers in a `thread_aware::Arc<Mutex<Timers>,
  PerThread>`-style per-thread cell, which is fine for …
- *Impact:* **High** — a single uncontended `Mutex` lock/unlock pair is ~20–40 cycles, which is unremarkable; the problem …
- *Fix:* Surgical, in increasing order of invasiveness: 1. Shard the `Shared` variant: replace `Mutex<Timers>` with a small …
- *Also cited:* `state.rs:71-77`

**OX-270 (g7-T2)** — Wakers are invoked while the timers mutex is held — guaranteed lock convoy · **High** · EMPIRICAL
- *Loc:* `crates/tick/src/timers.rs:101-130` (`advance_timers`, with the wake loop at `:118-120`), reached through
  `crates/tick/src/state.rs:112-127` (`with_timers` holds the lock for the whole closure), driven from
  `crates/tick/src/runtime/clock_driver.rs:39-52`.
- *Issue:* `advance_timers` splits the expired entries out of the `BTreeMap` and then calls `waker.wake()` on each of them —
  *inside* the `with_timers` closure, i.e. with the mutex still held. …
- *Impact:* **High** — this is a contention amplifier that fires precisely when contention is highest (a batch of …
- *Fix:* Surgical and entirely contained in `timers.rs::advance_timers`: `let expired = …split_off…;` inside the closure …

**OX-271 (g7-T3)** — Redundant `unregister_timer` on the normal timer-fired path · **Medium** · EMPIRICAL
- *Loc:* `crates/tick/src/delay.rs:98-113` (the ready branch), `crates/tick/src/periodic_timer.rs:126-140` (same pattern),
  `crates/tick/src/clock.rs:465-472` (`unregister_timer`). · When a timer fires, `Timers::advance_timers` has *already* removed its
  entry from the … **Impact Medium:** **Medium** — a third of the lock traffic on the timeout … **Fix:** Track registration state in
  the `Delay` (it already has a …
- *Also cited:* `timers.rs:113-115`

**OX-272 (g7-T4)** — A whole `Clock` is cloned per delay, timeout and periodic timer · **Medium** · EMPIRICAL+INFERRED
- *Loc:* `crates/tick/src/delay.rs:72-78` (`Delay::new`), `crates/tick/src/clock.rs:425-431` (`Clock::delay`),
  `crates/tick/src/periodic_timer.rs:95-105`, `crates/tick/src/future_ext.rs:28-38` (`FutureExt::timeout`),
  `crates/tick/src/timeout.rs` (holds the cloned `Clock` for the future's lifetime). · Every `Delay`, `Timeout` and `PeriodicTimer`
  stores its own cloned `Clock`. … **Impact Medium:** **Medium** — atomic RMWs on a shared refcount are the … **Fix:** Two options,
  both surgical: 1. Have `Delay` / `Timeout` …
- *Also cited:* `cell/mod.rs:142-150`, `factory.rs:43-52`

**OX-273 (g7-T5)** — Zero `#[inline]` across 35 public functions crossing the crate boundary · **Medium** · EMPIRICAL+INFERRED
- *Loc:* whole crate. Representative offenders on the hot path: `crates/tick/src/clock.rs` (`system_time`, `instant`,
  `simple_clock`), `crates/tick/src/simple_clock.rs:57-90` (`system_time`, `instant`), `crates/tick/src/stopwatch.rs:56-90` (`new`,
  `elapsed`). · A census of the crate finds **zero** `#[inline]` attributes against 35 public functions. … **Impact Medium:**
  **Medium** — individually a call/ret is a handful of … **Fix:** Add `#[inline]` judiciously — not blanket. …

**OX-274 (g7-T6)** — Idle clock driver pays an RwLock read and an O(processors) scan every tick · **Medium** · inferred
- *Loc:* `crates/tick/src/runtime/clock_driver.rs:39-52` (`advance_timers`, the `self.state.is_unique()` call at `:49`),
  `crates/tick/src/state.rs:129-140` (`is_unique`), `crates/thread_aware/src/cell/mod.rs:571-576` (`Arc::strong_count`),
  `crates/thread_aware/src/cell/storage.rs:70-85`. · `ClockDriver::advance_timers` is called by the runtime on *every* tick,
  including when … **Impact Medium:** **Medium** — it is idle-path work, so it does not affect … **Fix:** Only check `is_unique()`
  when the timer map has just …

**OX-275 (g7-T7)** — `test-util` feature unification makes every repo benchmark measure the slow clock · **Medium** · inferred
- *Loc:* `crates/tick/src/simple_clock.rs:30-50` (the `TimeKind` enum, whose `Controlled(ClockControl)` variant exists only under
  `test-util`), `crates/tick/src/stopwatch.rs:56-60`, `crates/tick/Cargo.toml` (`test-util` feature); consumers declaring `tick = {
  …, features = ["test-util"] }` in `[dev-dependencies]` include `fetch`, `seatbelt`, `cachet` and `http_extensions`. · Without
  `test-util`, `TimeKind` has a single variant, `SimpleClock` is effectively a ZST … **Impact Medium:** **Medium** — the per-call
  delta is small (a predictable … **Fix:** No code change to `tick` itself. Either (a) document in …

**OX-276 (g7-T8)** — `advance_timers` allocates a fresh `BTreeMap` per firing batch · **Low** · inferred
- *Loc:* `crates/tick/src/timers.rs:101-130`, specifically the `split_off` + `mem::replace` pair at `:113-115`. · The expired-timer
  extraction is `let mut expired … **Impact Low:** **Low** — it happens once per … **Fix:** For the common case of few expiries …

**OX-277 (g7-T9)** — `PeriodicTimer` re-anchors each tick on a fresh clock read, accumulating drift · **Low** · inferred
- *Loc:* `crates/tick/src/periodic_timer.rs:107-119`. · The next deadline is computed as … **Impact Low:** **Low** as a
  *performance* matter … **Fix:** Anchor on the previous deadline …

**OX-278 (g7-T10)** — `Clock` is copied by value into every future it constructs, and is not small · **Low** · EMPIRICAL+INFERRED
- *Loc:* `crates/tick/src/clock.rs:40-70` (the `Clock` struct), `crates/tick/src/state.rs:78-90` (`ClockState`), plus every
  constructor listed in **T4**. · `Clock` holds a `ClockState` (an enum whose … **Impact Low:** **Low** on its own — it is …
  **Fix:** Deferred to **TA1**; nothing to do …

### `uniflight`

*8 findings — 3 High, 3 Medium, 2 Low. Examined:* `uniflight` coalesces duplicate concurrent async work: N callers asking for the
  same key run the work once and all receive a … *Also cited:* `crates/uniflight/src/lib.rs:350-372`

**OX-279 (g7-U1)** — Every completion takes a DashMap shard write lock · **High** · EMPIRICAL
- *Loc:* `crates/uniflight/src/lib.rs:370` (`inner.remove_if(owned_key.borrow(), |_, weak| weak.upgrade().is_none())`), within
  `execute` at `:350-372`.
- *Issue:* After awaiting the result, *every* caller — leader and all followers — calls `DashMap::remove_if`. …
- *Impact:* **High** — this is the top finding for the crate and, arguably, for the cluster. …
- *Fix:* Surgical and contained to `execute`. Only the **last** holder should attempt removal. …

**OX-280 (g7-U2)** — A `String` is allocated on every call, and twice on cold keys · **High** · inferred
- *Loc:* `crates/uniflight/src/lib.rs:361` (`let owned_key = key.to_owned();`), `crates/uniflight/src/lib.rs:409` (`match
  map.entry(key.to_owned())` inside `insert_or_get_existing`). Contradicted documentation at `crates/uniflight/src/lib.rs:41-56` and
  the `execute` doc comment at `:330-349`.
- *Issue:* `execute` takes `key: &Q` where `Q: ToOwned<Owned = K>`, i.e. `&str` for a `String`-keyed merger — a signature that
  promises borrowed lookup. …
- *Impact:* **High** — `docs/performance.md` states the no-allocation-on-the-hot-path rule plainly, and this is an …
- *Fix:* 1. Make the `to_owned()` at `:361` lazy — move it inside the branch that actually needs it once **U1** makes …

**OX-281 (g7-U3)** — Every follower builds and boxes a future that is immediately discarded · **High** · inferred
- *Loc:* `crates/uniflight/src/lib.rs:364-367` (the comment justifying the boxing, and `let boxed = Box::pin(func());` at `:366`,
  followed by `cell.get_or_init(boxed).await`).
- *Issue:* All N callers of a coalesced key evaluate `func()` — constructing the work future, which may itself capture and allocate
  — and then `Box::pin` it, one heap allocation each. …
- *Impact:* **High** — it scales with exactly the parameter the crate is designed to make large. …
- *Fix:* Check the cell's initialised state *before* calling `func()`. …

**OX-282 (g7-U4)** — The key is hashed two to three times per call · **Medium** · inferred
- *Loc:* `crates/uniflight/src/lib.rs:381-390` (`map.get(key)` in `get_or_create_cell`), `:409` (`map.entry(key.to_owned())`),
  `:370` (`inner.remove_if(owned_key.borrow(), …)`). · A warm call hashes the key twice: once for the `get` and once for the
  `remove_if`. … **Impact Medium:** **Medium** — `ahash` is fast, so for short keys this is … **Fix:** Compute the hash once and use
  DashMap's `_and_hash` / …

**OX-283 (g7-U5)** — A `thread_aware::Arc` is cloned per call before any work begins · **Medium** · EMPIRICAL+INFERRED
- *Loc:* `crates/uniflight/src/lib.rs:359` (`let inner = self.inner.clone();`). · The first thing `execute` does is clone
  `self.inner`, a `thread_aware::Arc<DashMap<…> … **Impact Medium:** **Medium** — 3 shared-cache-line atomic RMWs per … **Fix:**
  Since the strategy is `PerProcess`, replace …
- *Also cited:* `crates/uniflight/Cargo.toml:20-25`, `thread_aware/src/cell/mod.rs:142-150`, `factory.rs:43-52`

**OX-284 (g7-U6)** — `remove_if` performs an upgrade/drop atomic pair inside the write lock · **Medium** · inferred
- *Loc:* `crates/uniflight/src/lib.rs:370` (the closure `|_, weak| weak.upgrade().is_none()`). · The predicate passed to `remove_if`
  runs *while the shard write lock is held*. … **Impact Medium:** **Medium** — subordinate to **U1** but worth calling out …
  **Fix:** Subsumed by **U1**: if only the last holder calls …

**OX-285 (g7-U7)** — `PanicAwareCell::get_or_init` clones the result for every caller · **Low** · inferred
- *Loc:* `crates/uniflight/src/lib.rs:367` (`cell.get_or_init(boxed).await.clone()`), `crates/uniflight/src/lib.rs:477-489`
  (`PanicAwareCell::get_or_init`). · Every caller receives `T` by clone. … **Impact Low:** **Low** — this is *inherent* to …
  **Fix:** Documentation only: recommend `Arc<_>` …

**OX-286 (g7-U8)** — `catch_unwind` wrapping adds a landing pad per call · **Low** · inferred
- *Loc:* `crates/uniflight/src/lib.rs:477-489` (`PanicAwareCell::get_or_init` and the `AssertUnwindSafe` / `catch_unwind` wrapping),
  with `LeaderPanicked` defined nearby. · The leader's future is wrapped in `catch_unwind` … **Impact Low:** **Low** — landing pads
  cost … **Fix:** None. Do not remove.
- *Philosophy:* This is the inverse of a conflicting finding — removing this would violate the "preserve defensive runtime checks"
  rule. …

## Benchmark coverage gaps

Criterion benchmarks live in `crates/<crate>/benches/`; Callgrind/Gungraun
instruction-count benchmarks are the `*_cg.rs` files beside them. `docs/naming.md:81-90`
requires each Callgrind file to be named after, and paired with, its Criterion counterpart;
OX-306 records the violations of that rule, and OX-307 the census — **19 of 53 crates
have any benchmark at all, 11 have Callgrind coverage**. What follows is the per-crate gap
list as recorded by the analysing workers, condensed. Absence of a line means the crate has
no benchmark directory and none of its operations are exercised.

- **`anyspawn`** — `crates/anyspawn/benches/` contains one file, `spawner.rs`, gated on `required-features = ["tokio"]`. There is
  **no Callgrind coverage**. …
- **`anyspawn_azure`** — **Zero.** There is no `benches/` directory, no Criterion benchmark, no Callgrind benchmark. Every finding
  in this section is therefore unmeasured, and the crate sits on the Azure SDK's per-request path. …
- **`automation`** — `crates/automation/benches/` does not exist and **should not**. This is build tooling with no runtime
  consumers; every operation in it is dominated by process spawning and filesystem I/O at human …
- **`benchmarking`** — `crates/benchmarking/benches/` does not exist and **should not**. Benchmarking the benchmark harness with the
  benchmark harness is circular: the offset F36 identifies would be present in both the … *Also cited:*
  `crates/benchmarking/src/lib.rs:210-247`
- **`bytesbuf`** — Files present in `crates/bytesbuf/benches/`: · File · Kind · Pairing `buf.rs` · Criterion · paired with
  `buf_cg.rs` · · `buf_cg.rs` · Callgrind/Gungraun · paired ✅ · · `view.rs` · Criterion · paired with …
- **`bytesbuf_io`** — **`crates/bytesbuf_io/benches/` does not exist. The crate has zero benchmarks — neither Criterion nor
  Callgrind.** This is the most significant coverage gap in the group after `cachet_tier`, because the …
- **`cachet`** — Files present in `crates/cachet/benches/`: · File · Kind · Notes `operations.rs` · Criterion · `required-features =
  ["logs", "test-util"]` — see F30 · · `dynamic.rs` · Criterion · measures `DynCacheTier` … *Also cited:* `fallback.rs:124-138`
- **`cachet_memory`** — File · Kind · Groups `overhead.rs` · Criterion · `get_hit` (moka / cachet_memory), `get_miss` (moka /
  cachet_memory), `insert` (moka / cachet_memory) · **No Callgrind benchmark** (F38). …
- **`cachet_service`** — **None.** `crates/cachet_service/` has no `benches/` directory and no `[[bench]]` sections in `Cargo.toml`
  (F49). …
- **`cachet_tier`** — **None.** `crates/cachet_tier/` has no `benches/` directory, no Criterion benchmarks and no Callgrind
  benchmarks (F44). …
- **`data_privacy`** — `crates/data_privacy/benches/` does not exist. This is the group's second highest-value gap after `ohno`, and
  arguably the more urgent of the two on a frequency basis, since redaction runs on the success …
- **`data_privacy_core`** — `crates/data_privacy_core/benches/` does not exist. One benchmark is warranted: **`DataClass` hashing
  and equality in isolation**, at short and long name lengths. …
- **`data_privacy_macros`** — `crates/data_privacy_macros/benches/` does not exist and should not. There is no runtime code to
  benchmark — every function in the crate is a proc-macro entry point that runs in `rustc`.
- **`data_privacy_macros_impl`** — `crates/data_privacy_macros_impl/benches/` does not exist and should not — see the `ohno_macros`
  reasoning; proc-macro cost is compile time and Criterion is the wrong instrument. …
- **`fetch`** — `fetch` has two Criterion benchmark files, `benches/http_crate.rs` and `benches/pipelines.rs`, both of which use
  `alloc_tracker` — good, and unusual for this workspace. … *Also cited:* `benches/pipelines.rs:39`, `benches/http_crate.rs:25`
- **`fetch_azure`** — **Zero benchmarks.** This crate is a thin adapter, but F19, F20 and F21 are all per-request/per-response costs
  that a small `benches/adapter.rs` with `alloc_tracker` would make visible immediately, without …
- **`fetch_hyper`** — **Zero benchmarks.** No `benches/` directory, no Criterion, no Callgrind. This is the crate that performs the
  actual HTTP transport work, so the absence is notable. …
- **`fetch_options`** — **Zero benchmarks.** Mostly appropriate for a configuration crate, with one exception:
  `PoolSelectionStrategy::select` is a genuine per-request algorithm and deserves its own microbenchmark — a Criterion …
- **`fetch_tls`** — **Zero benchmarks.** Given that the crate is configuration-only and its work is per-client, this is defensible
  and I would not recommend adding Criterion coverage here ahead of any other crate in the group. …
- **`fetch_winhttp`** — No benchmarks, correctly — there is nothing to benchmark. When the WinHTTP transport is actually
  implemented, it should be held to the same bar as `fetch_hyper`: a loopback throughput benchmark and a …
- **`fundle`** — `crates/fundle/benches/` does not exist, and there is nothing here to benchmark — the crate has no runtime logic of
  its own. …
- **`fundle_macros`** — `crates/fundle_macros/benches/` does not exist and should not — proc-macro entry points have no runtime
  code.
- **`fundle_macros_impl`** — `crates/fundle_macros_impl/benches/` does not exist and should not — proc-macro cost is compile time.
  The generated code's costs (F29, F30) are construction-path and deprioritised. …
- **`http_extensions`** — File · Callgrind twin · Covers `benches/router_resolve.rs` · `benches/router_resolve_cg.rs` ✅ ·
  `Router::resolve_request_uri` · · `benches/http_request_builder.rs` · ❌ none · `HttpRequestBuilder::build` · …
- **`http_path_template`** — **The model for the workspace.** `benches/hpt_parse.rs` and `benches/hpt_parse_cg.rs` are correctly
  paired per `docs/naming.md`, the Criterion group name (`hpt_parse/parse`) is prefixed by the file …
- **`internity`** — **What exists.** · Target · Kind · Contents `benches/internity_compare.rs` · Criterion · Groups
  `internity_compare/insert`, `/reuse`, `/lookup`, `/insert-concurrent`, `/reuse-concurrent` … *Also cited:*
  `crates/internity/benches/internity_compare.rs:520-557`, `crates/internity/src/shard.rs:107-110`
- **`internity_macros`** — None, and none is warranted. There is no runtime code here and no generated code — the shim only forwards
  `TokenStream`s. …
- **`internity_macros_impl`** — **None, and none is possible with the current tooling.** Neither Criterion nor Gungraun measures
  compile-time cost, and the workspace has no compile-time budget harness. …
- **`layered`** — `crates/layered/benches/` contains three files: `dynamic.rs`, `intercept.rs` and `tower.rs`, each gated behind
  `required-features`. …
- **`multitude`** — **What is covered.** 23 files under `crates/multitude/benches/`, organised as Criterion / Callgrind (`_cg`)
  pairs per `docs/naming.md`. … *Also cited:* `chunk_provider.rs:139-141`, `criterion_arena_vs_allocator.rs:96,102`,
  `multitude_teardown/shared.rs:47`, `multitude_alloc_common/mod.rs:55,63`, `docs/performance.md:29`,
  `docs/callgrind-benchmarks.md:10,153`
- **`multitude_macros`** — There is no `benches/` directory and none is warranted: the crate contains no runtime code. The
  meaningful measurement for a proc-macro crate is compile time, for which the ecosystem tool is `cargo build …
- **`multitude_macros_impl`** — No `benches/` directory, and correctly so — the crate has no runtime code. Coverage of the crate's
  *output* is what matters, and that is good: the derive is exercised end-to-end by `multitude_serde` (typed … *Also cited:*
  `benches/multitude_serde.rs:22-34`, `lib.rs:715-745`, `lib.rs:1265`, `lib.rs:1851`
- **`ohno`** — `crates/ohno/benches/` does not exist. This is the highest-value benchmark gap in the group: `ohno` has 25 public
  functions, zero benchmark coverage, and sits in the return type of a large fraction of the …
- **`ohno_macros`** — `crates/ohno_macros/benches/` does not exist, and **should not**. A proc-macro crate's cost is compile time;
  Criterion measures wall-clock time of a runtime closure and cannot express "how long does …
- **`plurality`** — **What exists.** · Target · Kind · Groups / benches `benches/criterion/main.rs` · Criterion · `alloc`; `clone`
  (`arc_clone`, `rc_clone`); `dyn_box` (`plurality_box`, `infinity_pinned` … *Also cited:* `crates/plurality/src/alloced.rs:17-32`,
  `crates/plurality/src/builder.rs:18`
- **`recoverable`** — `crates/recoverable/benches/` does not exist. One benchmark is warranted, and only just:
  **`RecoveryInfo::from(io::ErrorKind)`** across a representative spread of kinds. …
- **`rest_over_grpc`** — The crate itself has **no benchmarks**. Its only coverage is the two Callgrind files in
  `rest_over_grpc_tests` (`rog_router_cg.rs`, `rog_transcode_cg.rs`), which have no paired Criterion files — a … *Also cited:*
  `docs/naming.md:81-90`
- **`rest_over_grpc_examples`** — **Zero benchmarks.** For an examples crate that is normal and not itself a defect; the point of
  F38 is that the setup code here is a ready-made foundation for the benchmarks that `rest_over_grpc` is …
- **`rest_over_grpc_tests`** — Two Callgrind files, zero Criterion files — the inverse of the recommended shape. The route table
  used by both `build.rs` and the `grs_router_vs_matchit` benchmark lives in `bench_routes.rs` at the **crate …
- **`routerama`** — **Excellent — the model for the rest of the workspace.** Every Criterion file has a matching Callgrind file,
  satisfying `docs/naming.md`'s rule that each `*_cg.rs` has a Criterion counterpart (and going …
- **`routerama_build`** — `crates/routerama_build/benches/generator_scaling.rs` (70 lines) is the only benchmark. It parameterises
  `Generator::generate()` over three route shapes (literals, captures, affixes) × three table sizes …
- **`routerama_macros`** — No benchmarks, and none are appropriate. A proc-macro shim has no measurable runtime behaviour of its
  own, and its compile-time cost is a property of `routerama_build` (covered by `generator_scaling.rs`) …
- **`seatbelt`** — **What exists.** Five Criterion files and five paired Callgrind files, declared at
  `crates/seatbelt/Cargo.toml:162-210`: · area · Criterion · Callgrind · scenarios breaker · `benches/breaker.rs` · … *Also cited:*
  `breaker_cg.rs:114-120`, `engines.rs:17`, `benches/breaker.rs:38`, `breaker_cg.rs:65`, `benches/breaker.rs:92`,
  `breaker_cg.rs:78`, `crates/seatbelt_http/src/breaker.rs:154-159`, `benches/retry.rs:80`, `benches/hedging.rs:76-77`,
  `Cargo.toml:52-55`, `Cargo.toml:130-132`, `Cargo.toml:182-185`, `Cargo.toml:340-346`
- **`seatbelt_http`** — **None.** See H6. `crates/seatbelt_http` has no `benches/` directory, no `[[bench]]` manifest entries, and
  no benchmarking dev-dependencies (`Cargo.toml:47-53`: `futures`, `http_extensions`, `layered` … *Also cited:*
  `crates/seatbelt/benches/breaker.rs:92`, `breaker_cg.rs:78`, `benches/retry.rs:80`, `benches/hedging.rs:76-77`
- **`templated_uri`** — **What exists** — six files, correctly paired `<base>.rs` / `<base>_cg.rs`, which satisfies
  `docs/naming.md:81-90` (unlike `plurality`, P6). · File · Groups · Benches `benches/hot_path.rs` · `hot_path` …
- **`templated_uri_macros`** — None, and none is warranted — there is no logic here to benchmark.
- **`templated_uri_macros_impl`** — **None exists, and Criterion/Gungraun are the wrong instruments here** — they measure runtime,
  and this crate's cost is entirely compile-time. …
- **`testing_aids`** — `crates/testing_aids/benches/` does not exist and **should not**. Benchmarking test infrastructure measures
  the test harness rather than the product, produces numbers nobody can act on, and would add a …
- **`thread_aware`** — `crates/thread_aware/benches/` contains `criterion_third_party.rs` and `gungraun_third_party/main.rs`. This
  is the **only** crate in the cluster with any Callgrind coverage — but both files benchmark the …
- **`thread_aware_macros`** — **None**, and correctly so — the crate contains no runtime code. Benchmarking a facade would measure
  nothing. …
- **`thread_aware_macros_impl`** — **None.** There is no benchmark of any kind for this crate — neither Criterion nor Callgrind —
  which is expected for a proc-macro crate, since neither harness is designed to measure expansion time. …
- **`tick`** — `crates/tick/benches/` contains exactly one file, `clock_bench.rs`. There is **no Callgrind (`*_cg.rs`) coverage at
  all** for this crate. …
- **`uniflight`** — `crates/uniflight/benches/performance.rs` is the **only** multithreaded benchmark in this entire eight-crate
  cluster, which is to its credit. …

## Workspace / build-level observations

*26 findings. Scope: Cargo profiles and `.cargo/config.toml`, `[workspace.dependencies]` and `Cargo.lock`, feature-flag
  architecture, benchmark/profiling infrastructure (`justfile`, `justfiles/*.just`, `.github/workflows/`), and workspace-structure
  effects on cross-crate inlining.*

**OX-287 (g9-F1)** — `[profile.bench]` diverges from `[profile.release]`, making benchmarks blind to missing `#[inline]` · **High**
  · EMPIRICAL+INFERRED
- *Loc:* `Cargo.toml:340-341` (`[profile.release]`), `Cargo.toml:343-346` (`[profile.bench]`)
- *Issue:* `[profile.release]` sets only `debug = "line-tables-only"`. It does not set `lto` or `codegen-units`, so Cargo's defaults
  apply: `lto = false`, `codegen-units = 16`. …
- *Impact:* High — the damaging consequence is not optimistic absolute numbers. …
- *Fix:* Either (a) make `[profile.bench]` inherit release semantics for the two settings that govern cross-crate inlining …
- *Also cited:* `Cargo.toml:343`, `Cargo.toml:345`, `Cargo.toml:346`, `docs/performance.md:18-30`, `docs/performance.md:18-23`,
  `docs/performance.md:29`, `Cargo.toml:340-346`

**OX-288 (g9-F2)** — No `[profile.dev]` and no `[profile.*.package.*]` overrides anywhere · **Low** · EMPIRICAL
- *Loc:* `Cargo.toml:340-366` (the complete set of profile tables) · The workspace defines `release` (340), `bench` … **Impact
  Low:** this is a defensible … **Fix:** Consider …

**OX-289 (g9-F3)** — `[profile.test]` sets `debug = "full"`; `[profile.mutants]` and `[profile.fuzz]` are coherent · **Low** ·
  EMPIRICAL
- *Loc:* `Cargo.toml:351-352` (`[profile.test]`, `debug = "full"`), `Cargo.toml:355-360` (`[profile.mutants]`, inherits `test`,
  `debug = "none"`), `Cargo.toml:362-366` (`[profile.fuzz]`, inherits `dev`, `opt-level = 3`, `incremental = false`, `codegen-units
  = 1`) · These are all well-chosen. … **Impact Low:** recorded as a positive. … **Fix:** None.
- *Also cited:* `Cargo.toml:351-366`

**OX-290 (g9-F4)** — `-C target-cpu=x86-64-v3` is applied to workspace builds but never reaches consumers, and covers no ARM target
  · **Medium** · EMPIRICAL+INFERRED
- *Loc:* `.cargo/config.toml:1-7` (the entire file) · The file is seven lines. It sets `rustflags = ["-C", "target-cpu=x86-64-v3"]`
  for exactly … **Impact Medium:** it does not make the workspace slower; it makes … **Fix:** Document in `docs/benchmarks.md` that
  all local and CI …

**OX-291 (g9-F5)** — Duplicate-version policy is non-blocking from both directions · **Medium** · EMPIRICAL
- *Loc:* `Cargo.toml:329` (`clippy.multiple_crate_versions = "allow"`), `deny.toml:56` (`[bans] multiple-versions = "warn"`) · The
  workspace lint table explicitly *allows* clippy's `multiple_crate_versions`, and … **Impact Medium:** duplicate versions cost
  binary size, compile … **Fix:** Consider promoting `deny.toml`'s `multiple-versions` to …

**OX-292 (g9-F6)** — Benchmarks are compiled with `--all-features`, contaminating the workspace's lowest-overhead primitives ·
  **High** · EMPIRICAL+INFERRED
- *Loc:* `justfiles/anvil/checks/bench.just:13` (recipe `anvil-bench`), `justfiles/anvil/checks/bench.just:16` (`cargo bench <scope>
  --all-features --no-run`); also `justfiles/basic.just:45,56,59` (`build`, `check`, `clippy` all use `--all-features
  --all-targets`) and `justfiles/basic.just:214-222` (`test-more`, which uses `--all-features --locked --tests --benches`)
- *Issue:* The only way benchmarks are ever compiled in this repository is with `--all-features`. That turns on every optional,
  default-off, performance-relevant feature in every crate …
- *Impact:* High — every instruction count and wall-clock number for `multitude`, `plurality`, `seatbelt`, `cachet` and …
- *Fix:* Benchmark the default feature set, not `--all-features`. The most surgical form: change the bench recipe's default …
- *Philosophy:* none — this is aligned with `docs/performance.md`'s concern that measurements reflect what users actually run. ---
- *Also cited:* `crates/multitude/src/lib.rs:487`, `crates/multitude/src/lib.rs:528`,
  `crates/multitude/src/internal/chunk_mutator.rs:94,542,564,587`, `crates/multitude/src/internal/chunk_provider.rs:23,71,79,88`,
  `crates/plurality/src/lib.rs:125,137`, `crates/plurality/src/pool.rs:28,100,217,906`, `crates/plurality/src/builder.rs:124`,
  `crates/seatbelt/src/breaker/engine/engine_telemetry.rs:38`, `crates/cachet/Cargo.toml:92,106,110,118,122,126,166,170`,
  `crates/tick/src/state.rs:13-17`, `crates/tick/src/clock.rs:17`, `crates/tick/Cargo.toml:100-102`,
  `crates/bytesbuf/Cargo.toml:72,77,87,92,97,106,110,114`, `crates/bytesbuf/src/mem/mod.rs:61-62`, `engine_telemetry.rs:38`

**OX-293 (g9-F7)** — `just bench-cg` and `just bench` do not exist; nine documentation references point at nothing · **High** ·
  EMPIRICAL
- *Loc:* recipe inventory across `justfile`, `justfiles/*.just`, `justfiles/anvil/**`; references at
  `docs/callgrind-benchmarks.md:11,153,300,322,325,328,335,430`, `docs/callgrind-benchmarks.md:302` (`just bench`),
  `docs/performance.md:29`
- *Issue:* Enumerating every recipe defined in `justfile`, every `justfiles/*.just` and every file under `justfiles/anvil/` yields
  exactly one benchmark-related recipe: `anvil-bench` …
- *Impact:* High — this is the load-bearing instruction in the workspace's own performance methodology. …
- *Fix:* Add the missing recipes (a `bench` recipe running `cargo bench` for the scope, and a `bench-cg` recipe running the …
- *Also cited:* `justfiles/anvil/checks/bench.just:13`, `docs/performance.md:18-30`, `justfiles/setup.just:53-58`

**OX-294 (g9-F8)** — Nothing installs a `#[global_allocator]` in library code, but 19 bench files do — and inconsistently ·
  **Medium** · EMPIRICAL
- *Loc:* `crates/multitude/benches/criterion_arena_vs_allocator.rs:41-42` (mimalloc), `crates/plurality/benches/graph_churn.rs:27`
  (mimalloc), plus 16 files installing `alloc_tracker::Allocator<System>` (bytesbuf ×4, `crates/cachet/benches/dynamic.rs`, fetch
  ×2, http_extensions ×2, `crates/layered/benches/dynamic.rs`, `crates/multitude/benches/multitude_record_batch.rs`, seatbelt ×5),
  and `crates/internity/benches/internity_mem.rs` (a bespoke `Tracking` allocator) · No library crate in the workspace sets a
  `#[global_allocator]` — consumers get the system … **Impact Medium:** it does not affect shipped code at all. … **Fix:** Adopt one
  allocator convention per crate and state it in …
- *Also cited:* `docs/benchmarks.md:48-49`, `docs/callgrind-benchmarks.md:277-281`

**OX-295 (g9-F9)** — `cachet` ships `futures-executor` in its runtime dependency graph for test-only use · **Medium** · EMPIRICAL
- *Loc:* `crates/cachet/Cargo.toml:59` (`futures = { workspace = true, features = ["async-await", "executor"] }`, in
  `[dependencies]`) · The `executor` feature is requested in the *runtime* dependency table, so … **Impact Medium:** compile time
  and dependency-graph surface for … **Fix:** Drop `"executor"` from the `[dependencies]` entry and add …
- *Also cited:* `crates/cachet/src/fallback.rs:14`, `crates/cachet/src/refresh.rs:325`, `crates/cachet/src/cache.rs:881`,
  `crates/cachet/src/telemetry/cache.rs:439`

**OX-296 (g9-F10)** — 36 duplicated crate names in `Cargo.lock`, including four `hashbrown` generations · **Medium** · EMPIRICAL
- *Loc:* `Cargo.lock` (541 `[[package]]` entries) · Parsing the lockfile directly (the only option offline) gives 541 packages, of
  which 36 … **Impact Medium:** binary size and compile time mostly; the … **Fix:** Track the small set that can actually be unified
  — `matchit` …

**OX-297 (g9-F11)** — `ohno` is duplicated in the graph via the published `cpulist` crate · **Low** · EMPIRICAL
- *Loc:* `Cargo.lock` — `ohno` 0.3.5 and `ohno` 0.3.9; `ohno_macros` 0.3.3 and 0.3.5 · Fourteen workspace crates depend on the …
  **Impact Low:** `ohno` is a small crate and … **Fix:** If `cpulist` is maintained by the same …

**OX-298 (g9-F12)** — Five different hashers are linked with no workspace-level policy · **Medium** · EMPIRICAL
- *Loc:* `Cargo.lock`; `[workspace.dependencies]` at `Cargo.toml:52-240` · The graph contains `ahash` (via `uniflight`),
  `rapidhash`, `xxhash-rust` and `rustc-hash` … **Impact Medium:** three hashers in one crate (`data_privacy`) is … **Fix:** Pick a
  default hasher, record the decision and its rationale …
- *Also cited:* `docs/performance.md:86-106`

**OX-299 (g9-F13)** — `moka` reaches consumers through `cachet`'s **default** `memory` feature · **Medium** · EMPIRICAL
- *Loc:* `crates/cachet/Cargo.toml` (`default = ["memory"]`), `crates/cachet_memory`'s dependency on `moka` 0.12.15; transitive set
  from `Cargo.lock` · `cachet`'s default feature set includes `memory`, which brings `cachet_memory`, which … **Impact Medium:**
  dependency-graph weight and compile time on the … **Fix:** Consider making `memory` non-default so the base `cachet` …

**OX-300 (g9-F14)** — `[workspace.dependencies]` correctly enforces `default-features = false`, and CI checks it · **Low** ·
  EMPIRICAL
- *Loc:* `Cargo.toml:44-50` (the header comment mandating it), `Cargo.toml:52-240` (the table), `.github/workflows/main.yml:186`,
  `justfiles/anvil/checks/ensure-no-default-features.just` · None — this is a positive finding worth recording … **Impact Low:** Low
  (positive) — no action. **Fix:** None. Preserve it.
- *Also cited:* `main.yml:186`

**OX-301 (g9-F15)** — Features are additive and default-off; defaults are conservative · **Low** · EMPIRICAL
- *Loc:* the `[features]` table of each of the 53 crate manifests · None structurally. Census of default feature … **Impact Low:**
  Low (positive). **Fix:** None. The one caveat is `cachet`'s …

**OX-302 (g9-F16)** — `cachet` declares a `telemetry` feature that no code references · **Low** · EMPIRICAL
- *Loc:* `crates/cachet/Cargo.toml` (`telemetry = []` in `[features]`) · `grep -rn 'feature *= *"telemetry"' … **Impact Low:** no
  runtime cost. … **Fix:** Either wire it up (there is a …

**OX-303 (g9-F17)** — No `cfg(debug_assertions)` check leaks into release builds · **Low** · EMPIRICAL
- *Loc:* `crates/bytesbuf/src/buf.rs:486,488,494` · `crates/bytesbuf/src/view.rs:90` ·
  `crates/multitude/src/internal/chunk_mutator.rs:488` · `crates/ohno/src/error_label.rs:345,352,359` · None. There are exactly
  eight … **Impact Low:** Low (positive). **Fix:** None.
- *Also cited:* `docs/performance.md:40-71`

**OX-304 (g9-F18)** — Loom marker features are inert under `--all-features` · **Low** · EMPIRICAL
- *Loc:* the `loom` feature declarations in `crates/internity/Cargo.toml`, `crates/multitude/Cargo.toml`,
  `crates/plurality/Cargo.toml` · None. These are empty marker features used in … **Impact Low:** Low (positive) — recorded to close
  … **Fix:** None.

**OX-305 (g9-F19)** — No benchmark is ever executed anywhere in CI, and benches are not even compiled in PR CI · **High** ·
  EMPIRICAL
- *Loc:* `justfiles/anvil/checks/bench.just:13,16` · `.github/workflows/main.yml` (627 lines; `testing` job at 103-145, `cargo
  nextest run --workspace --all-features` at `main.yml:128`) · `justfiles/anvil/groups/scheduled-exhaustive.just:14` ·
  `.github/workflows/anvil-scheduled-impl.yml:105-116` · `docs/callgrind-benchmarks.md:436-437`
- *Issue:* Two distinct gaps. 1. **Never executed.** The only bench recipe, `anvil-bench`, runs `cargo bench <scope> --all-features
  --no-run` (`bench.just:16`). …
- *Impact:* High — a workspace of performance-oriented primitives with no regression detection and no PR-time compile …
- *Fix:* Two cheap, independent steps, in order of value per unit of effort: (a) add `--benches` to the PR-time …
- *Also cited:* `scheduled-exhaustive.just:14`, `justfiles/basic.just:214-222`

**OX-306 (g9-F20)** — Benchmark naming and pairing conventions are violated in ways that break the documented discovery mechanism ·
  **Medium** · EMPIRICAL
- *Loc:* `docs/naming.md:17-32` (crate-prefix rule; rationale for collisions at 28-29), `docs/naming.md:41-43` (`_bench` suffix
  ban), `docs/naming.md:76-95` (Callgrind pairing rule, mandatory pairing at 81-88), `docs/callgrind-benchmarks.md:153,338` (the
  `crates/*/benches/*_cg.rs` discovery glob), `docs/callgrind-benchmarks.md:267-290` (paired-setup requirement) · Four classes of
  deviation, in descending severity: 1. … **Impact Medium:** the collision (1) and the … **Fix:** Rename to satisfy `docs/naming.md`
  — at minimum resolve the …
- *Also cited:* `docs/naming.md:28-29`, `docs/naming.md:81-88`, `docs/naming.md:17-21`, `docs/naming.md:32`

**OX-307 (g9-F21)** — Benchmark coverage census — 19 of 53 crates have benchmarks; 11 have Callgrind coverage · **Medium** ·
  EMPIRICAL
- *Loc:* `crates/*/benches/` (full recursive listing), `crates/*/Cargo.toml` `[[bench]]` tables · The census below counts bench
  *files* recursively, including directory-style bench … **Impact Medium:** coverage is respectable for the data-structure …
  **Fix:** Prioritise a first benchmark for `ohno` (error construction …
- *Also cited:* `naming.md:32`

**OX-308 (g9-F22)** — Tooling installed by `just setup` supports a recipe that does not exist; version pins are, however, consistent
  · **Low** · EMPIRICAL
- *Loc:* `justfiles/setup.just:53-58` → `scripts/install-callgrind-tools.ps1`; `constants.env` (`GUNGRAUN_RUNNER_VERSION=0.19.2`);
  `Cargo.toml:112` (`gungraun = "0.19.2"`) · `just setup` installs Valgrind and … **Impact Low:** wasted setup time, and a …
  **Fix:** Fix F7; then mention the benchmark loop …

**OX-309 (g9-F23)** — `#[inline]` density census — the crates with the highest fan-in have none · **Medium** · EMPIRICAL
- *Loc:* whole-workspace grep of `pub ... fn` versus `#[inline` · Format is `crate PUB_FN / #[inline] / #[inline(always)] / src
  LOC`: | Crate | pub fn | … **Impact Medium:** Medium on its own; High in combination with F1 and F24 — … **Fix:** See F24. The
  census itself is data for the report author.
- *Also cited:* `docs/performance.md:32-34`

**OX-310 (g9-F24)** — The three highest-fan-in crates have zero `#[inline]` while the release profile leaves LTO off · **High** ·
  EMPIRICAL+INFERRED
- *Loc:* internal `[dependencies]` fan-in map (built from all 53 crate manifests); `Cargo.toml:340-341` (`[profile.release]` with no
  `lto`); `docs/performance.md:18-23` (rule 1)
- *Issue:* Internal fan-in, counting runtime `[dependencies]` only (dev-dependencies excluded), with each crate's `#[inline]` count
  from F23 in parentheses: | Crate | Internal dependents | …
- *Impact:* High — this is the workspace-level finding with the most plausible real-world cost, and it is invisible to …
- *Fix:* Two independent, both surgical: (a) Add `#[inline]` to the small public functions of `ohno`, `tick` …
- *Also cited:* `crates/tick/src/clock.rs:17`

**OX-311 (g9-F25)** — 26% of the workspace is proc-macro machinery, compiled against two `syn` generations · **Low** · EMPIRICAL
- *Loc:* the 8 proc-macro crates (`data_privacy_macros`, `fundle_macros`, `internity_macros`, `multitude_macros`, `ohno_macros`,
  `routerama_macros`, `templated_uri_macros`, `thread_aware_macros`) plus 6 `_macros_impl` crates; `Cargo.lock` (`syn` 2.0.118 and
  3.0.3, `prettyplease` 0.2.37 and 0.3.0) · 14 of 53 crates (26%) exist solely to generate … **Impact Low:** Low for runtime
  (proc-macros cost … **Fix:** Nothing at runtime. If build times are …

**OX-312 (g9-F26)** — Published crates include `benches/**` · **Low** · EMPIRICAL
- *Loc:* `Cargo.toml:32-42` (`[workspace.package].include`) · The packaging allowlist includes `/benches/**` … **Impact Low:**
  larger published artifacts … **Fix:** None required. Noted so the packaging …

## Considered and ruled out

Recorded so the reader can see the analysis was exhaustive rather than selective: these were
examined and found **not** to be problems (or to be problems whose remediation house
philosophy rejects). Condensed from the per-group `Considered and ruled out` sections.

- **`anyspawn`** — - **Atomic ordering.** No `SeqCst` in production code. - **`futures-channel` as a dependency.** It is declared
  with `default-features = false, features = ["alloc"]`, which is the minimal correct choice. …
- **`anyspawn_azure`** — - **Boxed futures in the trait implementations.** Forced by `azure_core`'s `TaskFuture = Pin<Box<dyn Future
  + Send>>` signature. …
- **`automation`** — * **`crates/automation/src/cargo_metadata.rs` parsing cost.** Read it; it deserialises `cargo metadata` output.
  …
- **`benchmarking`** — * **The `Vec::with_capacity` at `crates/benchmarking/src/lib.rs:140` allocating inside the measurement.**
  Ruled out — it is on line 140, and `measure(iterations)` is called on line 141. …
- **`bytesbuf`** — * **`nm` metric emission on the allocation path** (`mem/global.rs:171`, `:182`, `:212`).
  `crates/bytesbuf/AGENTS.md` states these metrics are low-overhead and explicitly must not be flagged. … *Also cited:*
  `mem/vec.rs:90-125`, `mem/global.rs:412`, `view.rs:1017-1042`, `view.rs:150-154`, `view_get.rs:280`, `buf.rs:496`, `buf.rs:588`,
  `view.rs:242`, `constants.rs:10-33`, `view.rs:860-936`
- **`bytesbuf_io`** — * **`#[trait_variant::make(Send)]` on `Read` (`read.rs:49`) and `ReadExt` (`read_ext.rs:16`).** This is the
  correct modern choice — RPITIT futures with no `Box` and no `dyn` — and stands in deliberate … *Also cited:*
  `read_futures.rs:57-66`, `read_futures.rs:77`, `read_ext.rs:135`, `read_ext.rs:158`, `read_ext.rs:122-133`,
  `crates/bytesbuf_io/Cargo.toml:26-29`, `Cargo.toml:31-35`
- **`cachet`** — * **`InsertPolicy::should_insert` (`crates/cachet/src/policy.rs:109-110`).** Already `#[inline]`, and it is
  `pub(crate)`. … *Also cited:* `crates/cachet/src/eviction.rs:47`, `crates/cachet/src/serialize/codec.rs:104-115`,
  `crates/cachet/src/wrapper.rs:142-147`, `telemetry/cache.rs:28`, `telemetry/cache.rs:45-53`, `fallback.rs:142`
- **`cachet_memory`** — * **Depending on `moka` rather than hand-rolling a concurrent cache.** Exactly the ecosystem-default choice
  `docs/performance.md` asks for. … *Also cited:* `tier.rs:57`, `tier.rs:211-214`, `tier.rs:216-218`, `tier.rs:197`,
  `notification.rs:27-40`
- **`cachet_service`** — * **`CacheServiceExt` as a blanket extension trait (`crates/cachet_service/src/ext.rs:24-32`).** Blanket
  impls on a trait bound are a standard, zero-cost Rust pattern; the dispatch is static. … *Also cited:* `ext.rs:15-21`,
  `adapter.rs:98-100`, `adapter.rs:70`, `ext.rs:33`, `Cargo.toml:31-33`, `request.rs:90`
- **`cachet_tier`** — * **`Error`'s `Box<dyn StdError + Send + Sync>` source (`crates/cachet_tier/src/error.rs:46-51`, `:74`,
  `:89`).** Boxing the source keeps `Error` small and moves the allocation onto the error path, which … *Also cited:*
  `error.rs:189`, `error.rs:126`, `Cargo.toml:31-32`, `tier.rs:38`, `tier.rs:58-96`, `dynamic.rs:26`, `entry.rs:133-139`
- **`data_privacy`** — * **`redacted_debug` / `redacted_display` allocating.** Ruled out empirically —
  `crates/data_privacy/src/redaction_engine.rs:85-100` uses `core::fmt::from_fn`, and the probe measured **0 allocations** for …
  *Also cited:* `crates/data_privacy/src/redaction_engine.rs:74`
- **`data_privacy_core`** — * **`Redacted` / `RedactedDisplay` / `RedactedDebug` trait dispatch.** Ruled out — these are statically
  dispatched at every call site examined; there is no vtable on the path. …
- **`data_privacy_macros`** — * **The extra crate in the graph.** Considered as a build-time cost; ruled out — it is a handful of
  forwarding functions, compiles in negligible time, and removing the split would cost testability. …
- **`data_privacy_macros_impl`** — * **`syn` feature bloat.** Ruled out and recorded as a **positive**: this crate does not enable
  `extra-traits`, unlike `ohno_macros` and `fundle_macros_impl`. …
- **`fetch`** — - **`dispatch.rs:109-131` — transport selected before the async block.** This is not a problem, it is exemplary, and
  worth recording as a positive. … *Also cited:* `dispatch.rs:115`
- **`fetch_azure`** — - **Error conversion.** Cold path; allocations there are irrelevant. - **Client construction.** Setup only. -
  **Zero `#[inline]` (1 public function).** With a single public function, an `#[inline]` census …
- **`fetch_hyper`** — - **`HyperHandler::execute` (`crates/fetch_hyper/src/connection/hyper_handler.rs:70-93`) — exemplary, no
  finding.** This is the crate's per-request hot path and it is written the way the house philosophy …
- **`fetch_options`** — - **Option struct sizes.** Several option structs are large, but they are constructed once per client and
  stored behind a shared reference; size is irrelevant here. - **`Duration` arithmetic in …
- **`fetch_tls`** — - **`TlsBackend` large enum variant (`crates/fetch_tls/src/backend.rs:19-34`).** This looks at first glance like
  a classic large-enum-variant finding, and `clippy::large_enum_variant` is indeed allowed on … *Also cited:*
  `crates/fetch_tls/Cargo.toml:38-41,51-56`
- **`fetch_winhttp`** — - **The placeholder's public surface.** Nothing is exported that a caller could route a request through, so
  there is no API-forces-slow-path finding to make. - **Feature flags.** The crate declares nothing …
- **`fundle`** — * **Re-export indirection.** Costs nothing at runtime; re-exports are resolved at name-resolution time. *
  **`fundle`'s own dependencies.** It has essentially none beyond the macro re-export. …
- **`fundle_macros`** — * **The shim/impl split.** Standard, correct, testability-motivated. Same reasoning as
  `data_privacy_macros`. …
- **`fundle_macros_impl`** — * **`#[bundle]` itself.** Ruled out and recorded as a strong **positive**: the generated `AsRef` impls
  at `crates/fundle_macros_impl/src/bundle.rs:133`, `:382`, `:462`, `:492` and `:572` return references … *Also cited:*
  `bundle.rs:382`
- **`http_extensions`** — - **`default = []` in the manifest.** Correct and commendable — consumers opt in to `json` and everything
  else. - **`Kind::Bytes(Option<BytesView>)`.** The in-memory body case is already allocation-free …
- **`http_path_template`** — - **`ParseError` design** (`crates/http_path_template/src/error.rs:55-107`). Empirically 16 bytes;
  `MaybeBacktrace` boxes only when `RUST_BACKTRACE` capture actually succeeds, so the no-backtrace case …
- **`internity`** — - **Hasher choice.** Already `rustc_hash::FxBuildHasher`, with a documented, deliberate bypass of `hash_one`'s
  terminator round. … *Also cited:* `crates/internity/src/lexicon.rs:47`
- **`internity_macros`** — - **Merging the shim into the impl crate to save a compilation unit.** This would make the expansion
  logic untestable (`proc-macro` crates cannot be linked by ordinary unit tests) and would save one very …
- **`internity_macros_impl`** — - **Generated code re-hashing field names at runtime.** It does not — all `rename` / `rename_all`
  resolution happens during expansion (`serialize.rs:96-104`) and the results are emitted as `&'static str` … *Also cited:*
  `serialize.rs:164-169`
- **`layered`** — - **Atomic ordering.** The only atomics in the crate are in tests, and they use `Relaxed`, which is correct for
  test counters. … *Also cited:* `service.rs:37-57`
- **`multitude`** — Things I specifically checked in `multitude` and found to be fine: - **No hidden allocation on any hot path.**
  Grepped all non-test `src/` for `format!`, `to_string()`, `.collect()`, `vec![` … *Also cited:*
  `internal/chunk_mutator.rs:152-186`, `arena/mod.rs:41`, `vec/mod.rs:126-209`, `internal/arena_buf.rs:142-155`,
  `internal/arena_buf.rs:205,241,303,329`, `de/containers.rs:437-452`, `de/limits.rs:412-417`, `de/json.rs:174-187`,
  `allocator_impl.rs:89-149`, `de/value/map.rs:25`, `strings/string.rs:505-509`, `strings/format_macro.rs:24-34`,
  `internal/chunk.rs:286-402`
- **`multitude_macros`** — - **The shim itself is optimal.** `derive_deserialize_in` does exactly one thing per invocation:
  `parse_quote!(::multitude::de)` to build the root path, then delegate. … *Also cited:* `lib.rs:16`
- **`multitude_macros_impl`** — - **The generated field matcher is the right shape.** `visit_str` emits a flat `match __value {
  "field_a" => Ok(Field0), ... , _ => <unknown> }` over `&str` literals (`lib.rs:562-566`, emitted at … *Also cited:*
  `lib.rs:662-680`, `lib.rs:700-708`, `lib.rs:715-745`, `attrs.rs:36-70`, `lib.rs:559-566`, `lib.rs:110-123`, `lib.rs:123`,
  `Cargo.toml:22-29`
- **`ohno`** — * **`Result<T, OhnoCore>` inflation on the success path.** Ruled out empirically: `OhnoCore` is 8 bytes,
  `Option<OhnoCore>` is 8 bytes (niche optimisation through the `Box`), and `Result<(), OhnoCore>` is … *Also cited:*
  `crates/ohno/src/core.rs:67`, `crates/ohno/src/backtrace.rs:65`, `crates/ohno/src/core.rs:192-201`
- **`ohno_macros`** — * **`proc-macro2` / `quote` dependency weight.** Ruled out — they are the universal ecosystem baseline for
  proc macros and there is no lighter alternative that is not a deviation requiring justification. …
- **`plurality`** — - **Directory double-indirection on every free-list pop** (`crates/plurality/src/pool.rs` `slot_for_global`). …
  *Also cited:* `crates/plurality/src/pool.rs:1079-1115`
- **`recoverable`** — * **`RecoveryInfo` layout.** `{ kind: RecoveryKind, delay: Option<Duration> }` at
  `crates/recoverable/src/lib.rs:107-110` — compact, `Copy`-friendly, no boxed variant, no oversized enum arm. … *Also cited:*
  `crates/recoverable/src/io.rs:58-84`
- **`rest_over_grpc`** — - **`Bytes::from(Vec<u8>)` at `crates/rest_over_grpc/src/serving.rs:262` and `:267`.** This conversion is
  O(1) — `Bytes` takes ownership of the `Vec`'s allocation without copying. … *Also cited:* `serving.rs:63-65`,
  `crates/rest_over_grpc/Cargo.toml:54-84`
- **`rest_over_grpc_examples`** — - **Example code efficiency.** Examples are optimised for clarity, and should be; nothing here is
  shipped. - **Compile time of the examples.** Out of scope. - **Whether examples should be run in CI as …
- **`rest_over_grpc_tests`** — - **Runtime performance of the test-support code itself.** It is test scaffolding; its own speed is
  irrelevant except insofar as it slows CI, which is out of scope. - **Generated service code volume.** …
- **`routerama`** — - **`HttpMethod` representation** (`crates/routerama/src/http_method.rs:81-94`).
  `HttpMethodRepr::Standard(&'static str)` avoids any allocation for the nine standard verbs; the type is 24 bytes. … *Also cited:*
  `crates/routerama/src/raw_match.rs:17,22`, `crates/routerama/src/dyn_builder.rs:37-81`,
  `crates/routerama/src/query/error.rs:28-32`, `walk.rs:51`
- **`routerama_build`** — - **Token-stream construction style.** The generator builds `proc_macro2:: TokenStream`s with `quote!`,
  which is the ecosystem-standard approach. …
- **`routerama_macros`** — - **`#[inline]` on the three entry points.** Impossible and meaningless for `#[proc_macro]` functions —
  the census's 0/3 is the right answer. - **Inlining `macro_impl` into this crate to avoid a crate …
- **`seatbelt`** — - **`EnableIf` dynamic dispatch** (`crates/seatbelt/src/utils/mod.rs:24-53`) — it is an enum with `Enabled` /
  `Disabled` / `Custom(Arc<dyn Fn>)` variants, so the default configuration costs a predictable … *Also cited:* `health.rs:110-113`,
  `engines.rs:43-46`, `crates/seatbelt/src/rnd.rs:15-22`, `crates/seatbelt/src/fallback/callbacks.rs:17-45`,
  `utils/define_fn_wrapper.rs:29,95`, `engine_core.rs:39,46`, `engines.rs:48,55`, `probing/options.rs:79`
- **`seatbelt_http`** — - **`Retry-After` header parsing on every response** — it is guarded. `ResponseExt::recovery_with_clock`
  (`crates/http_extensions/src/extensions/response_ext.rs:29-38`) only calls … *Also cited:* `http_recovery.rs:64-69`,
  `crates/seatbelt_http/src/retry.rs:143-150`, `http_clone.rs:67-73`, `breaker.rs:156`, `crates/seatbelt/Cargo.toml:48`,
  `Cargo.toml:35-38`
- **`templated_uri`** — - **`EscapedString::escape` allocating even for clean borrowed input**
  (`crates/templated_uri/src/escaped.rs:127-135`). … *Also cited:* `crates/templated_uri/src/path_and_query.rs:71-76`,
  `crates/templated_uri_macros_impl/src/struct_template.rs:167`
- **`templated_uri_macros`** — - **Merging the shim into the impl crate.** Would make expansion logic untestable. Standard ecosystem
  split. …
- **`templated_uri_macros_impl`** — - **The generated `render` / `render_into` / `render_capacity_hint` shape.** One correctly-sized
  allocation, straight-line `push_str`, compile-time capacity constant. … *Also cited:*
  `crates/templated_uri_macros_impl/src/struct_template.rs:125-128`, `crates/templated_uri_macros_impl/src/struct_template.rs:167`
- **`testing_aids`** — * **`init_tracing!` installing a global subscriber cheaply enough.** Per `docs/tracing-tests.md` this macro
  is required at module scope in every test binary that touches `tracing`. …
- **`thread_aware`** — - **Giving `Affinity` a niche to shrink `Factory`.** Tested empirically with the layout replica: adding a
  niche takes `Option<Affinity>` from 10 to 8 bytes but leaves `Factory` at 32, because the … *Also cited:*
  `crates/thread_aware/src/registry.rs:153-159`
- **`thread_aware_macros`** — - **Runtime cost** — the crate emits no runtime code of its own; all generated code originates in
  `thread_aware_macros_impl`. - **`proc-macro2`/`syn`/`quote` dependency weight** — declared by the `_impl` …
- **`thread_aware_macros_impl`** — - **`syn`/`quote`/`proc-macro2` as dependencies** — these are the universal ecosystem standard
  for derives; `docs/performance.md` asks for justification when deviating from ecosystem patterns, and there is …
- **`tick`** — - **Atomic ordering.** A grep of the crate's production code finds **no `SeqCst` anywhere** outside tests. The
  orderings in use are appropriate; nothing to tighten. - **`Timeout` future overhead.** …
- **`uniflight`** — - **Atomic ordering.** No `SeqCst` in production code; nothing to tighten. - **DashMap shard count.** The
  default (a multiple of the CPU count) is appropriate and per-key contention (**U1**) is not fixable …
- **`workspace`** — The following were investigated at workspace level and found to be **non-issues**. They are recorded so the
  report author knows they were checked rather than missed. 1. … *Also cited:* `crates/http_path_template/src/error.rs:66-80`,
  `error.rs:66-80`, `Cargo.toml:44-50`, `main.yml:186`, `Cargo.toml:112`, `Cargo.toml:340-341`,
  `crates/bytesbuf/src/mem/mod.rs:61-62`

*Citations that occur only in the group-level appendices and cross-crate notes of `docs/perf-findings/`, retained here for
  completeness:* `crates/cachet/src/fallback.rs:144`, `crates/cachet/src/telemetry/cache.rs:129-140`,
  `crates/cachet_memory/src/tier.rs:202`, `crates/cachet_service/src/adapter.rs:76`, `crates/cachet_service/src/ext.rs:39`,
  `crates/routerama_build/src/macro_impl/resolver.rs:370-371`, `engine_core.rs:51-55`, `probing/options.rs:88-92`.

