# g6-data-interning findings

Group **g6** covers seven crates: `plurality`, `internity`, `internity_macros`,
`internity_macros_impl`, `templated_uri`, `templated_uri_macros`,
`templated_uri_macros_impl`.

## Method and environment

This analysis was performed by reading source. The container has **no egress to
`index.crates.io` / `static.crates.io`** and no registry cache, so `cargo
build`/`test`/`clippy`/`bench` and every `just` recipe fail during dependency
resolution. This was re-confirmed once at the start of this round:

```
$ cargo build -p internity --offline
error: no matching package named `tokio` found
location searched: crates.io index
required by package `anyspawn v0.6.0 (.../crates/anyspawn)`
```

Consequently **every finding below is labelled `inferred from code reading`
unless stated otherwise**, and each finding names the specific benchmark that
would confirm or refute it. Three classes of claim *were* verified empirically
and are labelled as such:

1. **Type layout** — via a throwaway dependency-free program compiled with plain
   `rustc -O` containing layout-identical replicas of the types in question. The
   program was deleted afterwards and was never added to the repository.
2. **Textual censuses** — `grep`/`wc` counts over the checked-out tree.
3. **Dependency-graph shape** — computed from the checked-in `Cargo.lock`.

Layout results obtained this way (used below):

| Replica | `size_of` | `align_of` |
|---|---|---|
| `Sym` (`#[repr(transparent)] NonZeroU32`) | 4 | 4 |
| `Option<Sym>` | 4 | 4 |
| `EscapedString` = `Escaped<Cow<'static, str>>` | 24 | 8 |
| `Shard` (`#[repr(align(128))]`) | 128 | 128 |
| `[Shard; 64]` (the `ThreadedLexiconInner` payload) | **8192** | 128 |
| `SlotCell<u64>` | 16 | 8 |
| `SlotCell<[u8; 3]>` | 12 | 4 |

## Cross-cutting context carried in from round 1

Two workspace-level items reported by sibling groups affect how the numbers in
these crates should be read. They are recorded here because they change the
interpretation of every finding, but they are **not** g6 findings and are not
counted in g6's totals.

- Root `Cargo.toml:340-346`: `[profile.bench]` sets `lto = "fat"` and
  `codegen-units = 1`; `[profile.release]` sets neither. All committed benchmark
  numbers therefore come from a build configuration no downstream consumer gets,
  and fat LTO makes the benchmarks structurally **blind to missing `#[inline]`**
  — cross-crate calls are inlined by LTO in the bench build and are real calls in
  a consumer's `release` build. This matters a great deal for finding **T1**
  below (`templated_uri` has one `#[inline]` in the whole crate): the existing
  `hot_path` benchmark cannot detect the problem it has.
- Benchmarks build with `--all-features`. For `plurality` this enables the
  default-off `stats` feature. See **P7** — this concern turns out to be
  **materially smaller for `plurality` than the cross-group note implies**, and
  P7 records the correction.

## Totals

| Crate | Findings | High | Medium | Low |
|---|---|---|---|---|
| `plurality` | 6 | 0 | 2 | 4 |
| `internity` | 9 | 1 | 3 | 5 |
| `internity_macros` | 0 | 0 | 0 | 0 |
| `internity_macros_impl` | 3 | 0 | 0 | 3 |
| `templated_uri` | 8 | 1 | 3 | 4 |
| `templated_uri_macros` | 0 | 0 | 0 | 0 |
| `templated_uri_macros_impl` | 4 | 1 | 2 | 1 |
| **Total** | **30** | **3** | **10** | **17** |

---

## Crate: plurality

### Summary

`plurality` is not a collections crate — it is a **pooling memory allocator**
(`crates/plurality/Cargo.toml` description: "A highly efficient pooling memory
allocator"). It hands out four one-pointer-wide handle types (`Box`, `Alloc`,
`Arc`, `Rc`) over slots carved from power-of-two-sized chunks.

The design is unusually careful and the hot paths are already close to optimal:

- `SlotCell<T>` is `#[repr(C)]` with `value` as **field 0**
  (`crates/plurality/src/slot.rs:60-80`), so a handle is a pointer straight at
  the value and `Deref` is a no-op. The refcount is recovered by offset
  arithmetic that folds to a compile-time constant for sized `T`.
- The free list is **embedded**: a free slot's `refcount` field stores the next
  free global index, so the pool needs no side table
  (`crates/plurality/src/slot.rs`, `FREE_END = u32::MAX`).
- `grow()`, `teardown()` and `pool_full()` are correctly `#[cold]` /
  `#[inline(never)]` (`crates/plurality/src/pool.rs:851-919`, `1162-1188`).
- The `#[inline]` census is the best in the workspace: 122 `#[inline]`
  attributes across `src/` against 66 public functions. Verified by grep:
  `pool.rs` 51, `common.rs` 26, `rc.rs` 13, `boxed.rs` 11, `sync.rs` 7,
  `slot.rs` 6, `alloced.rs` 4, `chunk.rs` 2, `coerce.rs` 2.
- `Rc` uses non-atomic refcount reads/writes through the `atomic.rs` shim
  (sound because `Rc` is `!Send`), with `loom` types swapped in under `cfg(loom)`
  — exactly the right structure.

The findings below are therefore mostly second-order. The single most valuable
observation is not a code defect at all but a **coverage hole**: the crate's only
concurrency — cross-thread frees pushing onto the MPSC free list — has **no
benchmark of any kind** (P5).

### Findings

#### P1. Immutable pool geometry shares cache lines with the two hottest atomics

- **Location:** `crates/plurality/src/pool.rs:37-44` (`PoolCore`) and
  `crates/plurality/src/pool.rs:82-110` (`PoolInner`), used from
  `crates/plurality/src/pool.rs:807-825` (`alloc_slot`) and
  `crates/plurality/src/pool.rs:967-985` (`push_free`).
- **Issue:** `PoolInner<T, A>` is `#[repr(C)]` with `PoolCore` first — the
  source comment at `pool.rs:80-81` explains why (the type-erased `teardown`
  callback casts the `PoolCore` pointer back to the concrete `PoolInner`), so
  the ordering is load-bearing, not accidental.

  Walking the `#[repr(C)]` layout on a 64-bit target: `free_head: AtomicU32`
  at offset 0, four bytes of padding, `pool_refcount: AtomicUsize` at 8,
  `teardown: unsafe fn` at 16 — `PoolCore` ends at 24. Then `chunk_size` at 24,
  `shift` at 28, `mask` at 32, `max_chunks: Option<u32>` at 36, and
  `chunks_allocated: AtomicU32` at 44. **Every one of those falls inside the
  same 64-byte cache line as `free_head` and `pool_refcount`.**

  `chunk_size`, `shift`, `mask` and `max_chunks` are immutable after
  construction and are read by `slot_for_global` on **every single
  allocation**.

  `free_head` and `pool_refcount` are read-modify-written by *other* threads:
  a cross-thread `Arc::drop` performs `push_free` (a CAS loop on `free_head`)
  and decrements `pool_refcount`. Every such drop takes that cache line into
  M-state on the dropping core, which invalidates it on the allocator thread —
  the thread that must read the geometry constants and the directory pointer to
  service the *next* allocation. The geometry fields are pure read-only data
  that never need to be invalidated; they are being dragged along by false
  sharing with the two contended atomics.
- **Impact:** Medium — only in the producer/consumer pattern the crate is
  explicitly designed for (allocate on one thread, `Arc` handles dropped on
  others). In the single-threaded case there is no effect at all. On x86 the
  cost is a cache-line ping-pong per cross-thread drop burst; on a
  multi-socket or big.LITTLE ARM machine it is worse.
- **Remediation:** Surgical. Either (a) reorder `PoolInner` so the read-only
  geometry and `directory` fields precede `PoolCore` rather than following it,
  or (b) give the contended atomics their own alignment, mirroring what
  `internity` already does for the same reason
  (`crates/internity/src/shard.rs`, `#[repr(align(128))] struct Shard`). Option
  (a) is preferable: it is a field reorder with no `repr` change and no new
  padding for the single-threaded case, though `PoolCore` must stay at a fixed
  known offset for the type-erased `teardown` path, so option (b) may be the
  only one that preserves that invariant. The existing `#[repr(align(128))]`
  precedent in `internity` means (b) is not a deviation from house practice.
- **Evidence:** inferred from code reading. **Confirming benchmark:** none
  exists — this needs a new multithreaded Criterion benchmark in
  `crates/plurality/benches/criterion/main.rs` in the shape of
  `internity_compare.rs`'s `bench_reuse_concurrent`
  (`crates/internity/benches/internity_compare.rs:520-557`), using
  `bench_on_threadpool()` per `docs/benchmarks.md`: one thread allocating
  `Arc<T>` in a loop while 1/3/7 other threads drop them. See P5.

#### P2. `Arc`/`Rc` clone and drop pay a metadata load before they can find the refcount, for unsized `T`

- **Location:** `crates/plurality/src/pool.rs:1125-1140` (`refcount_ptr`);
  callers in `crates/plurality/src/sync.rs` (`Arc::clone`, `Arc::drop`) and
  `crates/plurality/src/rc.rs`.
- **Issue:** Because `SlotCell` puts the value at field 0 and the refcount
  *after* it, `refcount_ptr` must compute
  `round_up(size_of_val(value.as_ref()), align_of::<AtomicU32>())` to locate the
  refcount. For sized `T` this folds to a constant and costs nothing. For
  `T: ?Sized` — which the crate explicitly supports via its `Coercion` tokens
  (`crates/plurality/src/coerce.rs`) — `size_of_val` is a **vtable load** (for
  `dyn Trait`) or a fat-pointer metadata read (for slices). So every
  `Arc<dyn Trait>::clone` and every `Arc<dyn Trait>::drop` executes a dependent
  load *before* it can even issue the refcount RMW, lengthening the critical
  path. `std::sync::Arc` has no equivalent dependency: it places the refcounts
  *before* the value, at a fixed negative offset from the value pointer.
- **Impact:** Medium-Low — zero cost for sized `T` (the common case and the one
  the benchmarks cover), real for the `dyn Box` / unsized story the crate
  advertises and benchmarks in the `dyn_box` group.
- **Remediation:** There is no surgical fix. Putting the refcount *before* the
  value would invert the layout the entire handle design rests on. The
  practical response is to (a) document the asymmetry on `Coercion` /
  `unsize()`, and (b) add the missing benchmark (P5) so the size of the effect
  is known before anyone contemplates the redesign.
- **Evidence:** inferred from code reading; the `SlotCell` layout that forces it
  was verified empirically (`SlotCell<u64>` = 16 bytes, value at offset 0).
  **Confirming benchmark:** a Gungraun instruction-count benchmark comparing
  `Arc<u64>` clone+drop against `Arc<dyn Trait>` clone+drop, added to the
  existing `clone` group in `crates/plurality/benches/gungraun/linux.rs` — the
  vtable load shows up directly as extra instructions and an extra D1 miss.
- **Philosophy note:** **Conflicting.** Any real fix here is architectural, and
  `docs/performance.md` prefers surgical interventions. Recorded for
  completeness; the recommendation is explicitly *not* to act on it, only to
  measure it.

#### P3. Free-list pop uses a stronger success ordering than the protocol requires

