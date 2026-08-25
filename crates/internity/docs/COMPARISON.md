# Rust String Interner — Comparative Analysis

A comparison of the Rust string-interning ecosystem across **handle size, memory
model, threading model, limits, reclamation, safety, special features, and
performance**. Where a crate ships multiple interner *models* (e.g. `lasso`'s
`Rodeo` vs `ThreadedRodeo`, or `string-interner`'s three backends), each model is
treated as a distinct row.

> **Sources.** Design/handle/threading/limit facts are drawn from each crate's
> public API and source (and, for `internity`, its own `src/`). Performance numbers
> come from `internity`'s in-repo head-to-head harness ([`docs/PERF.md`](PERF.md),
> `cargo bench --bench internity_compare`) over a corpus of ≈6000
> identifier-like strings on one dev box (`--release`, fat LTO). All timings are
> wall-clock medians measured by Criterion; treat them as *relative signal on this
> workload*, not universal constants — interner ranking shifts with string length,
> corpus size, hit/miss ratio, and thread count.

---

## 1. Master comparison table

| Crate / Model | Handle type | Handle size | `Option` niche | `Copy` | Storage & dedup design | Threading model | Reclamation | `unsafe` | `no_std` | Interns |
|---|---|---|---|:--:|---|---|---|:--:|:--:|---|
| **internity** `LocalLexicon` (single-thread) | `Sym(NonZeroU32)` = dense 1-based index | **4 B** | ✅ 4 B | ✅ | One contiguous `String` buffer + `Vec<u32>` CSR offsets (start/end, branch-free resolve); `hashbrown::HashTable<Sym>` dedup (store handle, probe by hash) — flat & cache-coherent, à la `string-interner` `StringBackend` | single-thread (`&mut` intern, `&self` resolve) | leak-until-drop | contained (`storage`) | ✅ | `str` |
| **internity** `ThreadedLexicon` (concurrent) | `Sym(NonZeroU32)` = `[shard:6\|local:26]` | **4 B** | ✅ 4 B | ✅ | 64 `align(128)` shards, each `RwLock<{ offsets:Vec<u32>, bytes:Vec<u8>, HashTable<Sym> }>`; upgradable-read hit path (cross-shard intern independent, same-shard intern serialized); **fill-then-freeze** (no live resolve); cheap `Clone` `Arc` handle | concurrent (`&self` intern, per-shard `RwLock`) | leak-until-drop | contained (`storage`) | ❌ (`std`) | `str` |
| **internity** `Reader` (frozen) | `Sym(NonZeroU32)` (same) | **4 B** | ✅ 4 B | ✅ | `freeze()` → flat `(offsets:[u32], bytes:[u8])` blob (CSR start/end, branch-free) — one blob for `LocalLexicon`, per-shard for `ThreadedLexicon`; `Reader` is a sealed **trait**, `freeze` returns the concrete `LocalReader` / `ThreadedReader` (static dispatch, nameable and storable by value) | immutable, **lock-free + atomic-free** resolve | leak-until-drop | contained (`storage`) | ✅ flat / ❌ sharded | `str` |
| **lasso** `Rodeo` | `Spur`=`NonZeroU32` (Mini/Micro/Large: 1–8 B) | 4 B (1–8 B) | ✅ | ✅ | Doubling bump-arena buckets; hashbrown raw-entry (store key, probe by hash) | single-thread (`&mut`) | leak-until-drop | yes (`Key` trait) | ✅ | `str` |
| **lasso** `ThreadedRodeo` | `Spur` (as above) | 4 B | ✅ | ✅ | **Two `DashMap`s** (str→key, key→str) + lock-free CAS arena | fully concurrent (`&self`) | leak-until-drop | yes | ✅ | `str` |
| **lasso** `RodeoReader` | `Spur` | 4 B | ✅ | ✅ | Frozen: drops the str→key map, keeps both directions read-only | concurrent read (`Sync`) | frozen | yes | ✅ | `str` |
| **lasso** `RodeoResolver` | `Spur` | 4 B | ✅ | ✅ | Frozen: **resolve only** (drops str→key map entirely) — least memory | concurrent read (`Sync`) | frozen | yes | ✅ | `str` |
| **string-interner** `StringBackend` (default) | `SymbolU32`=`NonZeroU32` (16/32/usize) | 4 B | ✅ | ✅ | One contiguous `String` + `Vec<usize>` ends — most cache-coherent, **no stable refs** | single-thread (`&mut`) | leak-until-drop | minimal (safe `Symbol`) | ✅ | `str` |
| **string-interner** `BucketBackend` | `SymbolU32` | 4 B | ✅ | ✅ | Fat-pointer spans, **stable refs**, `intern_static` | single-thread | leak-until-drop | some | ✅ | `str` |
| **string-interner** `BufferBackend` | `SymbolU32` | 4 B | ✅ | ✅ | Varint-length-packed into one buffer — **smallest memory**, slower resolve | single-thread | leak-until-drop | some | ✅ | `str` |
| **ustr** | `Ustr`=`NonNull<u8>` → UTF-8 (hash+len in header before chars) | **8 B** (ptr) | ✅ | ✅ | 64 cache-line-aligned shards (`parking_lot::Mutex`), open-addressing, **bump-down alloc**, precomputed ahash; pointer eq ≡ string eq | global, fully concurrent (`&self` free fns) | **leaks forever** | lots | ❌ | `str` |
| **internment** `Intern<T>` | `&'static T` | 8 B (ptr) | ✅ | ✅ | 32 type-sharded `Mutex<HashSet>` + `Box::leak` | concurrent | **leaks** | yes | partial | any `T` (+ DST `str`) |
| **internment** `ArcIntern<T>` | `NonNull<RefCount<T>>` | 8 B (ptr) | ✅ | ✅ (clone bumps rc) | per-type `DashMap` + SeqCst refcounts, pointer hash/eq | concurrent | **Arc GC (freed)** | yes | partial | any `T` |
| **internment** `ArenaIntern<'a,T>` | `&'a T` | 8 B (ptr) | ✅ | ✅ | scoped arena, lifetime-bound | scoped | drop with arena | yes | partial | any `T` |
| **string_cache** `Atom` (inline) | `NonZeroU64`, 2-bit tagged | **8 B** | ✅ | ❌ (Clone) | ≤7 B packed **inline** in the tag — no heap, no lock | concurrent | n/a (inline) | yes | ❌ | `str` |
| **string_cache** `Atom` (static) | `NonZeroU64` tagged | 8 B | ✅ | ❌ | Compile-time **PHF** index via `string_cache_codegen` / `atom!()` | concurrent read | static | yes | ❌ | `str` |
| **string_cache** `Atom` (dynamic) | `NonZeroU64` tagged | 8 B | ✅ | ❌ | 4096 `parking_lot::Mutex` buckets + chained lists, refcounted heap `Entry` | concurrent | **refcounted (freed)** | yes | ❌ | `str` |
| **symbol_table** `SymbolTable` | `Symbol`=`NonZeroU32` | 4 B | ✅ | ✅ | 16 `CachePadded` shards, foldhash (deterministic); `&self` intern+resolve | concurrent (`&self`) | leak-until-drop | some | ✅ | `str` |
| **symbol_table** `GlobalSymbol` (`global`) | `NonZeroU32` | 4 B | ✅ | ✅ | Process-global table + `static_symbol!` macro | concurrent, global | leaks | some | ✅ | `str` |
| **intaglio** | `u32` (Sym newtype) | 4 B | (u32) | ✅ | Broadest type support; `&'static` opt | single-thread (`&mut`) | leak | some | ❌ | `str`/`[u8]`/`CStr`/`OsStr`/`Path` |
| **arc-interner** | `Arc<T>` | 16 B (fat) | — | ✅ (clone) | `DashMap`; **value-hash O(n)** (poor map key) | concurrent | Arc GC | some | ❌ | any `T` |
| **interner** (khonsulabs) | `Arc` + index | ptr+idx | — | ❌ | `Mutex`; real GC + **slot recycling**, `#![forbid(unsafe)]` | concurrent | ✅ GC | **none** | ❌ | generic |
| **simple-interner** | `&'a T` | 8 B | ✅ | ✅ | `RwLock`, returns `&T` | concurrent | leak | some | ❌ | generic |
| **symbol** (remexre) | `&'static str` | 8 B (fat 16 B `str`) | — | ✅ | Global spin + **`BTreeSet` (O(log n))**, `gensym()` | global | leak | some | ❌ | `str` |
| **internship** | 16 B tagged | 16 B | — | ❌ | per-thread `Rc`; inline ≤15 B SSO idea | single-thread | Rc | some | ❌ | `str` |