- **Location:** `crates/plurality/src/pool.rs:807-825` (`alloc_slot`).
- **Issue:** The pop is a `load(Acquire)` followed by
  `compare_exchange_weak(head, next, AcqRel, Acquire)`. The popping thread
  publishes nothing to other threads at the point of the CAS — the slot's value
  is written *after* the pop succeeds, and the only reader of that value is the
  same thread (or a thread that synchronises through the handle being sent).
  The `Release` half of `AcqRel` therefore appears unnecessary; `Acquire` on
  both arms should suffice. On x86-64 this is free (both compile to `lock
  cmpxchg`); on aarch64 `AcqRel` forces `stlxr` where `stxr` would do.
- **Impact:** Low — architecture-specific, one instruction variant, and only on
  the allocation path.
- **Remediation:** Change the success ordering to `Acquire`. **This must not be
  done on reasoning alone.** The crate already carries `loom` support
  (`crates/plurality/src/atomic.rs`); the change should be gated on the existing
  loom model checks passing, and on a reviewer independently confirming the
  free-list protocol (`push_free` at `crates/plurality/src/pool.rs:967-985` and
  the splice path) does not rely on the release edge.
- **Evidence:** inferred from code reading. Presented as **speculative** —
  memory-ordering reasoning without a model checker run is not evidence.
  **Confirming benchmark:** `crates/plurality/benches/gungraun/linux.rs`, `alloc`
  group, on an aarch64 runner; instruction counts would be unchanged but the
  emitted instruction *kind* differs, so a disassembly diff is the more
  appropriate check.

#### P4. Cheap public accessors are not `#[inline]`

- **Location:** `crates/plurality/src/pool.rs:180-262` — `chunk_size`,
  `max_chunks`, `max_capacity`, `chunks_allocated`, `len`, `capacity`,
  `available`, `is_empty`, `stats`.
- **Issue:** These are one-or-two-instruction field reads that cross the crate
  boundary and carry no `#[inline]`.
- **Impact:** Low — and arguably **not a defect at all**. Every one of them
  lives in `impl<T, A: Allocator> Pool<T, A>`, so they are *generic* functions,
  their MIR is exported to downstream crates regardless, and the compiler can
  inline them at the instantiation site without the attribute.
  `docs/performance.md` rule 2 says generic functions get `#[inline]` **only
  with measurement**.
- **Remediation:** No action recommended. Recorded so that a future reviewer
  running an `#[inline]` census does not "fix" this and regress compile time
  for nothing. If someone does want to act, rule 2 requires
  `just package=plurality bench-cg` before and after.
- **Evidence:** empirically verified (grep census of `#[inline]` per file, and
  reading the enclosing `impl` headers).

#### P5. The crate's only concurrency has no benchmark

- **Location:** `crates/plurality/benches/` (whole directory);
  the unmeasured code is `crates/plurality/src/pool.rs:967-985` (`push_free`)
  and the cross-thread `Arc::drop` path in `crates/plurality/src/sync.rs`.
- **Issue:** `plurality`'s central design claim is that the pool is `Send +
  !Sync` — allocation happens on one thread, but `Arc` handles may be dropped
  on **any** thread, and those frees are pushed onto a lock-free MPSC free list.
  Every benchmark in the crate is single-threaded. The CAS loop in `push_free`,
  the `pool_refcount` decrement, and the cache-line behaviour described in P1
  are all completely unmeasured. A contention regression in the crate's most
  delicate code would be invisible to CI.
- **Impact:** Medium — this is the highest-value gap in the crate. Every other
  finding here is blocked on it.
- **Remediation:** Add a multithreaded Criterion benchmark using
  `bench_on_threadpool()` (mandated by `docs/benchmarks.md` for multithreaded
  benches). The natural model is
  `crates/internity/benches/internity_compare.rs:520-557`. Scenarios: one
  allocator thread plus 1/3/7 dropper threads; and an all-`Alloc` control (which
  never touches `pool_refcount`) to isolate the refcount cost from the free-list
  cost.
- **Evidence:** empirically verified — enumeration of all four benchmark targets
  in `crates/plurality/benches/`, none of which spawns a thread.

#### P6. Benchmark layout does not satisfy the mandatory Callgrind/Criterion pairing rule

- **Location:** `crates/plurality/benches/gungraun/main.rs`,
  `crates/plurality/benches/gungraun/linux.rs`,
  `crates/plurality/benches/pool_comparison/main.rs`,
  `crates/plurality/benches/pool_comparison/linux.rs`,
  `crates/plurality/benches/criterion/main.rs`,
  `crates/plurality/benches/graph_churn.rs`; rule at `docs/naming.md:81-90`.
- **Issue:** Three separate deviations.
  1. `docs/naming.md` requires Callgrind benches to be named `<base>_cg.rs`
     **and** to have a paired Criterion `<base>.rs` in the same directory.
     `plurality`'s Callgrind benches are *directories* (`gungraun/`,
     `pool_comparison/`) registered as `[[bench]]` targets, so the pairing rule
     is structurally unsatisfiable for them. `pool_comparison` has no Criterion
     counterpart at all, so the competitor comparison is instruction-counts-only
     with no wall-clock cross-check.
  2. Criterion group names are `alloc`, `clone`, `dyn_box` — flat, not
     `<file-basename>/`-prefixed as `docs/benchmarks.md` requires. The file
     basename here would be `criterion`, which is itself a sign the target
     naming has drifted.
  3. `crates/plurality/benches/graph_churn.rs` is a bare `fn main()` custom
     harness comparing the pool against mimalloc. It produces numbers no
     regression tracker can consume.
- **Impact:** Low for runtime performance; Medium for the ability to *detect*
  performance changes, which is what these documents exist to protect.
- **Remediation:** Either bring the layout into line with `docs/naming.md`
  (rename to `gungraun_cg.rs` / `pool_comparison_cg.rs` with paired Criterion
  files, prefix the group names), or amend `docs/naming.md` to bless the
  directory-per-bench-target form that `plurality` actually uses. Doing neither
  leaves a documented rule that one of the workspace's most
  performance-sensitive crates silently violates.
- **Evidence:** empirically verified (directory listing and grep of
  `benchmark_group` / `library_benchmark_group!` names, cross-read against
  `docs/naming.md`).

#### P7. Correction: the `stats` feature does **not** contaminate `plurality`'s steady-state benchmark numbers

- **Location:** `crates/plurality/src/pool.rs:907` (the only accounting site),
  inside `grow()` at `crates/plurality/src/pool.rs:851-919`;
  `crates/plurality/src/pool.rs:28, 100, 217-218`;
  `crates/plurality/src/builder.rs:124`; `crates/plurality/src/pool_stats.rs`.
- **Issue:** The cross-group round-1 note warns that because benchmarks build
  with `--all-features`, `plurality`'s instruction counts include `stats`
  counter work that no default consumer executes. Grepping every
  `#[cfg(feature = "stats")]` site shows this is **not** true of the hot path:
  the only mutation site is a single `fetch_add` at `pool.rs:907`, inside
  `grow()`, which is `#[cold] #[inline(never)]` and runs once per *chunk*
  allocation (default 32 slots per chunk,
  `crates/plurality/src/builder.rs:18`). The `alloc`, `clone` and `dyn_box`
  benchmark groups do not execute it in steady state at all.

  The one real effect is that `stats` adds an `AtomicUsize` field to
  `PoolInner` (`pool.rs:100`), which shifts the offsets of everything after it
  and therefore *changes the cache-line story described in P1* between a
  `--all-features` bench build and a default consumer build.
- **Impact:** Low — but recording it matters, because acting on the
  cross-group note as written would mean chasing a cost that is not there.
- **Remediation:** No code change. Note in `docs/benchmarks.md` that
  `plurality`'s `stats` counters are cold-path only, and be aware that P1's
  layout analysis must be redone against whichever feature set is being
  measured.
- **Evidence:** empirically verified — exhaustive grep of
  `feature = "stats"` in `crates/plurality/src/` (11 sites, listed above), each
  inspected.

### Benchmark coverage

**What exists.**
| Target | Kind | Groups / benches |
|---|---|---|
| `benches/criterion/main.rs` | Criterion | `alloc`; `clone` (`arc_clone`, `rc_clone`); `dyn_box` (`plurality_box`, `infinity_pinned`, `infinity_local_pinned`, `infinity_blind`, `infinity_local_blind`, `std_box`) |
| `benches/gungraun/main.rs` + `benches/gungraun/linux.rs` | Gungraun / Callgrind | `alloc`, `clone`, `dyn_box` — mirrors the Criterion groups |
| `benches/pool_comparison/main.rs` + `benches/pool_comparison/linux.rs` | Gungraun / Callgrind | `comparison` — vs `slab`, `sharded-slab`, `slotmap`, `object-pool`, `opool`, `deadpool`, `infinity_pool` |
| `benches/graph_churn.rs` | bare `fn main()` | pool vs mimalloc under graph-shaped churn |

The Criterion/Gungraun pairing for `alloc` / `clone` / `dyn_box` is genuinely
good: the same three scenarios are measured both in wall-clock and in
instruction counts, which is exactly the discipline
`docs/callgrind-benchmarks.md` asks for.

**What is missing.**

- **Any multithreaded benchmark at all.** See P5. This is the crate's only
  concurrency and it is entirely unmeasured: `push_free`'s CAS loop, the
  `pool_refcount` `fetch_add`/`fetch_sub` pair, and cross-thread `Arc::drop`.
- **`Alloc<'pool, T>` in Criterion.** `Alloc` is documented as the *cheapest*
  handle — it skips `pool_refcount` entirely because the `'pool` borrow proves
  liveness (`crates/plurality/src/alloced.rs:17-32`). It appears only in the
  `pool_comparison` Callgrind target, so there is no wall-clock number for the
  crate's own fastest path and no `Box`-vs-`Alloc` delta to justify the
  existence of two handle types.
- **Unsized clone/drop.** `dyn_box` benchmarks *construction* of a `dyn` box but
  not `Arc<dyn Trait>` clone/drop, which is precisely where P2's vtable load
  lands.
- **Pool growth (`grow`) and teardown.** Deliberately not benchmarked, and
  `docs/performance.md` explicitly deprioritises first-insert and teardown
  costs, so this is **correct** and is recorded here only so it is not mistaken
  for an oversight.
- **`chunk_size` sensitivity.** The default is 32 slots
  (`crates/plurality/src/builder.rs:18`) and it is rounded up to a power of two
  so the index math is a shift/mask. Nothing measures whether 32 is the right
  default; a parameterised `alloc` benchmark over 16/32/64/256 would settle it.

### Considered and ruled out

- **Directory double-indirection on every free-list pop**
  (`crates/plurality/src/pool.rs` `slot_for_global`). Popping a global index
  requires loading the directory `Vec`'s data pointer, then the chunk pointer,
  then the slot — three dependent loads before the CAS. A pointer-threaded
  Treiber stack would remove two of them, but would reintroduce the ABA problem
  that the 32-bit index encoding avoids, and would break the "free list costs
  zero extra memory" property. The deviation from the textbook pointer stack is
  justified and documented in the source. **Ruled out.**
- **Chunks are never returned to the allocator before pool teardown.** A memory
  watermark property, not a throughput one, and it is documented. Also squarely
  in the teardown category `docs/performance.md` deprioritises. **Ruled out.**
- **`free_slot_erased` reconstructing slot geometry arithmetically on every
  free** (`crates/plurality/src/pool.rs:1079-1115`). The source documents that
  this folds to constants for sized `T`, and the type-erased form is what makes
  the one-word handle possible. **Ruled out.**
- **`check_refcount_overflow` on every `Arc::clone`**
  (`crates/plurality/src/sync.rs`). A predictable compare against a constant
  with a `#[cold]` abort arm — the same defensive check `std::sync::Arc` makes.
  `docs/performance.md` requires defensive runtime checks to be preserved.
  **Ruled out, and should stay.**
- **`Arc::drop`'s `fetch_sub(Release)` + `fence(Acquire)`.** This is the
  canonical `std::sync::Arc` sequence and cannot be weakened. **Ruled out.**
- **`Rc`'s non-atomic refcount through `atomic.rs`.** Already the optimal form,
  and correctly sound via `!Send`. Noted as a positive. **Ruled out.**
- **`PoolBuilder` allocation.** Construction-path only. **Ruled out.**

---

## Crate: internity

### Summary

`internity` is a **fill-then-freeze** string/value interner. A `Sym` is a
4-byte `NonZeroU32` niche (verified: `Sym` = 4 bytes, `Option<Sym>` = 4 bytes,
so `Option<Sym>` is free). Two write engines exist:

- `LocalLexicon` (`crates/internity/src/local_lexicon.rs`) — single-threaded,
  dense indices, one `String` byte buffer plus a CSR-style `offsets: Vec<u32>`.
- `ThreadedLexicon` (`crates/internity/src/threaded_lexicon.rs`) — 64 shards,
  symbol packed as `[shard: 6 bits][local: 26 bits]`, each shard owning its own
  `Vec<u8>` buffer and offsets.

Once filled, a lexicon is `freeze()`d into an immutable `Reader`
(`FlatReader`, `ShardReader`, `ShardedReader`) that resolves without any lock.

The engineering quality is high and several things are already right:

- The `offsets` vector carries a **leading zero sentinel**, so `resolve` is
  `offsets[i]..offsets[i+1]` with no branch on `i == 0`
  (`crates/internity/src/storage.rs:50-59`).
- `unsafe` is confined to `storage.rs`, and only the internal `str_at` skips
  bounds checks; the public `resolve` is range-checked. This is the right
  split under `docs/performance.md`'s "preserve defensive runtime checks".
- The hasher is `rustc_hash::FxBuildHasher`, not SipHash — the single most
  important hashing decision in an interner, and it is already made correctly.
  Furthermore the code deliberately hashes raw bytes via
  `build_hasher()` + `write()` + `finish()` rather than `BuildHasher::hash_one`,
  to skip the `0xff` terminator round that `Hash for str` adds. This deviation
  from the ecosystem default is documented in-source, exactly as
  `docs/performance.md` requires.
- Shard selection uses a golden-ratio multiply-mix
  (`MIX = 0x9E37_79B9_7F4A_7C15`, `h.wrapping_mul(MIX) >> (64 - SHARD_BITS)`,
  `crates/internity/src/threaded_lexicon.rs:292-296`) specifically to
  decorrelate shard choice from the bits `hashbrown` uses for its control
  bytes. That is a subtle and correct piece of work.
- `hashbrown` is pulled with `inline-more` enabled.
- 49 `#[inline]` attributes across `src/`, concentrated where they belong
  (`sym.rs` 9, `local_lexicon.rs` 9, `threaded_lexicon.rs` 9, `shard.rs` 4,
  `shard_write.rs` 4, `symbol_map.rs` 3).

The one genuinely important finding is **I1**: the *repeat-intern* path — which
`docs/performance.md` says is the path that matters, since first-insert costs
are deprioritised — takes an exclusive-ish lock.

### Findings

#### I1. Repeat interns of an already-present string serialise against each other within a shard

- **Location:** `crates/internity/src/shard.rs:67-79` (`Shard::intern`) and
  `crates/internity/src/shard.rs:89-104` (`Shard::intern_bytes`); the trade-off
  is documented at `crates/internity/src/shard.rs:13-22` and restated at
  `crates/internity/src/threaded_lexicon.rs:42-45`.
- **Issue:** Both functions open with `self.state.upgradable_read()` and then
  probe the dedup table. On a **hit** — the overwhelmingly common case for an
  interner in steady state — the guard is simply dropped and the existing `Sym`
  returned; nothing is written.

  `parking_lot::RwLock` permits only **one upgradable-read guard at a time**.
  An upgradable guard excludes other upgradable guards even though it does not
  exclude plain readers. So two threads interning *different already-interned
  strings that land in the same shard* block each other, despite neither doing
  any writing. With 64 shards and `T` threads the birthday-collision rate is
  non-trivial well before `T` approaches 64, and any workload with a skewed
  string distribution (which is the normal case for identifiers, header names,
  metric labels, URIs) will concentrate on a handful of shards.

  This inverts the priority `docs/performance.md` sets out. A slow *first*
  intern is explicitly acceptable. A slow *repeat* intern is exactly what an
  interner exists to make fast.
- **Impact:** High — this is the crate's primary steady-state operation, under
  the crate's primary concurrency story, and the cost grows with thread count.
- **Remediation:** Surgical: make the hit path a **double-checked read**.
  Acquire a plain `read()` guard first and probe the dedup table; on a hit
  return immediately (plain read guards do not exclude one another, so N
  threads hitting the same shard proceed fully in parallel). Only on a miss
  drop the read guard, take `upgradable_read()`, **re-probe** (another thread
  may have inserted in the window), and upgrade to write if still absent. The
  re-probe is required for correctness and costs one extra hash-table lookup
  on the miss path only — i.e. the cost lands squarely on the first-insert
  path that the house rules say to deprioritise, which is the right place for
  it.

  The current design is *not* an oversight: the module documentation at
  `shard.rs:13-22` explains the choice as trading hit-path parallelism for a
  simpler protocol and a single lock acquisition per intern. Reopening it must
  therefore be evidence-led, not argued.
- **Evidence:** inferred from code reading, plus `parking_lot`'s documented
  single-upgradable-guard invariant. **Confirming benchmark: this one already
  exists.** `crates/internity/benches/internity_compare.rs:520-557`
  (`bench_reuse_concurrent`) runs the reuse workload at 1/2/4/8 threads against
  `lasso`-threaded, `symbol_table`, `ustr` and `string_cache`. If I1 is real,
  `internity`'s reuse throughput will scale visibly worse from 4 to 8 threads
  than the competitors that use plain read locks or lock-free reads on the hit
  path; the double-checked-read variant should close that gap while leaving the
  1-thread number unchanged. Note that round 1 reported this finding as
  unfalsifiable with existing benchmarks — **that was wrong**, and the
  correction matters because it makes this the cheapest finding in the group to
  settle.

#### I2. The serde path cannot reach `intern_bytes`, so every deserialised symbol is UTF-8-validated even on a dedup hit

- **Location:** `crates/internity/src/de/impls.rs:30-45` (`SymVisitor`) and
  `crates/internity/src/de/impls.rs:47-53` (`DeserializeIn for Sym`, which calls
  `deserializer.deserialize_str(...)`). The bypassed fast path is
  `Lexicon::intern_bytes` (`crates/internity/src/lexicon.rs:65`), implemented at
  `crates/internity/src/shard.rs:89-104`.
- **Issue:** `SymVisitor` implements only `visit_str` and `visit_string`, and
  `DeserializeIn for Sym` requests `deserialize_str`. Requesting a `str` from
  the format obliges the *format* to guarantee UTF-8, so a binary format
  (postcard, bincode, messagepack, CBOR) runs `str::from_utf8` over every
  symbol's bytes before handing them over — **including for symbols that are
  already interned**.

  `intern_bytes` exists precisely to avoid that. Its documented contract
  (`crates/internity/src/shard.rs:89-104`) is that it probes the dedup table by
  raw bytes and validates UTF-8 **only on a miss**, because a hit is
  byte-equal to a string that was already validated on its first insert. In an
  interning workload the hit rate is, by construction, close to 1 — so the
  validation being paid here is almost entirely redundant.

  The trait method is already public and already on `Lexicon`
  (`crates/internity/src/lexicon.rs:65`), so nothing structural is in the way;
  the deserialisation path simply does not use it. That the crate's bulk-load
  path — the single most likely producer of large volumes of
  already-validated bytes — is the one path that cannot reach its own
  bytes-oriented fast path is an unfortunate accident rather than a design
  decision.
- **Impact:** Medium — one redundant UTF-8 scan per symbol per document on
  binary formats, on the bulk-load path. It does not affect JSON and similar
  text formats, where the format has to produce a `str` regardless.
- **Remediation:** Surgical, and confined to one file. Add `visit_bytes` and
  `visit_borrowed_bytes` to `SymVisitor`, forwarding to
  `Lexicon::intern_bytes` and mapping its `Utf8Error` into `E` via
  `serde::de::Error::custom` (or `invalid_value`). Then dispatch on the
  format: `deserialize_str` remains correct for human-readable formats, and
  `Deserializer::is_human_readable()` is the standard hook serde provides for
  choosing `deserialize_bytes` on the binary side. No defensive check is lost —
  UTF-8 is still validated, just once per distinct string rather than once per
  occurrence.
- **Evidence:** inferred from code reading; the `SymVisitor` body and the
  `deserialize_str` call site were read in full, and `Lexicon::intern_bytes`
  confirmed present on the trait at `lexicon.rs:65`.
  **Confirming benchmark:** none exists — the entire `serde` surface
  (`SerializeIn`, `DeserializeIn`) is unbenchmarked. A new
  `benches/serde_roundtrip.rs` + `benches/serde_roundtrip_cg.rs` pair
  deserialising the existing 6000-symbol corpus from both a JSON document and a
  postcard document, with a high duplicate rate to mimic real interning
  traffic, would both demonstrate the redundant validation and measure its
  removal. The Callgrind counterpart is the more informative of the two here,
  since a UTF-8 scan of short identifiers is a small enough constant that
  wall-clock noise may swamp it.

#### I3. `intern_bytes` — a headline feature — has no benchmark at all

- **Location:** `crates/internity/benches/` (whole directory);
  the unmeasured code is `crates/internity/src/shard.rs:89-104` and
  `crates/internity/src/shard_write.rs:125-131` (`get_bytes`).
- **Issue:** `internity_compare.rs`, `internity_compare_cg.rs` and
  `internity_mem.rs` exercise only the `&str` interning API. The entire
  `intern_bytes` code path — whose *whole reason to exist* is to be faster than
  `intern` — has never been measured against `intern`, so the claim it embodies
  is unverified in CI and cannot regress detectably.
- **Impact:** Medium — a performance feature with no performance evidence.
- **Remediation:** Add `intern_bytes` variants to the existing `insert` and
  `reuse` groups in `crates/internity/benches/internity_compare.rs`, and the
  matching `insert_internity_bytes` / `reuse_internity_bytes` functions to the
  `insert` / `reuse` Gungraun groups in
  `crates/internity/benches/internity_compare_cg.rs` (naming per
  `docs/naming.md`: the Callgrind function name mirrors the Criterion id with
  `/` replaced by `_`). No competitor equivalents are needed — the meaningful
  comparison is `intern` vs `intern_bytes` within `internity`.
- **Evidence:** empirically verified — grep of `intern_bytes` across
  `crates/internity/benches/` returns nothing.

#### I4. `SymMap` / `SymSet` / `SymBuildHasher` have no benchmark

- **Location:** `crates/internity/src/symbol_map.rs` (whole file);
  `crates/internity/benches/` (absent).