**Reference points (not full interners, but instructive):**

| Thing | Handle | Design | Why it matters |
|---|---|---|---|
| **rustc `Symbol`** | `u32` newtype | `DroplessArena` bump + `HashTable` + FxHash + `symbols!` pre-seeding of common idents | The gold-standard compiler interner: no per-string alloc, no per-string drop, keyword fast path skips hashing. Session-scoped `Lock`, not global. |
| **`smol_str`** (~92M dl) / **`kstring`** (~47M dl) | inline SSO string (~23 B inline, static zero-copy, `Arc<str>` for long) | **No central dedup** | Complementary "value-side" technique: cheap when strings are diverse/short; interning wins when strings repeat heavily. |

---

## 2. Limits & special features

**Max distinct strings** = ceiling imposed by the handle width (and, where noted,
by the byte-offset encoding). **Max single string** = largest one string's payload.
"Cached hash" = the interner stores the string's hash so lookups/resizes never
re-hash (and downstream maps can identity-hash the handle).

| Crate / Model | Max distinct strings | Max single string | Cached hash | Other special features |
|---|---|---|---|---|
| **internity** `LocalLexicon` (single-thread) | `NonZeroU32` → **~4.29 B**, capped by a **≤ 4 GB** single buffer (`u32` offsets) | bounded by remaining buffer (≤ ~4 GB) | ❌ (recomputes on table resize) | flat dense-index resolve; `try_resolve` range-checks the numeric handle (out-of-range → `None`; in-range foreign handles may resolve to unrelated strings); `freeze()` → flat `Reader`; generic hasher (FxHash default); unchecked UTF-8 centralized in `storage`, Miri-clean; **`intern_bytes(&[u8])`** amortizes UTF-8 validation — checked once per distinct string, skipped on dedup hits — while still resolving to `&str` |
| **internity** `ThreadedLexicon` (concurrent) | `[shard:6\|local:26]` → **~4.29 B** (≤ ~67 M per shard × 64) | bounded by its shard's **≤ 4 GB** buffer | ❌ | cheap `Clone` `Arc` handle; per-shard `RwLock` (cross-shard intern independent; same-shard intern serialized; plain reads coexist); **fill-then-freeze**; up to **~256 GB** aggregate bytes (64 × 4 GB shards); safe public API, Miri-clean; **`intern_bytes(&[u8])`** amortizes UTF-8 validation (checked once per distinct string, skipped on hits) |
| **lasso** `Rodeo`/`ThreadedRodeo` | `Spur`=`NonZeroU32` → **~4.29 B** (`MiniSpur` 65 535 · `MicroSpur` 255 · `LargeSpur` `NonZeroUsize`) | bounded by memory / arena | ❌ | **`MemoryLimits`** (hard byte cap, fallible `try_get_or_intern`), `get_or_intern_static`, progressive freeze, custom `Key` widths/niches |
| **string-interner** `StringBackend` | `SymbolU32`=`NonZeroU32` → **~4.29 B** (`u16`/`usize` keys selectable) | bounded by memory (`usize` end) | ❌ | swappable backends, serde by default, `iter()` |
| **string-interner** `BufferBackend` | bounded by **~4 GB buffer** (symbol *is* a byte offset) | bounded by remaining buffer (varint len) | ❌ | smallest memory (varint packing, one allocation) |
| **string-interner** `BucketBackend` | ~4.29 B | bounded by memory | ❌ | stable `&str` refs, `get_or_intern_static` |
| **ustr** | unbounded (pointer handle; memory-bound, **leaks**) | bounded by memory (`len` in header) | ✅ **`precomputed_hash()`** (ahash stored in header) | pointer eq ≡ string eq, identity-hashed `UstrMap`/`UstrSet`, `as_str()->&'static str`, **FFI `as_cstr()`** (NUL-terminated), global `ustr(s)`/`existing_ustr` |
| **internment** `Intern`/`ArcIntern`/`ArenaIntern` | unbounded (pointer; memory-bound) | bounded by memory | ❌ (hashes by pointer, not value) | **generic over any `T`**, DST `Intern<str>`, `ArcIntern` refcounts & **frees**, `Copy` handle |
| **string_cache** `Atom` | inline: unbounded · static: PHF-set size · dynamic: memory-bound | **inline ≤ 7 B**; static/dynamic bounded by memory | ✅ **`get_hash()`** (64-bit hash in the tagged word / heap `Entry`) | 3-in-1 tagged handle (inline SSO / compile-time `atom!()` static / refcounted dynamic), ASCII-case helpers |
| **symbol_table** `SymbolTable`/`GlobalSymbol` | `NonZeroU32` → **~4.29 B** | bounded by memory | ❌ | `&self` intern **and** resolve, deterministic foldhash, `static_symbol!`, `global` feature, `no_std` |
| **intaglio** | `u32` → **~4.29 B** | bounded by memory | ❌ | interns `str`/`[u8]`/`CStr`/`OsStr`/`Path`, `&'static` optimization |
| **arc-interner** | unbounded (`Arc`; memory-bound) | bounded by memory | ❌ (value-hash O(n)) | generic `T`, Arc GC (unmaintained) |
| **rustc `Symbol`** (ref) | `u32` → **~4.29 B** | bounded by arena | ❌ | `symbols!` pre-seeds keywords (range check, no hash), dropless bump arena, session-scoped |