- **Issue:** `symbol_map.rs` ships a bespoke `Hasher` for `Sym` keys — a
  multiply-mix over the 32-bit symbol rather than SipHash — plus `SymMap` and
  `SymSet` aliases. This is a pure performance feature: it exists only to make
  `HashMap<Sym, V>` faster than the default. It has no benchmark, so the
  benefit over `HashMap<Sym, V, FxBuildHasher>` (let alone over
  `std`'s default) is unquantified.

  Worth noting for whoever writes that benchmark: the multiply-mix places
  entropy in the **high** bits, which is what `hashbrown` wants for its 7-bit
  control tag, while the low bits used for the bucket index remain a bijection
  of the input because the multiplier is odd. The design looks right; it is
  just unmeasured.
- **Impact:** Medium — an unmeasured performance-only API surface. Downstream
  crates choosing `SymMap` are doing so on faith.
- **Remediation:** Add a `benches/sym_map.rs` + `benches/sym_map_cg.rs` pair
  (paired as `docs/naming.md:81-90` requires) with `lookup_hit`,
  `lookup_miss` and `insert` scenarios comparing `SymMap`,
  `HashMap<Sym, V, FxBuildHasher>` and `std::collections::HashMap<Sym, V>`.
- **Evidence:** empirically verified — enumeration of
  `crates/internity/benches/`.

#### I5. `ThreadedLexicon::len` / `is_empty` acquire all 64 shard locks

- **Location:** `crates/internity/src/threaded_lexicon.rs:342-345`
  (`ThreadedLexiconInner::len`), reached from `is_empty` and from the `Debug`
  impl.
- **Issue:** `len()` sums each shard's length, taking a read lock on each of the
  64 shards in turn — 64 lock acquisitions and 64 touches of 128-byte-aligned,
  almost certainly cold cache lines (8 KiB of footprint, see I6). `is_empty()`
  is defined in terms of `len()`, so asking "is this lexicon empty?" costs 64
  lock acquisitions even though a single non-empty shard would answer it.
- **Impact:** Low — neither is on a documented hot path, and neither is
  `#[inline]`. It becomes Medium if a caller ever puts `is_empty()` or a
  `tracing` field capturing `len()` inside a request loop, which is an easy
  mistake to make given how cheap the names sound.
- **Remediation:** Surgical: give `is_empty` its own implementation that
  short-circuits on the first non-empty shard. Optionally document on `len`
  that it is O(shards) and takes locks, so the cost is visible at the call
  site. Do **not** add a cached atomic counter — that would put a contended
  `fetch_add` on the intern hot path to speed up a cold query, which is
  precisely the wrong trade.
- **Evidence:** inferred from code reading. **Confirming benchmark:** none, and
  none is warranted — the fix is obviously correct and the operation should not
  be benchmarked into prominence it does not deserve.

#### I6. An empty `ThreadedLexicon` costs 8 KiB and touches 64 cache lines

- **Location:** `crates/internity/src/shard.rs` (`#[repr(align(128))] struct
  Shard`), `crates/internity/src/threaded_lexicon.rs` (`ThreadedLexiconInner`
  holding `[Shard; SHARD_COUNT]` inline, `SHARD_COUNT = 64`).
- **Issue:** The 64 shards are stored **inline** in the `Arc` payload, each
  padded to 128 bytes to avoid false sharing between adjacent shards' locks.
  A `ThreadedLexicon::new()` therefore heap-allocates at least 8192 bytes and
  the resulting object spans 128 cache lines, regardless of whether a single
  string is ever interned. Any workload that creates many small, short-lived
  threaded lexicons (per-request, per-document, per-test) pays this each time.
- **Impact:** Low — the alignment is *correct* for the crate's intended
  long-lived, heavily-contended use, and `docs/performance.md` explicitly
  deprioritises construction costs. Recorded because the number is large enough
  to surprise, and because it interacts with I5.
- **Remediation:** No change recommended to the default. If the
  many-small-lexicons pattern turns out to matter, the answer is to point those
  users at `LocalLexicon`, not to shrink the shard array — and that is a
  documentation change, not a code change.
- **Evidence:** **empirically verified** — a dependency-free `rustc -O` program
  containing a layout-identical `#[repr(align(128))] struct Shard` replica
  printed `size_of::<[Shard; 64]>() == 8192`, `align_of == 128`.
- **Philosophy note:** **Conflicting.** `docs/performance.md` deprioritises
  first-insert and construction costs; this finding is a construction cost.
  It is reported only for completeness and the recommendation is no action.

#### I7. `ThreadedLexicon::deserialize_in` clones the `Arc` on every call

- **Location:** `crates/internity/src/de/inherent.rs:105` —
  `T::deserialize_in(&mut self.clone(), ...)`.
- **Issue:** The inherent `deserialize_in(&self, ...)` cannot call the trait
  method directly because `Lexicon::intern` takes `&mut self`
  (`crates/internity/src/lexicon.rs:47`), so it materialises a temporary
  `&mut` by cloning the `Arc`. That is an atomic increment plus, on drop, an
  atomic decrement with a `Release`/`Acquire` fence — per deserialisation
  call, not per symbol.
- **Impact:** Low — amortised over an entire document's worth of symbols. On a
  many-small-messages workload (one `deserialize_in` per wire message, which is
  the realistic RPC shape) it becomes a per-message atomic RMW pair on a
  refcount that every other thread is also touching, which is the classic
  contention hotspot.
- **Remediation:** Surgical: since `ThreadedLexicon` interns through `&self`
  internally, the inherent method can call the shard machinery directly instead
  of round-tripping through the `&mut self` trait signature. Alternatively,
  hoist the clone so it happens once per deserialiser rather than once per
  call. The `&mut self` on `Lexicon::intern` is a deliberate API choice (it is
  what lets `LocalLexicon` avoid interior mutability) and should not be changed
  for this.
- **Evidence:** inferred from code reading. **Confirming benchmark:** the same
  new `benches/serde_roundtrip.rs` proposed in I2, parameterised over
  many-small-documents vs one-large-document; the effect is visible only in the
  former.

#### I8. `Reader::iter` returns a boxed trait-object iterator

- **Location:** `crates/internity/src/reader.rs` (the `iter` method on the
  `Reader` trait), implemented in
  `crates/internity/src/flat_reader.rs`,
  `crates/internity/src/shard_reader.rs`,
  `crates/internity/src/sharded_reader.rs`; consumed at
  `crates/internity/src/serde_impls.rs:42` (`collect_seq(self.iter()...)`).
- **Issue:** Returning `Box<dyn Iterator<Item = ...> + '_>` costs one heap
  allocation per call and — more importantly — makes **every `next()` an
  indirect call** that the optimiser cannot inline or unroll. The serialisation
  path goes through it for every symbol in the lexicon.
- **Impact:** Low-Medium. Low because iteration is not the crate's headline
  operation; Medium on the serialise path, where the per-element indirect call
  is paid for every symbol in a potentially very large lexicon.
- **Remediation:** The clean fix is an associated type
  (`type Iter<'a>: Iterator<...>`) on the `Reader` trait so each reader returns
  its concrete iterator, removing both the allocation and the dispatch. That is
  a public-API change to a trait, so it is more than surgical, and it would
  make `Reader` no longer object-safe — which may well be a deliberate
  requirement. A narrower alternative is to give `serde_impls.rs:42` a direct,
  non-`dyn` path over the concrete reader types.
- **Evidence:** inferred from code reading. **Confirming benchmark:** none
  exists; the proposed `benches/serde_roundtrip.rs` (I2) would cover the
  serialise direction.
- **Philosophy note:** **Partially conflicting.** The trait-level fix is
  architectural and may break object safety, which `docs/performance.md`'s
  "surgical over architectural" preference argues against. Only the narrower
  `serde_impls.rs` variant is recommended without further evidence.

#### I9. `freeze()` and the reader-construction paths are unbenchmarked

- **Location:** `crates/internity/src/threaded_lexicon.rs:364-368`
  (`build_reader`), and the `freeze()` entry points on both lexicons.
- **Issue:** The crate offers two distinct freeze paths — a consuming one that
  moves the buffers into the reader, and a `build_reader` that *copies* them so
  the lexicon stays writable. The second is O(total bytes) and allocates the
  whole corpus again. Neither is benchmarked, so the cost difference that
  should drive a caller's choice between them is undocumented and unmeasured.
- **Impact:** Low — freezing is a once-per-lifetime operation for the intended
  usage, which `docs/performance.md` deprioritises. It rises if `build_reader`
  is used repeatedly (e.g. publishing a snapshot per epoch), which the API
  invites.
- **Remediation:** Add a `freeze` group to
  `crates/internity/benches/internity_compare.rs` with `freeze_moved` and
  `freeze_copied` at a couple of corpus sizes, plus matching Gungraun functions.
  Document the O(bytes) copy on `build_reader`.
- **Evidence:** empirically verified — enumeration of benchmark groups (below).

### Benchmark coverage

**What exists.**

| Target | Kind | Contents |
|---|---|---|
| `benches/internity_compare.rs` | Criterion | Groups `internity_compare/insert`, `/reuse`, `/lookup`, `/insert-concurrent`, `/reuse-concurrent`, `/lookup-concurrent` (1/2/4/8 threads). Compares against `lasso`, `string-interner`, `symbol_table`, `ustr`, `string_cache`. Corpus of 6000 identifiers, overridable via `INTERNITY_BENCH_CORPUS_SIZE`. |
| `benches/internity_compare_cg.rs` (+ `benches/counts/linux.rs`) | Gungraun / Callgrind | Groups `insert`, `reuse`, `lookup`; functions `insert_internity`, `reuse_internity`, `insert_internity_threaded`, `reuse_internity_threaded`, `lookup_internity`, `lookup_internity_frozen`, plus competitor equivalents. |
| `benches/internity_mem.rs` | bare `fn main()` | Memory-footprint report via a tracking allocator. |

The Criterion group names are correctly `<file-basename>/`-prefixed, the
Callgrind file is correctly named `<base>_cg.rs` with a paired `<base>.rs`, and
the Callgrind function names correctly mirror the Criterion ids with `/`
replaced by `_`. This crate is the workspace's best example of
`docs/naming.md` being followed — worth citing as the reference when fixing
`plurality` (P6) and `templated_uri` (T8).

The concurrent groups deserve particular credit: `bench_reuse_concurrent`
(`crates/internity/benches/internity_compare.rs:520-557`) sweeps 1/2/4/8
threads against five competitors, which is exactly the instrument needed to
settle I1.

**What is missing.**

- **`intern_bytes`** — zero coverage. See I3.
- **`SymMap` / `SymSet` / `SymBuildHasher`** — zero coverage. See I4.
- **`get` (non-interning lookup by string)** — the `lookup` group measures
  `resolve` (symbol → string) but not the string → `Option<Sym>` direction on a
  live lexicon, which goes through `Shard::get`
  (`crates/internity/src/shard.rs:107-110`) and is the one path that already
  uses a plain shared `read()` guard. It is the natural control for I1.
- **`freeze` / `build_reader`** — zero coverage. See I9.
- **The whole `serde` surface** (`SerializeIn`, `DeserializeIn`, `SymVisitor`) —
  zero coverage. See I2 and I7.
- **`Reader::iter` / `LocalLexicon::iter`** — zero coverage. See I8.
- **`internity_mem.rs`** is a bare `fn main()`, not a Criterion target, and has
  no `_cg` pair. Memory footprint is a legitimate thing to track and arguably
  should not be forced into the Criterion shape, but as it stands it produces
  numbers nothing consumes automatically.

### Considered and ruled out

- **Hasher choice.** Already `rustc_hash::FxBuildHasher`, with a documented,
  deliberate bypass of `hash_one`'s terminator round. There is nothing to
  improve; this is the finding that *would* have been the headline in a less
  careful crate. **Ruled out — noted as a positive.**
- **Repeated hashing of the same key.** The hash is computed once per intern
  and threaded through shard selection and the dedup probe. No double hashing.
  **Ruled out.**
- **`contains_key` then `insert` double lookup.** The dedup probe uses
  `hashbrown`'s raw entry-style API, so a miss reuses the probe position for
  the insert. **Ruled out.**
- **`HashMap` / `HashSet` `DeserializeIn` impls not pre-reserving capacity.**
  This looks like a missing `with_capacity` but is a deliberate defence against
  a hostile `size_hint` causing an unbounded allocation, and it is documented as
  such. `docs/performance.md` requires defensive checks to be preserved.
  **Ruled out, and should stay.**
- **`insert_new`'s `StorageRollback` drop guard plus `core::mem::forget`.**
  Panic-safety machinery on the insert path; the `forget` on the success path
  means it costs nothing when nothing goes wrong. **Ruled out.**
- **`LocalLexicon` using a `String` buffer while `ShardWrite` uses `Vec<u8>`.**
  A cosmetic asymmetry with no performance consequence — `String` is a
  `Vec<u8>` with an invariant. **Ruled out.**
- **Missing `#[inline]` on `LocalLexicon` / `ThreadedLexicon` methods.** Almost
  all of them are generic over `S: BuildHasher`, so `docs/performance.md` rule 2
  applies and they must not be annotated without measurement. The ones that are
  hot already carry the attribute. **Ruled out.**
- **`Lexicon::intern` taking `&mut self`** (`crates/internity/src/lexicon.rs:47`).
  This forces generic callers into sequential filling and causes I7's `Arc`
  clone. It is nonetheless the right signature: it is what allows `LocalLexicon`
  to avoid interior mutability entirely, which is where the crate's
  single-threaded performance comes from. **Ruled out as an API design
  decision, with I7 recorded as its localised cost.**
- **`storage.rs`'s unchecked `str_at`.** Already the fast path, already
  correctly confined, and the public `resolve` retains its bounds check.
  **Ruled out.**
- **Feature set.** `default = ["std"]`; `std` pulls `parking_lot`; `serde` and
  `loom` are opt-in; `internity_macros` is optional. Dependencies are
  `hashbrown` (with `inline-more`), `rustc-hash`, and the optional three.
  Nothing heavy is on by default. **Ruled out — noted as a positive.**

---

## Crate: internity_macros

### Summary

**No performance issues found.**

`crates/internity_macros/src/lib.rs` is 95 lines and is a pure `proc_macro`
facade: it re-exports the derive entry points and immediately delegates to
`internity_macros_impl`. This two-crate split (thin `proc-macro = true` shim
over a normal library that can be unit-tested) is the standard,
correct pattern.

Its build-graph cost is the cost of `internity_macros_impl`, analysed in the
next section, which is minimal.

### Findings

None.

### Benchmark coverage

None, and none is warranted. There is no runtime code here and no generated
code — the shim only forwards `TokenStream`s. Compile-time cost is attributable
entirely to `internity_macros_impl`.

### Considered and ruled out

- **Merging the shim into the impl crate to save a compilation unit.** This
  would make the expansion logic untestable (`proc-macro` crates cannot be
  linked by ordinary unit tests) and would save one very small crate's
  compilation. The split is the ecosystem default. **Ruled out.**
- **`#[inline]` on the entry points.** Meaningless for `proc_macro` functions —
  they are invoked by the compiler across a dynamic library boundary.
  **Ruled out.**

---

## Crate: internity_macros_impl

### Summary

The expansion crate behind `internity_macros`: 3437 lines across `attrs.rs`
(925), `deserialize.rs` (673), `serialize.rs` (267), `shared.rs` (280),
`lib.rs` (58), `hygiene.rs` (56), `roots.rs` (28) and `tests.rs` (1150). It
generates `SerializeIn` / `DeserializeIn` implementations that thread a lexicon
through serde, supporting the serde attribute vocabulary (`rename`,
`rename_all`, `with`, `serialize_with`, `deserialize_with`, `skip*`,
`transparent`).

For a proc-macro crate, the two performance questions are (a) the compile-time
cost it imposes on every downstream consumer and (b) the quality of the code it
emits. **Both are good here**, and the contrast with
`templated_uri_macros_impl` (next section) is stark.

**Compile-time cost — empirically verified from `Cargo.lock`.** The transitive
dependency closure of `internity_macros_impl` is **four packages**:
`proc-macro2`, `quote`, `syn`, `unicode-ident`. That is the irreducible minimum
for a `syn`-based derive. Nothing else is dragged into a consumer's build graph.

**Generated-code quality.** The emitted serialiser
(`crates/internity_macros_impl/src/serialize.rs:164-169`) is a flat sequence of
`SerializeStruct::serialize_field` calls with the field count known at expansion
time, so serde's own capacity hints are correct. Field names are emitted as
string literals resolved at macro time — the `rename` / `rename_all` rules are
applied during expansion (`serialize.rs:96-104`), not at runtime, so no runtime
string manipulation survives into the generated code. The `transparent` case
(`serialize.rs:131-142`) short-circuits to a direct call on the single field
with no wrapper at all. The deserialiser threads the lexicon by reborrow rather
than by clone. All of this is what you want.

The three findings below are all **compile-time only** and all **Low**.

### Findings

#### IM1. Hygiene analysis attempts to parse every string literal in the input as a path

- **Location:** `crates/internity_macros_impl/src/hygiene.rs:18-36`
  (`collect_ident_strings`), called from
  `crates/internity_macros_impl/src/hygiene.rs:39-43` (`used_identifiers`).
- **Issue:** To guarantee generated helper names cannot collide with anything
  the user wrote, the crate walks the entire `DeriveInput` token stream. For
  every `Literal` token it runs `syn::parse2::<syn::LitStr>` and, on success,
  `syn::parse_str::<syn::Path>` on the literal's contents. Both allocate and
  build `syn` AST nodes. Most string literals in a real derive input are doc
  comments, which are long, numerous, and never valid paths — so the common
  case is "allocate, tokenise, fail".
- **Impact:** Low — bounded by the size of the derive input, executed once per
  derive site, and the *reason* for it is sound: serde routes user code through
  string-valued paths (`#[serde(with = "path::to::fn")]`), so those identifiers
  really can collide with generated names. On a type with heavy documentation
  it is nonetheless pure waste.
- **Remediation:** Surgical: skip literals that cannot be paths before invoking
  `syn`. Doc comments arrive as `#[doc = "..."]`, so skipping the contents of
  `doc` attributes — or simply rejecting any literal whose text contains a
  space or a character outside `[A-Za-z0-9_:]` before calling `parse_str` —
  removes the overwhelming majority of the failed parses with a cheap byte
  scan and no behaviour change.
- **Evidence:** inferred from code reading (the function body is quoted in full
  above from `hygiene.rs:18-36`). **Confirming benchmark:** none exists;
  proc-macro expansion cost is not benchmarked anywhere in the workspace. A
  `cargo build --timings` comparison on a crate deriving over a
  heavily-documented type, before and after, is the practical measurement.

#### IM2. Fresh-identifier generation allocates a `String` per generated name and probes linearly

- **Location:** `crates/internity_macros_impl/src/hygiene.rs:48-56`
  (`fresh_ident`); called at
  `crates/internity_macros_impl/src/serialize.rs:108`, `:112`, `:121`, `:122`,
  `:186-193` and the corresponding sites in `deserialize.rs`.
- **Issue:** Each call site builds the base name with `format!` (a `String`
  allocation) and `fresh_ident` then loops `while used.contains(&candidate) {
  candidate.push('_') }`. Up to four names are generated per field
  (`__internity_value_N`, `__internity_slot_N`, and optionally a
  `with`-seed and a `serialize_with` adapter), so a wide struct performs a few
  hundred small allocations and hash lookups.
- **Impact:** Low — a few hundred short-string allocations is nothing next to
  the cost of `syn` parsing the input in the first place, and the collision loop
  effectively never iterates.
- **Remediation:** No action recommended. Recorded for completeness. If it ever
  mattered, the fix is to reuse a single scratch `String` buffer across the
  loop, which would trade readable code for an immeasurable gain — a bad trade
  under `docs/performance.md`'s "surgical and motivated" standard.
- **Evidence:** inferred from code reading.

#### IM3. Field types and identifiers are deep-cloned into the plan structures

- **Location:** `crates/internity_macros_impl/src/serialize.rs:91-124`
  (`plans.push(NamedPlan { ident: fident, ty: field.ty.clone(), ... })`) and
  `crates/internity_macros_impl/src/serialize.rs:183-201` for the tuple case;
  the same pattern appears in `deserialize.rs`.
- **Issue:** `field.ty.clone()` deep-copies a `syn::Type`, which for a generic
  type is a non-trivial tree of allocations. It is done once per field per
  derive, purely so the plan struct can own its data rather than borrow from
  the `DeriveInput`.
- **Impact:** Low — the ownership makes the expansion code substantially
  clearer, and the cost is a small multiple of the parse cost already paid.
- **Remediation:** No action recommended. The borrow-based alternative
  (`ty: &'a syn::Type`) would propagate a lifetime through every plan struct
  and every expansion helper for no measurable gain.
- **Evidence:** inferred from code reading.

### Benchmark coverage

**None, and none is possible with the current tooling.** Neither Criterion nor
Gungraun measures compile-time cost, and the workspace has no compile-time
budget harness.

The gap that *does* matter is not a benchmark but a guard: nothing in CI would
notice if a future change added a heavyweight dependency to this crate and
inflated every consumer's build graph. That is exactly what has happened to
`templated_uri_macros_impl` (see U1–U3). A cheap, high-value control would be a
test asserting the transitive dependency count of each `*_macros_impl` crate
stays within a stated budget — `internity_macros_impl`'s current **4** is the
number to defend.

`crates/internity_macros_impl/src/tests.rs` (1150 lines) does provide good
*behavioural* coverage of the expansion, including snapshot-style checks, so a
change to the shape of the generated code would be caught. It is the
*dependency* axis that is unguarded.

### Considered and ruled out

- **Generated code re-hashing field names at runtime.** It does not — all
  `rename` / `rename_all` resolution happens during expansion
  (`serialize.rs:96-104`) and the results are emitted as `&'static str`
  literals. **Ruled out — noted as a positive.**
- **Generated code allocating per field.** It does not; the emitted body is a
  flat `SerializeStruct::serialize_field` sequence with a compile-time-known
  field count (`serialize.rs:164-169`). **Ruled out — noted as a positive.**
- **Generated `DeserializeIn` cloning the lexicon per element.** It reborrows
  (`&mut *self.interner`) rather than cloning, so element deserialisation is
  allocation-free with respect to the seed. **Ruled out — noted as a positive.**
- **`#[inline]` on generated trait methods.** They are reached through serde's
  generic machinery and are already monomorphised at the call site; adding
  `#[inline]` would be rule-2 territory with no measurement.
  **Ruled out.**
- **`darling` for attribute parsing.** This crate hand-rolls attribute parsing
  in `attrs.rs` (925 lines) rather than depending on `darling`. That is more
  code to maintain but keeps seven packages out of every consumer's build
  graph. Given the size of the serde attribute vocabulary it must support, the
  hand-rolled route is defensible and its cost is borne here rather than by
  every downstream user. **Ruled out — noted as a positive, and as the
  precedent that U2 argues `templated_uri_macros_impl` should follow.**

---

## Crate: templated_uri

### Summary

`templated_uri` performs RFC 6570-style URI template expansion: a derive on a
struct produces a `PathAndQueryTemplate` implementation, and the runtime types
(`BaseUri`, `Origin`, `BasePath`, `PathAndQuery`, `Uri`, `EscapedString`)
compose that into a request URI.

Parts of it are very well optimised. `BasePath::join_path_and_query`
(`crates/templated_uri/src/base_path.rs:66-93`) has a documented root-`/` fast
path that returns `other.clone()` — a `Bytes` refcount bump with no allocation
and no re-validation. `BasePath::join_rendered`
(`crates/templated_uri/src/base_path.rs:119-146`) renders directly into a
capacity-hinted buffer using a mark-and-drain pattern and validates once.
`first_reserved` (`crates/templated_uri/src/escaped.rs:265`) is written to
auto-vectorise. `PathAndQuery::render_capacity_hint`
(`crates/templated_uri/src/path_and_query.rs:62-67`) is a compile-time
estimate. Someone clearly did careful work on the render path.

The problem is that this work stops at the boundary of the render path. The
`Display`/`to_string` path — the one an application actually calls to turn a
`Uri` into something it can log or send — re-allocates repeatedly and goes
through `core::fmt` where `push_str` would do, even though the zero-copy
primitives it needs already exist a few lines away. And the whole crate has
**one** `#[inline]` attribute.

### Findings

#### T1. The crate has exactly one `#[inline]` attribute across ~43 public non-generic functions

- **Location:** whole of `crates/templated_uri/src/`. The single `#[inline]` is
  on `EscapedString::from_static` in
  `crates/templated_uri/src/escaped.rs`. Public non-generic `pub fn` counts per
  file: `base_uri.rs` 15, `origin.rs` 9, `uri.rs` 9, `path_and_query.rs` 5,
  `escaped.rs` 3, `base_path.rs` 2.
- **Issue:** `docs/performance.md` rule 1 says `#[inline]` should be applied to
  **non-generic exported functions on a hot path**, on knowledge alone, without
  requiring measurement — precisely because a non-generic function in another
  crate cannot be inlined at all unless the attribute (or LTO) exports its MIR.
  Almost every function here is non-generic and inherent on a concrete type:
  `Uri::path_and_query`, `Uri::base`, `Origin::scheme`, `Origin::host`,
  `Origin::port`, `BaseUri::origin`, `EscapedString::as_str`, and so on. These
  are one-instruction field reads that, in a consumer's `release` build, become
  real out-of-line calls.

  The situation is made worse by the profile split noted in the cross-cutting
  section: `[profile.bench]` sets `lto = "fat"`, so within the benchmark build
  every one of these calls *is* inlined by LTO. The crate's own
  `benches/hot_path.rs` therefore **cannot observe the problem**, and will
  report no improvement if it is fixed. The benchmark is structurally blind to
  the single largest finding about the crate it benchmarks.
- **Impact:** High — it applies to essentially every accessor on the crate's
  public surface, on the documented hot path, in the configuration real
  consumers build with, and it is a direct violation of a rule the workspace
  wrote down.
- **Remediation:** Surgical and mechanical: add `#[inline]` to the small
  non-generic public accessors on `Uri`, `BaseUri`, `Origin`, `BasePath`,
  `PathAndQuery` and `EscapedString`. Rule 1 authorises this without
  measurement. Do **not** blanket-annotate — the larger functions
  (`join_path_and_query`, `render_into`, `escape`) should be left alone, per
  `docs/performance.md`'s "be judicious".
- **Evidence:** **empirically verified** — `grep -c '#\[inline'` over every file
  in `crates/templated_uri/src/` returns 1 in `escaped.rs` and 0 everywhere
  else; `pub fn` counts obtained by grep as listed above.
- **Confirming benchmark:** `crates/templated_uri/benches/hot_path_cg.rs`
  (`build_uri`, `materialize`) — **but only if the Callgrind run is done
  against a `codegen-units=1`, non-LTO profile matching `[profile.release]`.**
  Run under the existing `[profile.bench]` it will show nothing, for the reason
  described above. This is the clearest single example in the group of why the
  profile discrepancy matters.

#### T2. `Uri::to_string` allocates twice and renders through `core::fmt`

- **Location:** `crates/templated_uri/src/uri.rs:200-211`.
- **Issue:** The `Display` implementation formats the base into a `String` (via
  `ToString::to_string`), then formats the path-and-query into a **second**
  `String` — which for the templated case runs the full `render()` — and then
  `push_str`s the second into the first. Two heap allocations and a copy, per
  URI stringification.

  The crate already owns the primitives that make this unnecessary:
  `PathAndQuery::render_into` (`crates/templated_uri/src/path_and_query.rs:53-58`)
  writes into a caller-supplied buffer, and
  `PathAndQuery::render_capacity_hint`
  (`crates/templated_uri/src/path_and_query.rs:62-67`) supplies the size. The
  mark-and-drain pattern that combines them correctly is already written down
  in this very crate at `crates/templated_uri/src/base_path.rs:125-143`.
- **Impact:** Medium — URI stringification is what happens on every outbound
  request that logs its target, and `to_string` is the obvious method for a
  caller to reach for. "Memory allocation is the root of all evil" is the first
  line of `docs/performance.md`'s hot-path section.
- **Remediation:** Surgical: implement `Display for Uri` to write the origin and
  base path directly into the `Formatter` and then call `render_into` against a
  single buffer sized by `render_capacity_hint`, following the existing
  `base_path.rs:125-143` pattern. One allocation instead of two, and no
  intermediate copy.
- **Evidence:** inferred from code reading. **Confirming benchmark:** none —
  `to_string` / `Display` is not benchmarked at all (see T8). A
  `hot_path/to_string` Criterion benchmark plus a `to_string` Gungraun function
  in `crates/templated_uri/benches/hot_path_cg.rs` would show the allocation
  count directly.

#### T3. Redacted formatting allocates a `String` per call, and the two impls are exact duplicates

- **Location:** `crates/templated_uri/src/uri.rs:214-227` (`RedactedDisplay for
  Uri`) and `crates/templated_uri/src/uri.rs:229-245` (`RedactedDebug for
  Uri`).
- **Issue:** Both implementations are, to the byte, the same eleven lines. Each
  writes the base URI into the `Formatter` with `write!(f, "{base_uri}")` — no
  allocation, good — and then calls
  `self.path_and_query.as_ref().map(|p| p.to_redacted_string(redactor))`,
  which materialises an owned `String`, purely so that the leading `/` can be
  trimmed off with `trim_start_matches('/')` before `f.write_str`.

  So one heap allocation per redacted log line. The redaction itself genuinely
  produces new text and must go somewhere, but the `RedactedDisplay` trait
  hands these functions a `Formatter` they could have redacted straight into;
  the intermediate `String` exists only because there is no
  "redact-into-a-writer" composition available, and because of the slash trim.

  To be clear about what this finding is *not*: these implementations are
  markedly better than `Uri::to_string` (T2) — they write into the `Formatter`
  rather than building and concatenating two owned strings, and only the
  redacted path-and-query is materialised.
- **Impact:** Low — one allocation per redacted format call, on a logging path.
  It matters because logging paths in a service run at request rate, but the
  cost is a single allocation, not the double allocation of T2.
- **Remediation:** Two parts, both worth doing independently.
  1. Factor the identical bodies of the two impls into one private helper
     called by both. This is a maintenance fix with no performance effect, but
     duplicated formatting code is how the two silently drift apart.
  2. To remove the allocation, `PathAndQuery` would need a
     `redacted_into(&self, redactor, &mut dyn Write)` form, and the leading-slash
     trim would need a small `Formatter` adapter that swallows a leading `/`.
     That is a new API surface for one allocation, so it should not be done
     without the measurement below.
- **Evidence:** inferred from code reading (both function bodies read in full).
  **Confirming benchmark:** none exists; the redaction surface is entirely
  unbenchmarked. Add a `hot_path/redacted_display` Criterion benchmark and a
  Gungraun counterpart — the instruction counts will show the `malloc`/`free`
  pair directly, which is the number that decides whether part 2 above is
  worth its API cost.

#### T4. Integer, `IpAddr` and `Uuid` template parameters render through `core::fmt` and allocate

- **Location:** `crates/templated_uri/src/escape.rs:33-38` (the default
  `escape_into` / `raw_into` bodies, `write!(out, "{}", self.escape())`) and
  the impls for integers / `IpAddr` / `Uuid` that do not override them.
- **Issue:** `EscapedString` overrides `escape_into` / `raw_into` to a direct
  `push_str`. Every other implementor inherits the default, which (a) calls
  `self.escape()` — allocating an `EscapedString`, i.e. a heap `String`, for
  what is a stack-renderable integer — and (b) pushes it through
  `write!`/`core::fmt`'s dynamic formatting machinery. A template such as
  `/users/{id}/posts/{post_id}` with two integer parameters therefore performs
  two heap allocations and two `core::fmt` dispatches per render, on the
  crate's documented hot path.
- **Impact:** Medium — integer path parameters are the single most common kind
  of template parameter in practice, and this is squarely a hot-path
  allocation.
- **Remediation:** Surgical: override `escape_into` / `raw_into` for the
  integer types to format into a stack buffer (`itoa`-style, or a small
  `[u8; 20]` with the standard digit loop) and `push_str` the result — no
  allocation, no `core::fmt`. `IpAddr` and `Uuid` can use the same technique
  with fixed-size stack buffers, since both have known maximum lengths.
- **Evidence:** inferred from code reading. **Confirming benchmark:**
  `crates/templated_uri/benches/hot_path.rs`'s `render` benchmark currently
  uses a `UserPostPath` sample; the effect would show up directly if the
  benchmark corpus includes integer parameters. The Gungraun `render` function
  in `hot_path_cg.rs` would show the removed `malloc` calls in the instruction
  counts.

#### T5. `Sensitive<T>` does not forward the fast render path to its inner type

- **Location:** `crates/templated_uri/src/escape.rs:128-144` (the `Escape` /
  `Raw` implementations for `Sensitive<T>`).
- **Issue:** `Sensitive<T>` implements the escape traits but inherits the
  default `escape_into` / `raw_into` from `escape.rs:33-38` rather than
  forwarding to `T`'s. So wrapping a field in `Sensitive` — which is a
  *privacy* annotation, carrying no rendering semantics of its own — silently
  drops the field off the `push_str` fast path and onto the
  allocate-then-`write!` slow path. A `Sensitive<EscapedString>` field is
  strictly slower to render than a bare `EscapedString`, for no reason a caller
  could anticipate.
- **Impact:** Medium — it is a silent, invisible performance cliff attached to a
  correctness/privacy annotation, which is the worst kind. Anyone doing the
  right thing about data privacy is penalised for it.
- **Remediation:** Surgical and small: implement `escape_into` and `raw_into`
  on `Sensitive<T>` as direct forwards to the inner `T`'s. No behaviour change
  — the redaction semantics live in the `RedactedDisplay` path, not here.
- **Evidence:** inferred from code reading. **Confirming benchmark:** none
  exists. Add a `hot_path/render_sensitive` Criterion benchmark rendering a
  template whose fields are `Sensitive<EscapedString>`, to sit alongside the
  existing `render`; the delta against `render` is the finding.

#### T6. `Origin::fmt` uses four `write!` invocations where `write_str` would do

- **Location:** `crates/templated_uri/src/origin.rs:268-277`.
- **Issue:** The `Display` implementation is a sequence of
  `write!(f, "{}", ...)` calls for values that are already `&str` (the scheme,
  the literal `"://"`, the host). Each `write!` constructs an `Arguments`
  value and dispatches through `core::fmt::Formatter::write_fmt` and the
  formatting virtual table, where `f.write_str(...)` is a direct call. Only the
  port genuinely needs integer formatting, and only when present.
- **Impact:** Low-Medium — small per call, but it sits on the `BaseUri` display
  path that `Uri::to_string` (T2) traverses, so it multiplies with T2's
  frequency.
- **Remediation:** Surgical: `f.write_str(self.scheme.as_str())?;
  f.write_str("://")?; f.write_str(host)?;` and keep `write!` only for the
  optional port.
- **Evidence:** inferred from code reading. **Confirming benchmark:** none;
  covered by the `hot_path/to_string` benchmark proposed in T2.

#### T7. `percent_encode` emits each escape byte as a `char`

- **Location:** `crates/templated_uri/src/escaped.rs:283-315`.
- **Issue:** The escape emission is `out.push('%')` followed by two
  `out.push(HEX[..] as char)`. `String::push` for a `char` runs
  `char::encode_utf8`, which involves a length computation and a branch on the
  code point's range, for characters that are provably single-byte ASCII.
- **Impact:** Low — three branches per escaped byte, and only on the
  needs-escaping path (the clean path is handled by the vectorised
  `first_reserved` scan at `crates/templated_uri/src/escaped.rs:265`).
  It becomes relevant for values that are heavily escaped, which is exactly
  what `hot_path.rs`'s `escape` benchmark measures.
- **Remediation:** Surgical: build the three bytes in a `[u8; 3]` on the stack
  and `push_str` the result via
  `str::from_utf8_unchecked` — or, staying entirely safe, keep a small
  stack array and use `core::str::from_utf8(...).expect(...)` with an
  expect-message documenting that the bytes are constructed from an ASCII hex
  table and therefore cannot fail. The latter preserves the defensive check
  `docs/performance.md` asks for at negligible cost.
- **Evidence:** inferred from code reading. **Confirming benchmark:**
  `crates/templated_uri/benches/escaped_string.rs`'s `short_encoded`,
  `long_encoded` and `boundary_heap_25` cases, plus their Gungraun
  counterparts in `escaped_string_cg.rs`, cover this directly. This is the one
  finding in the crate with fully adequate existing coverage.

#### T8. Benchmark group names are not file-prefixed, and the `Display`/redaction surface is unbenchmarked

- **Location:** `crates/templated_uri/benches/escaped_string.rs:56` (group
  `escaped_construct`), `:81` (group `request_construct`);
  `crates/templated_uri/benches/routing_rerender.rs:116` (group
  `route_materialize`), `:150` (`per_send`), `:194` (`per_send_hedged_x3`);
  rule at `docs/benchmarks.md` and `docs/naming.md`.
- **Issue:** Two things.
  1. `docs/benchmarks.md` requires Criterion group names to be prefixed with
     the benchmark file's basename. `hot_path.rs` complies (group `hot_path`,
     line 49). `escaped_string.rs` and `routing_rerender.rs` do not — their
     five groups carry names unrelated to their files. Since Criterion's
     on-disk result directory is keyed by group name, this makes it
     unnecessarily hard to attribute a regression to a file, which is the whole
     point of the rule. Note that the Callgrind counterparts *do* comply
     (`escaped_string_cg.rs:114` declares group `escaped_string`;
     `routing_rerender_cg.rs:128` declares `route_materialize`), so the
     Criterion and Callgrind names have also drifted apart from each other,
     breaking the mirroring `docs/naming.md` specifies.
  2. Nothing benchmarks `Uri::to_string` / `Display`, `RedactedDisplay`,
     `RedactedDebug`, `Origin::fmt`, `EscapedString::try_new`, `from_static`,
     or the optional `serde` impls. Findings T2, T3, T5 and T6 all land in that
     unmeasured region.
- **Impact:** Medium — for the ability to detect regressions in exactly the
  code this analysis identifies as weakest.
- **Remediation:** Rename the five groups to `escaped_string/...` and
  `routing_rerender/...` (matching the `_cg` names, per `docs/naming.md`), and
  add the `to_string` / `redacted_display` / `render_sensitive` benchmarks
  named in T2, T3 and T5.
- **Evidence:** **empirically verified** — grep of `benchmark_group` and
  `library_benchmark_group!` across `crates/templated_uri/benches/`, listed in
  the coverage section below.

### Benchmark coverage

**What exists** — six files, correctly paired `<base>.rs` / `<base>_cg.rs`,
which satisfies `docs/naming.md:81-90` (unlike `plurality`, P6).

| File | Groups | Benches |
|---|---|---|
| `benches/hot_path.rs` | `hot_path` (line 49) | `escape` (52), `escape_clean` (55), `render` (62), `to_path_and_query` (65), `build_uri` (71), `materialize` (84) |
| `benches/hot_path_cg.rs` | `hot_path` (114) | four `#[bench::sample]` functions over `sample_path()` / `(sample_path(), sample_base())` (81, 89, 97, 107) |
| `benches/routing_rerender.rs` | `route_materialize` (116), `per_send` (150), `per_send_hedged_x3` (194) | `rerender_current` (118), `reuse_cached_optimized` (122), `rerender_current_heavy` (126), `reuse_cached_optimized_heavy` (130), `double_render_current` (152), `single_render_optimized` (169), `rerender_each_attempt_current` (196), `reuse_cached_each_attempt_optimized` (212) |
| `benches/routing_rerender_cg.rs` | `route_materialize` (128) | four `#[bench::sample]` functions (100, 108, 115, 122) |
| `benches/escaped_string.rs` | `escaped_construct` (56), `request_construct` (81) | `short_clean` (58), `short_encoded` (61), `boundary_inline_24` (64), `boundary_heap_25` (67), `long_clean` (70), `long_encoded` (73), `three_short_fields` (86) |
| `benches/escaped_string_cg.rs` | `escaped_string` (114) | matching escape scenarios |