**Cached-hash takeaway:** only **ustr** and **string_cache** persist the string's
hash in the handle/entry, which is why their repeated-lookup / map-key paths are so
cheap (ustr's `resolve` is a handful of instructions; its maps skip re-hashing entirely).
Index-handle interners (internity, lasso, string-interner, symbol_table) recompute
the hash on each `intern` probe and on table growth.

---

## 3. Performance (internity in-repo harness, ≈6000 identifiers)

Three operations — **insert** (fresh strings), **reuse** (re-intern existing =
dedup hits), **lookup** (resolve handle → `&str`) — each measured **single-threaded**
and **multi-threaded** at 1/2/4/8 threads. Single-threaded uses internity's
`LocalLexicon` (+ the single-thread crates); multi-threaded uses `ThreadedLexicon`
(+ the concurrent crates). The concurrent rounds use one **coordinator-owned
interval**: every worker is prepared and released together, the timer starts at
release and stops once the last worker signals completion, so the reported figure
is the true wall-clock of the parallel phase (including scheduling latency), not
the maximum individual worker time. The **full matrix** (all thread counts, all
crates) lives in [`docs/PERF.md`](PERF.md); highlights below.

### Wall-clock timings

All wall-clock tables — the single-threaded per-operation comparison and the
concurrent scaling tables at 1/2/4/8 threads — live in
[`docs/PERF.md`](PERF.md) and are not duplicated here, so they cannot drift out of
sync with it. Global/cache-backed rows (`ustr`, `string_cache`) keep their table
alive for the whole process and therefore have no repeatable first-time
single-threaded `insert` timing. For insert/reuse, `lasso` is `ThreadedRodeo`; for
lookup, `lasso` is the frozen `RodeoResolver`, matching internity's frozen
`Reader` comparison.

**Read of the numbers:**
- **Single-threaded insert:** internity is the fastest owned interner on wall clock,
  ahead of `string-interner` and well clear of `lasso` and `symbol_table`.
- **Single-threaded reuse:** internity leads on retained-hit dedupe. The
  process-global `ustr` does less work per hit but is slower at corpus-level wall
  time on this workload.
- **Single-threaded lookup:** flat-array designs are all cheap, and internity's
  frozen `Reader` resolves faster than its live interner; `lasso` and `ustr` edge
  it here. Recommended pattern: intern → `freeze` → resolve.
- **Multi-threaded insert:** `ThreadedLexicon` and `symbol_table` trade the lead,
  and internity stays ahead of `lasso::ThreadedRodeo` at every measured insert
  thread count.
- **Multi-threaded reuse:** internity is competitive through 4 threads, but
  `lasso::ThreadedRodeo` pulls ahead at 8 threads on retained hits.
- **Multi-threaded lookup:** compared frozen-to-frozen, internity's frozen sharded
  `Reader` is level with `lasso::RodeoResolver` and stays far ahead of
  `symbol_table`. At this granularity the coordinator-owned interval is dominated
  by cross-thread scheduling, so the crates with a compact frozen reader
  (internity, `lasso`, `ustr`) cluster closely.

### Memory footprint (live heap, ≈6000 identifiers ≈ 73 KiB of text)

Measured with a tracking global allocator (`cargo bench --bench internity_mem`). `insert` =
the filled interner; `lookup` = the read structure (frozen form where a crate has
one). Lower is better.

| Interner | insert | lookup |
|---|---|---|
| **internity** (`LocalLexicon` → frozen `Reader`) | **172 KiB** ⭐ | **96 KiB** ⭐ |
| **internity** (`ThreadedLexicon` → frozen `Reader`) | 181 KiB | 99 KiB |
| lasso (`Rodeo` → `RodeoResolver`) | 264 KiB | 224 KiB |
| string-interner | 204 KiB | 204 KiB¹ |
| symbol_table | 241 KiB | 241 KiB¹ |
| ustr (global) | 8232 KiB² | 8232 KiB² |
| string_cache (global) | 352 KiB | 352 KiB |

¹ No frozen read form, so the lookup structure is the full filled interner.
² `ustr` pre-reserves a large global table/arena; it trades ~85× the memory of
internity's frozen reader for its pointer-is-the-string lookups.

internity is the **most compact owned interner** in both phases, and freezing
**roughly halves** its footprint by dropping the string→handle dedup map — a memory
win on top of the speed results above.

---

## 4. What makes each unique

- **internity** — ships **two front-ends over one 4-byte `Sym` and one `Reader`**:
  a single-threaded **`LocalLexicon`** (flat `String` buffer + `Vec<u32>` CSR offsets,
  `&mut` intern) that leads `string-interner` on both insert and
  reuse, and a concurrent **`ThreadedLexicon`** (64 `align(128)` shards, per-shard
  `RwLock` with an upgradable-read hit path, cheap `Clone` `Arc` handle) that
  leads `lasso` concurrent insert at every measured thread count and remains much
  faster than `symbol_table` for concurrent lookup.
  Both are **fill-then-freeze**: `freeze()` yields a lock-free/atomic-free `Reader`
  whose resolve is faster than the live interner's and costs a quarter of its index
  memory. All unchecked UTF-8
  reconstruction is centralized in one `storage` module; every other module forbids `unsafe`.
  Miri-clean, range-checkable ids, generic hasher (FxHash default).
- **lasso** — the concurrent workhorse with a **progressive-freezing pipeline**
  (`Rodeo` → `RodeoReader` → `RodeoResolver`) that sheds memory as you drop
  capabilities. Multiple key widths (1–8 B), fallible `try_*` API, memory limits,
  `get_or_intern_static`. Concurrent path uses **two DashMaps** (scales poorly past
  ~24 threads).
- **string-interner** — the flexible single-threaded standard with **three
  swappable backends**: `StringBackend` (cache-coherent, fastest fill/hit),
  `BucketBackend` (stable refs + static opt), `BufferBackend` (varint-packed,
  smallest memory). Safe `Symbol` trait, serde by default. No concurrent variant.
- **ustr** — the fastest **global concurrent** interner and repeated-lookup king:
  the handle is a **bare pointer straight at the UTF-8**, with hash+len stored in a
  header before the chars, so `resolve` is a handful of instructions and pointer equality ≡
  string equality. Identity-hashed `UstrMap`/`UstrSet`. Trade-off: **leaks
  forever**, lots of `unsafe`, no `no_std`, fixed hash seed.
- **internment** — the **generic** interner: interns any `T` (with DST `Intern<str>`),
  not just strings. Three models: `Intern` (Copy, leaks), `ArcIntern` (refcounted,
  **actually frees** memory), `ArenaIntern` (scoped). Per-type lock contention and
  SeqCst refcount cost.
- **string_cache** (Servo) — **tagged `NonZeroU64` atoms** that unify three worlds:
  **inline SSO** (≤7 B, no heap/lock), **compile-time static** (`atom!()` PHF, single
  int compare), and **refcounted dynamic**. Powers html5ever/cssparser. Dynamic
  creation is slow (lock + linked-list walk + Box); handle is Clone, not Copy.
- **symbol_table** — minimal, deterministic (foldhash) sharded interner with
  **`&self` intern *and* resolve** (16 `CachePadded` shards), a `global` feature, and
  `static_symbol!`. `no_std`. Closest design sibling to internity.
- **intaglio** — broadest **type coverage** (str/bytes/CStr/OsStr/Path) with
  `&'static` optimization; from Artichoke Ruby. Single-threaded (`&mut`).
- **interner** (khonsulabs) — **`#![forbid(unsafe)]`** with a real **GC + slot
  recycling**; handle is not Copy. Stalled since 2023.
- **rustc `Symbol`** (reference) — the archetype: dropless bump arena + FxHash +
  `symbols!` pre-seeding so hot keywords skip hashing entirely. Session-scoped, not
  global.
- **smol_str / kstring** (reference) — SSO string *values* with **no central
  dedup**; complementary to interning rather than competing with it.

---

## 5. Choosing an interner

| If you need… | Pick |
|---|---|
| Fastest single-thread insert/reuse, simple API | **internity** `LocalLexicon` leads both insert and reuse on this workload, with **string-interner** (`StringBackend`) next; pick `lasso` `Rodeo` for its fallible/memory-limit API |
| Highest concurrent-insert throughput with a 4-byte handle | **internity** `ThreadedLexicon` and **symbol_table** trade the lead across thread counts; both lead `lasso`-threaded on this workload. |
| Fastest concurrent lookup with a compact handle | **internity** frozen `Reader` and **lasso** `RodeoResolver` are level in the frozen-reader comparison; both are far faster than `symbol_table` and internity keeps the smallest measured owned footprint |
| Fastest repeated lookup / `HashMap` keys / global pointer-equality | **ustr** |
| Progressive memory shedding after an intern phase | **lasso** `RodeoReader`/`RodeoResolver`, or **internity** `freeze()` |
| Smallest memory footprint | **string-interner** `BufferBackend` |
| Interning arbitrary types, not just strings | **internment** (or **intaglio** for str/bytes/paths) |
| Reclamation / GC of short-lived sets | **internment** `ArcIntern`, **string_cache** dynamic, **interner** (khonsulabs) |
| Compile-time keyword sets / zero-cost static atoms | **string_cache** `atom!()` (or rustc-style `symbols!`) |
| Zero `unsafe` | **interner** (khonsulabs) |
| Strings are diverse/short (little repetition) | **smol_str / kstring** (SSO, skip interning) |

---

## 6. Best-fit vs ill-suited scenarios (per solution)

| Solution | Great for | Ill-suited / avoid when |
|---|---|---|
| **internity** `LocalLexicon` (single-thread) | Single-threaded build-up of a symbol table with near-best insert/reuse, a 4-byte `Copy`, range-checkable handle, and unchecked UTF-8 isolated in one storage module; then `freeze()` for compact resolve | Concurrent interning (use `ThreadedLexicon`); resolve-heavy access **before** `freeze()`; a single corpus **> ~4 GB** of bytes |
| **internity** `ThreadedLexicon` (concurrent) | Concurrent build-up from many threads (compilers, parsers, log/label ingestion); ahead of `lasso` at every measured insert thread count; frozen lookup is much faster than `symbol_table`; cross-shard dedup hits proceed independently; cheap `Clone` `Arc` handle | Needing to **resolve while still interning** — it's fill-then-freeze, so resolve happens on the frozen `Reader`; read-heavy repeated interning of values in one shard (one upgradable-read slot); retained-hit throughput at 8 threads on this workload (`lasso::ThreadedRodeo` leads); a single string **> ~4 GB** (its shard's buffer) |
| **internity** `Reader` (frozen) | Intern once, then resolve forever, lock-free and atomic-free, keeping existing `Sym`s valid; frozen `LocalLexicon` resolve is faster than the live interner's at ¼ its index memory | Any further interning (it's immutable); still behind `ustr`'s pointer-*is*-the-string resolve |
| **lasso** `Rodeo` | Single-threaded interning with a clean, fallible API, tunable key width, and a **hard memory cap** | Concurrent writers (needs `ThreadedRodeo`); resolve-latency-critical paths where `ustr`/flat arrays win |
| **lasso** `ThreadedRodeo` | Shared concurrent interner with both directions and fallible ops | **High core counts** — the dual-`DashMap` design scales poorly past ~24 threads and trails internity/symbol_table ~1.2–3× on concurrent insert; heaviest memory of the lasso family |
| **lasso** `RodeoReader` / `RodeoResolver` | Freezing after a build phase to shed memory and share read-only across threads; `Resolver` is the smallest-footprint resolve-only option | Workloads that still intern; `Resolver` when you also need string→key lookups (it drops that map) |
| **string-interner** `StringBackend` | Cache-coherent single-threaded fill/hit (the closest owned competitor to internity's `LocalLexicon`); the default "just works" choice; serde | Any multithreading (no `Sync` interner); when you need stable `&str` refs (buffer relocates) or `&'static` static interning |
| **string-interner** `BufferBackend` | **Tightest memory** footprint (varint-packed single allocation) | Resolve-heavy use (slower unpack); corpora **> ~4 GB** (symbol is a byte offset) |
| **string-interner** `BucketBackend` | Single-thread interning that needs stable string refs + `intern_static` | Concurrency; absolute lowest memory (buckets cost more than the buffer backend) |
| **ustr** | **Fastest repeated lookup & map keys** (cached hash, pointer-eq, identity-hashed maps), heavy concurrent interning, FFI/C interop, `&'static str` escape | Long-running processes that intern **unbounded/attacker-controlled** strings — it **leaks forever** (memory-exhaustion risk); `no_std`; needing a DoS-resistant/seedable hash; needing a compact integer handle (it's pointer-sized, global-only) |
| **internment** `Intern<T>` | Interning **arbitrary types** (incl. DST `Intern<str>`) with a `Copy`, leak-forever handle | Hot per-type contention (one lock per `T`); bounded-memory needs (leaks); short-lived values |
| **internment** `ArcIntern<T>` | Long-running services with **churning / short-lived** interned sets that must be **reclaimed** | Throughput-critical creation (per-type `DashMap` + SeqCst refcounts serialize); using the handle as a stable ordered key (pointer-based identity) |
| **string_cache** `Atom` | HTML/CSS/XML-style workloads with **compile-time keyword sets** (`atom!()`), many **tiny** strings (≤7 B inline, no heap/lock), and reclaimable dynamic atoms | Bulk interning of **long or unique** strings (dynamic path: lock + linked-list walk + `Box` is slow); needing a `Copy` handle (it's `Clone`) or `no_std` |
| **symbol_table** | Deterministic, `no_std`, `&self` intern **and** resolve with a compact `NonZeroU32`; global `static_symbol!` sets | Resolve-latency-critical paths (sharded scatter makes it the **slowest resolver** measured); single-thread fill vs `string-interner` |
| **intaglio** | Interning **non-string** payloads (`[u8]`/`CStr`/`OsStr`/`Path`) with `&'static` optimization | Multithreaded interning (single-threaded `&mut` only) |
| **arc-interner** | (Legacy) generic refcounted interning | New code — **unmaintained**, and value-hash O(n) makes the handle a poor map key |
| **interner** (khonsulabs) | Needing **zero `unsafe`** plus real GC with slot recycling | Performance-sensitive or `Copy`-handle needs; project is stalled |
| **rustc `Symbol`** (pattern) | Compiler/session-scoped interning: dropless arena + pre-seeded keywords that skip hashing | As an off-the-shelf crate (it's internal to rustc) or a process-global shared interner |
| **smol_str / kstring** (complementary) | Strings that are **short and diverse** (little repetition): inline SSO avoids any central table | Highly **repeated** strings where dedup + a small handle pays off — use a real interner instead |