The `routing_rerender` family deserves credit: it is explicitly built to
compare a "current" implementation against an "optimized" one across
realistic scenarios (single send, hedged×3, light and heavy paths), which is
exactly the user-facing-scenario framing `docs/performance.md` asks for rather
than a raw micro-benchmark delta. The `escaped_string` boundary cases
(`boundary_inline_24` vs `boundary_heap_25`) show someone deliberately
bracketing an allocation threshold. This is good benchmark design.

**What is missing.**

- `Uri::to_string` / `Display for Uri` — the finding in T2 sits here.
- `RedactedDisplay` / `RedactedDebug` — T3. The two impls are also exact
  duplicates, so a regression introduced in one would not be caught by a test
  of the other.
- Rendering with `Sensitive<T>` fields — T5.
- Rendering with **integer** parameters — T4. Every existing sample
  (`sample_path`, `sample_heavy_path`) appears to use string-shaped fields, so
  the `core::fmt` cost on integer parameters is not exercised at all.
- `Origin::fmt` in isolation — T6 (would be covered indirectly by T2's
  benchmark).
- `EscapedString::try_new` and `from_static` — the latter is the crate's only
  `#[inline]`, and nothing measures whether that matters.
- The optional `serde` impls.
- Any benchmark run under a profile resembling `[profile.release]` — see T1.
  Without one, the crate's largest finding is invisible to its own benchmarks.

### Considered and ruled out

- **`EscapedString::escape` allocating even for clean borrowed input**
  (`crates/templated_uri/src/escaped.rs:127-135`). Because the type is
  `Escaped<Cow<'static, str>>` (verified: 24 bytes) and the input has a
  non-`'static` lifetime, a clean input still has to be copied to satisfy the
  `'static` bound. This is inherent to the type, it is documented, and
  `from_static` exists for the `'static` case. Fixing it would mean adding a
  lifetime parameter to a widely-used public type. **Ruled out as
  architectural.**
- **`PathAndQuery::template()` allocating for the `Static` arm**
  (`crates/templated_uri/src/path_and_query.rs:71-76`). Forced by the
  `Cow<'static, str>` return type. **Ruled out.**
- **`PathAndQueryInner::Templated(Arc<dyn PathAndQueryTemplate>)`** — one heap
  allocation plus atomic refcounting per templated `PathAndQuery`. Deliberate:
  it buys a cheap `clone`, which is what the `routing_rerender` scenarios show
  actually matters. It is also a construction cost, which
  `docs/performance.md` deprioritises. **Ruled out.**
- **`to_path_and_query` re-validating the rendered string.**
  `http::uri::PathAndQuery::try_from(String)` re-scans the whole path. This is
  a defensive check on data that has just been assembled from user-supplied
  parameters and it must be preserved. The generated code already minimises the
  cost by handing `http` an owned `String` it can adopt rather than a `&str` it
  would have to copy (see `crates/templated_uri_macros_impl/src/struct_template.rs:167`).
  **Ruled out, and should stay.**
- **`default = ["uuid"]`.** The default feature set pulls the `uuid` crate into
  every consumer's build even if no template has a UUID parameter. This is a
  build-time cost, not a runtime one, and `uuid` is small. Worth a note in the
  crate docs that `default-features = false` is available; not worth a change.
  **Ruled out.**
- **`Bytes`-based storage in `BasePath`.** Enables the allocation-free
  `join_path_and_query` fast path. **Ruled out — noted as a positive.**

---

## Crate: templated_uri_macros

### Summary

**No performance issues found.**

`crates/templated_uri_macros/src/lib.rs` is 35 lines: a `proc-macro = true`
shim exporting the `templated` attribute macro and the `Escape` and `Raw`
derives, each delegating immediately to `templated_uri_macros_impl`. Same
correct pattern as `internity_macros`.

Its build-graph cost, however, is not its own — it is the cost of
`templated_uri_macros_impl`, and that cost is substantial. See U1–U3.

### Findings

None attributable to this crate.

### Benchmark coverage

None, and none is warranted — there is no logic here to benchmark.

### Considered and ruled out

- **Merging the shim into the impl crate.** Would make expansion logic
  untestable. Standard ecosystem split. **Ruled out.**
- **Re-exporting from `templated_uri` instead of requiring a second dependency.**
  An ergonomics question, not a performance one. **Ruled out.**

---

## Crate: templated_uri_macros_impl

### Summary

The expansion crate behind `templated_uri_macros`: 2139 lines across
`template_parser.rs`, `struct_template.rs`, `enum_template.rs`, `uri_param.rs`,
`lib.rs` and `error.rs`.

**The code it generates is excellent.** `struct_template.rs:138-190` emits:

- `render()` as `String::with_capacity(<compile-time constant>)` followed by
  `render_into` — one allocation, correctly sized, no growth reallocations;
- `render_into` as a **flat, straight-line sequence** of `__out.push_str(<literal>)`
  and `Escape::escape_into` / `Raw::raw_into` calls, with the literal segments
  resolved at expansion time — there is no runtime template interpretation at
  all;
- `render_capacity_hint()` as a compile-time constant (literal lengths summed
  exactly, plus `ESTIMATED_VALUE_LEN = 16` per parameter);
- `to_path_and_query` reusing `render()`'s `String` so `http` can adopt the
  allocation rather than copy it (line 167);
- `RedactedDisplay` writing straight into the `Formatter` via `f.write_str`,
  with the literal segments emitted as `&'static str` — no intermediate owned
  string for the template's own text.

Generated trait methods correctly carry no `#[inline]`, since they are reached
through `dyn PathAndQueryTemplate` where it would have no effect. This is the
highest-quality code generation in the group.

**The compile-time cost it imposes is the problem**, and it is severe.

### Findings

#### U1. `chumsky` — a full parser-combinator framework — is pulled into every consumer's build graph to parse a small fixed grammar

- **Location:** `crates/templated_uri_macros_impl/Cargo.toml` (dependency
  `chumsky = { workspace = true, features = ["std"] }`);
  `crates/templated_uri_macros_impl/src/template_parser.rs:6`
  (`use chumsky::prelude::*`).
- **Issue:** The URI template grammar this crate parses is small and fixed:
  literal segments, `{name}` placeholders, and a handful of operators. It is
  parsed at **compile time only**, from string literals the developer wrote, so
  parse throughput is irrelevant — the only thing that matters is how long the
  parser itself takes to build.

  `chumsky` is a heavyweight, deeply generic parser-combinator library. From
  the checked-in `Cargo.lock`, its direct dependencies are `hashbrown`,
  `regex-automata`, `serde`, `unicode-ident` and `unicode-segmentation`; the
  full transitive closure is **15 packages**, including `aho-corasick`,
  `memchr`, `regex-syntax`, `serde_core`, `serde_derive` (itself a proc-macro,
  so it must be built and dynamically loaded before this crate can compile),
  `allocator-api2` and `foldhash`.

  Every one of those must be compiled before `templated_uri_macros_impl` can
  compile, which must complete before `templated_uri` can compile, which must
  complete before any consumer of `templated_uri` can compile. It is on the
  critical path of a serial dependency chain — it cannot be parallelised away.
  And `chumsky`'s combinator style produces very deeply nested generic types,
  which is a well-known source of slow monomorphisation and large debug builds
  independent of the package count.

  The contrast within this same workspace is stark: `internity_macros_impl`
  does comparable work with a transitive closure of **four** packages
  (`proc-macro2`, `quote`, `syn`, `unicode-ident`).
- **Impact:** High — this is compile-time cost, paid by every developer and
  every CI run of every crate that depends on `templated_uri`, on a serial
  critical path, forever. `docs/performance.md` requires deviations from
  ecosystem norms to be justified; for a proc-macro crate the ecosystem norm is
  `syn` + `quote` + `proc-macro2` and nothing else, and there is no
  in-source justification for the deviation.
- **Remediation:** Replace the `chumsky` grammar in `template_parser.rs` with a
  hand-written scanner. The grammar is small enough that this is realistically
  ~200 lines of straightforward byte-scanning code with no dependencies, and it
  would produce better error spans than a generic combinator library does.
  `crates/internity_macros_impl/src/attrs.rs` (925 lines of hand-rolled
  attribute parsing) is the in-workspace precedent, and it was clearly the
  right call there.

  This is a single-file replacement with existing tests to hold it in place, so
  despite the size of the win it is a **contained** change, not an
  architectural one.
- **Evidence:** **empirically verified** — dependency closure computed from the
  checked-in `Cargo.lock`. Caveat: `Cargo.lock` records the union of
  feature-selected dependencies, so a few of the 15 (notably `serde` and
  `serde_derive`, since `chumsky` is declared `default-features = false` at
  workspace root with only `std` enabled here) may be pruned by feature
  resolution for this particular consumer. The core of the closure —
  `regex-automata`, `regex-syntax`, `aho-corasick`, `memchr`, `hashbrown`,
  `foldhash`, `allocator-api2`, `unicode-segmentation` — is not prunable.
  **Confirming measurement:** `cargo build --timings` on a crate depending on
  `templated_uri`, reading the self-time of `chumsky` and its unique
  dependencies off the critical path.

#### U2. `darling` is pulled in for a handful of attribute fields

- **Location:** `crates/templated_uri_macros_impl/Cargo.toml` (dependency
  `darling`); used in `crates/templated_uri_macros_impl/src/struct_template.rs`
  for attribute parsing.
- **Issue:** `darling` is a derive-helper framework whose transitive closure is
  **7 packages**: `darling_core`, `darling_macro`, `ident_case`,
  `proc-macro2`, `quote`, `syn`, `unicode-ident`. Critically, `darling_macro`
  is itself a `proc-macro` crate, so building it requires a full
  compile-and-dynamically-link cycle *inside* the build of a proc-macro crate —
  proc-macros in the build graph of proc-macros are among the most expensive
  things a Cargo build graph can contain.

  The attribute surface being parsed here is small — a handful of fields on the
  `templated` attribute, nothing like the serde vocabulary that justifies
  `internity_macros_impl`'s 925-line hand-rolled `attrs.rs`.
- **Impact:** Medium — smaller than U1 but the same kind of cost, on the same
  serial critical path.
- **Remediation:** Parse the attributes directly with `syn`'s
  `Attribute::parse_nested_meta`, which is the modern ecosystem-standard
  approach and needs no additional dependency. For an attribute surface this
  small the hand-written version is likely shorter than the `darling`
  derive-plus-glue it replaces. Best done in the same change as U1, since both
  touch attribute/template parsing.
- **Evidence:** **empirically verified** — closure computed from `Cargo.lock`
  (`darling` → `darling_core`, `darling_macro`, `ident_case`, plus the syn
  triple). **Confirming measurement:** `cargo build --timings` as in U1.

#### U3. `ohno` — a runtime error framework — is a build dependency of a proc-macro crate

- **Location:** `crates/templated_uri_macros_impl/Cargo.toml` (dependency
  `ohno`); `crates/templated_uri_macros_impl/src/error.rs` (17 lines, one
  `#[ohno::error]` type).
- **Issue:** `ohno` is one of the workspace's own runtime crates. Its
  `Cargo.lock` closure includes `futures` (and its seven sub-crates),
  `chacha20`, `getrandom`, `bumpalo`, `ctor`, `libc`, `js-sys`, `bytes`,
  `linktime-proc-macro` and a good deal more. Even discounting entries that are
  `ohno`'s own dev-dependencies (as a workspace member, its `Cargo.lock` entry
  lists those too, so the raw closure figure of 88 is an overstatement), the
  genuine build-graph contribution is large — and it is being paid to define
  **one error enum, in 17 lines, that exists only to be converted into a
  `syn::Error` and reported by the compiler**.

  For scale: the transitive closure of `templated_uri_macros_impl` as a whole
  comes to 98 packages, against `internity_macros_impl`'s 4.
- **Impact:** Medium — same critical-path compile-time cost as U1 and U2, for
  the least benefit of the three. Consistency with the workspace's error
  conventions is a real argument, but those conventions exist for *runtime*
  error types that users handle programmatically; a proc-macro's internal error
  type is consumed exclusively by `syn::Error::to_compile_error`.
- **Remediation:** Replace the `#[ohno::error]` type in `error.rs` with a plain
  enum plus a `Display` impl, or use `syn::Error` throughout. This is a 17-line
  file. `internity_macros_impl` already does exactly this — it uses `syn::Error`
  and has no error-framework dependency at all.
- **Evidence:** **empirically verified** — closure computed from `Cargo.lock`,
  with the dev-dependency caveat stated above. **Confirming measurement:**
  `cargo tree -e normal -p templated_uri_macros_impl` (not runnable here
  without registry access) would give the exact non-dev figure; `cargo build
  --timings` would give the wall-clock contribution.

#### U4. Nothing guards the compile-time cost of any macro crate

- **Location:** `crates/templated_uri_macros_impl/` (no benches, no
  compile-time test); workspace-wide.
- **Issue:** U1, U2 and U3 each entered the codebase silently, because nothing
  in CI observes the shape of the build graph. A proc-macro crate's dependency
  list is a direct tax on every downstream consumer's build, and it is
  currently completely unmonitored.
- **Impact:** Low as a runtime matter; Medium as a process matter — it is the
  reason the other three findings exist and were not caught earlier.
- **Remediation:** Add a cheap test asserting that each `*_macros_impl` crate's
  **normal** (non-dev) transitive dependency count stays within a declared
  budget. `internity_macros_impl`'s current value of 4 is the standard to
  defend; `templated_uri_macros_impl` would be given a budget that ratchets
  down as U1–U3 are addressed. This is a plain assertion over
  `cargo metadata` output, not a benchmark, and costs nothing to run.
- **Evidence:** empirically verified (absence of any such test in the
  workspace).

### Benchmark coverage

**None exists, and Criterion/Gungraun are the wrong instruments here** — they
measure runtime, and this crate's cost is entirely compile-time. The correct
instruments are `cargo build --timings` and the dependency-budget assertion
proposed in U4.

The *generated* code, by contrast, **is** well covered — through
`crates/templated_uri/benches/hot_path.rs`'s `render`, `to_path_and_query` and
`materialize` benchmarks and their Gungraun counterparts, all of which execute
code emitted by this crate. So the two halves of "proc-macro performance" are
in opposite states: the emitted code is measured and excellent; the compile-time
cost is unmeasured and poor.

### Considered and ruled out

- **The generated `render` / `render_into` / `render_capacity_hint` shape.**
  One correctly-sized allocation, straight-line `push_str`, compile-time
  capacity constant. This is the ideal form. **Ruled out — noted as a
  positive.**
- **`ESTIMATED_VALUE_LEN = 16` as the per-parameter capacity estimate.** A
  heuristic, but it only affects whether the single `String` ever needs to
  grow, and it errs in a sensible direction. Tuning it would need real
  parameter-length distributions and would be worth very little. **Ruled out.**
- **`HashSet<String>` built from `.map(|p| p.name.to_owned()).collect()`**
  (`crates/templated_uri_macros_impl/src/struct_template.rs:125-128`).
  Compile-time only, bounded by the number of template parameters (single
  digits in practice). **Ruled out.**
- **No `#[inline]` on generated trait methods.** Correct — they are dispatched
  through `dyn PathAndQueryTemplate`, so the attribute would have no effect.
  **Ruled out — noted as a positive.**
- **`to_path_and_query` handing `http` an owned `String`**
  (`crates/templated_uri_macros_impl/src/struct_template.rs:167`). Lets `http`
  adopt the allocation instead of copying. **Ruled out — noted as a positive.**
- **Generated `RedactedDisplay` writing into the `Formatter`.** No intermediate
  owned string for the template's literal segments. **Ruled out — noted as a
  positive.**

---

## Appendix: the three highest-value actions in g6

For a reader who takes only three things from this document:

1. **`internity` I1** — make the intern hit path a double-checked plain
   `read()` instead of an `upgradable_read()`
   (`crates/internity/src/shard.rs:67-79`). This is the group's only High
   finding on a runtime hot path, it is on the crate's primary operation under
   its primary concurrency story, and — uniquely among the High findings — **a
   benchmark that can settle it already exists** at
   `crates/internity/benches/internity_compare.rs:520-557`. Cheapest confirmable
   win in the group.
2. **`templated_uri` T1** — add `#[inline]` to the crate's small non-generic
   public accessors. `docs/performance.md` rule 1 authorises this without
   measurement, and it applies to ~43 functions of which exactly one is
   currently annotated. Note that the crate's own benchmarks **cannot** show the
   improvement while `[profile.bench]` uses fat LTO, so do not wait for a
   benchmark to justify it.
3. **`templated_uri_macros_impl` U1–U3** — drop `chumsky`, `darling` and `ohno`
   from a proc-macro crate whose peer, `internity_macros_impl`, does comparable
   work with four transitive dependencies. This costs every developer and every
   CI run on a serial critical path, and U1 in particular is a single-file
   replacement backed by existing tests.
