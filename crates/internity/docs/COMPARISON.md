# Rust String Interner — Comparative Analysis

A comparison of the Rust string-interning ecosystem across **handle size, memory
model, threading model, limits, reclamation, safety, special features, and
performance**. Where a crate ships multiple interner *models* (e.g. `lasso`'s
`Rodeo` vs `ThreadedRodeo`, or `string-interner`'s three backends), each model is
treated as a distinct row.

> **Sources.** Design/handle/threading/limit facts are drawn from each crate's
> public API and source (and, for `internity`, its own `src/`). Performance numbers
> come from `internity`'s in-repo head-to-head harness (`docs/PERF.md`,
> `cargo bench --bench compare` / `--bench counts`) over a corpus of ≈6000
> identifier-like strings on one dev box (`--release`, fat LTO). Treat timings as
> *relative signal on this workload*, not universal constants — interner ranking
> shifts with string length, corpus size, hit/miss ratio, and thread count.

---

## 1. Master comparison table

| Crate / Model | Handle type | Handle size | `Option` niche | `Copy` | Storage & dedup design | Threading model | Reclamation | `unsafe` | `no_std` | Interns |
|---|---|---|---|:--:|---|---|---|:--:|:--:|---|
| **internity** `Lexicon` (single-thread) | `Sym(NonZeroU32)` = dense 1-based index | **4 B** | ✅ 4 B | ✅ | One contiguous `String` buffer + `Vec<u32>` CSR offsets (start/end, branch-free resolve); `hashbrown::HashTable<Sym>` dedup (store handle, probe by hash) — flat & cache-coherent, à la `string-interner` `StringBackend` | single-thread (`&mut` intern, `&self` resolve) | leak-until-drop | contained (`storage`) | ✅ | `str` |
| **internity** `ThreadedLexicon` (concurrent) | `Sym(NonZeroU32)` = `[shard:6\|local:26]` | **4 B** | ✅ 4 B | ✅ | 64 `align(128)` shards, each `RwLock<{ offsets:Vec<u32>, bytes:Vec<u8>, HashTable<Sym> }>`; upgradable-read hit path (parallel hits); **fill-then-freeze** (no live resolve); cheap `Clone` `Arc` handle | concurrent (`&self` intern, per-shard `RwLock`) | leak-until-drop | contained (`storage`) | ❌ (`std`) | `str` |
| **internity** `Reader` (frozen) | `Sym(NonZeroU32)` (same) | **4 B** | ✅ 4 B | ✅ | `freeze()` → flat `(offsets:[u32], bytes:[u8])` blob (CSR start/end, branch-free) — one blob for `Lexicon`, per-shard for `ThreadedLexicon`; `Reader` is a **trait**, `freeze` returns `impl Reader` (static dispatch, concrete types hidden) | immutable, **lock-free + atomic-free** resolve | leak-until-drop | contained (`storage`) | ✅ flat / ❌ sharded | `str` |
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
| **internity** `Lexicon` (single-thread) | `NonZeroU32` → **~4.29 B**, capped by a **≤ 4 GB** single buffer (`u32` offsets) | bounded by remaining buffer (≤ ~4 GB) | ❌ (recomputes on table resize) | flat dense-index resolve; range-checked `try_resolve` rejects foreign/stale handles; `freeze()` → flat `Reader`; generic hasher (FxHash default); unchecked UTF-8 centralized in `storage`, Miri-clean |
| **internity** `ThreadedLexicon` (concurrent) | `[shard:6\|local:26]` → **~4.29 B** (≤ ~67 M per shard × 64) | bounded by its shard's **≤ 4 GB** buffer | ❌ | cheap `Clone` `Arc` handle; per-shard `RwLock` (upgradable-read hits run in parallel); **fill-then-freeze**; up to **~256 GB** aggregate bytes (64 × 4 GB shards); safe public API, Miri-clean |
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
cheap (ustr's `resolve` is ~4 instructions; its maps skip re-hashing entirely).
Index-handle interners (internity, lasso, string-interner, symbol_table) recompute
the hash on each `intern` probe and on table growth.

---

## 3. Performance (internity in-repo harness, ≈6000 identifiers)

Three operations — **insert** (fresh strings), **reuse** (re-intern existing =
dedup hits), **lookup** (resolve handle → `&str`) — each measured **single-threaded**
and **multi-threaded** at 1/2/4/8 threads. Single-threaded uses internity's
`Lexicon` (+ the single-thread crates); multi-threaded uses `ThreadedLexicon`
(+ the concurrent crates), barrier-timed so only the parallel work counts. The
**full matrix** (all thread counts, all crates) lives in
[`docs/PERF.md`](PERF.md); highlights below.

### Single-threaded (one core) — wall-clock, ⭐ = fastest

| Op | internity | lasso | string-interner | symbol_table | ustr | string_cache |
|---|---|---|---|---|---|---|
| **insert** | **225 µs** ⭐ | 521 µs | 254 µs | 401 µs | —¹ | —¹ |
| **reuse** | 97 µs | 224 µs | **94 µs** ⭐ | 130 µs | 217 µs | 277 µs |
| **lookup** | 18 µs live / **11.8 µs frozen** | 12.1 µs | 12.1 µs | 53 µs | **8.4 µs** ⭐ | 10.4 µs |

¹ Global caches can't be reset between iterations, so a repeatable single-threaded
`insert` isn't expressible; their single-insert cost is captured in instructions.

**Instruction counts (gungraun/Callgrind, deterministic)** — per single op:

| Op | internity | lasso | string-interner | symbol_table | ustr | string_cache |
|---|---|---|---|---|---|---|
| insert | **201** | 293 | 212 | 498 | 168 | 653 |
| reuse | **167** | 175 | 179 | 450 | 133 | 287 |
| lookup | 50 live / **35 frozen** | 36 | 45 | 510 | **4** | 16 |

### Multi-threaded — wall-clock (total work = threads × corpus), ⭐ = fastest

| Op / threads | internity | lasso-threaded | symbol_table | ustr | string_cache |
|---|---|---|---|---|---|
| **insert** ×2 | **1.55 ms** ⭐ | 3.4 ms | 2.01 ms | — | — |
| **insert** ×4 | **2.22 ms** ⭐ | 4.2 ms | 2.42 ms | — | — |
| **insert** ×8 | **3.36 ms** ⭐ | 6.3 ms | 3.59 ms | — | — |
| **reuse** ×4 | 1.02 ms | 1.14 ms | 1.07 ms | **0.90 ms** ⭐ | 1.22 ms |
| **reuse** ×8 | 1.72 ms | **1.59 ms** ⭐ | 2.29 ms | 1.76 ms | 1.68 ms |
| **lookup** ×4 | **356 µs** ⭐ | 790 µs | 647 µs | 354 µs | 362 µs |
| **lookup** ×8 | **674 µs** ⭐ | 1.3 ms | 1.4 ms | ~0.68 ms | ~0.68 ms |

**Read of the numbers:**
- **Single-threaded insert:** internity's `Lexicon` **leads decisively** — a single
  contiguous `String` buffer, no lock — ~13 % ahead of `string-interner` and 1.8–2.3×
  ahead of `lasso`/`symbol_table`.
- **Single-threaded reuse:** a **statistical tie** with `string-interner` (order
  flips run-to-run within a few %); internity uses fewer instructions (167 vs 179)
  and decisively beats everyone else.
- **Single-threaded lookup:** flat-array designs win; `ustr` is untouchable (the
  pointer *is* the string, 4 instr). internity's **live** resolve pays UTF-8
  boundary checks (~18 µs), but its **frozen** `Reader` (unchecked flat slice, CSR
  `offsets`, branch-free) lands at **11.8 µs / 35 instr — edging out `lasso` (36)
  and `string-interner` (45)** at ¼ of lasso's index memory. Recommended pattern:
  intern → `freeze` → resolve.
- **Multi-threaded insert:** `ThreadedLexicon` **leads at every thread count** (2–8),
  beating `lasso::ThreadedRodeo` ~1.6–2.2× and `symbol_table` by 7–30 %. Interning
  under an upgradable read lock keeps the miss path a single lock escalation.
- **Multi-threaded reuse:** the upgradable-read fast path lets concurrent hits run
  in parallel, so internity **beats `symbol_table` (up to 1.3×)** and matches or
  beats the leak-forever globals (`ustr`, `string_cache`) except at the very highest
  thread counts, where those pointer-based caches pull a few % ahead.
- **Multi-threaded lookup:** the frozen sharded `Reader` **beats `lasso::ThreadedRodeo`
  and `symbol_table` ~2–4×** and runs neck-and-neck with the pointer-based globals
  (`ustr`, `string_cache`).

### Memory footprint (live heap, ≈6000 identifiers ≈ 73 KiB of text)

Measured with a tracking global allocator (`cargo bench --bench mem`). `insert` =
the filled interner; `lookup` = the read structure (frozen form where a crate has
one). Lower is better.

| Interner | insert | lookup |
|---|---|---|
| **internity** (`Lexicon` → frozen `Reader`) | **172 KiB** ⭐ | **96 KiB** ⭐ |
| **internity** (`ThreadedLexicon` → frozen `Reader`) | 181 KiB | 99 KiB |
| lasso (`Rodeo` → `RodeoResolver`) | 264 KiB | 224 KiB |
| string-interner | 204 KiB | 204 KiB¹ |
| symbol_table | 241 KiB | 241 KiB¹ |
| ustr (global) | 8232 KiB² | 8232 KiB² |
| string_cache (global) | 352 KiB | 352 KiB |

¹ No frozen read form, so the lookup structure is the full filled interner.
² `ustr` pre-reserves a large global table/arena; it trades ~45× the memory of
internity's frozen reader for its pointer-is-the-string lookups.

internity is the **most compact owned interner** in both phases, and freezing
**roughly halves** its footprint by dropping the string→handle dedup map — a memory
win on top of the speed results above.

---

## 4. What makes each unique

- **internity** — ships **two front-ends over one 4-byte `Sym` and one `Reader`**:
  a single-threaded **`Lexicon`** (flat `String` buffer + `Vec<u32>` CSR offsets,
  `&mut` intern) that **leads single-thread insert** and ties `string-interner` on
  reuse, and a concurrent **`ThreadedLexicon`** (64 `align(128)` shards, per-shard
  `RwLock` with an upgradable-read hit path, cheap `Clone` `Arc` handle) that
  **leads concurrent insert at every thread count and concurrent lookup outright**.
  Both are **fill-then-freeze**: `freeze()` yields a lock-free/atomic-free `Reader`
  whose resolve **edges out `lasso`**. All unchecked UTF-8 reconstruction is
  centralized in one `storage` module; every other module forbids `unsafe`.
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
  header before the chars, so `resolve` is ~4 instructions and pointer equality ≡
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
| Fastest single-thread insert/reuse, simple API | **internity** `Lexicon` (leads insert, ties on reuse), **string-interner** (`StringBackend`), or **lasso** `Rodeo` |
| Highest concurrent-insert throughput with a 4-byte handle | **internity** `ThreadedLexicon` (leads every thread count, 2–8; ~1.6–2.2× over `lasso`-threaded) |
| Fastest concurrent lookup with a compact handle | **internity** frozen `Reader` (beats `lasso`-threaded / `symbol_table` ~2–4×, ties the pointer globals) |
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
| **internity** `Lexicon` (single-thread) | Single-threaded build-up of a symbol table with the **fastest insert measured** (reuse tied with `string-interner`) and a 4-byte `Copy`, range-checkable handle, in fully-safe code; then `freeze()` for lasso-beating resolve | Concurrent interning (use `ThreadedLexicon`); resolve-heavy access **before** `freeze()` (the live slice pays UTF-8 boundary checks); a single corpus **> ~4 GB** of bytes |
| **internity** `ThreadedLexicon` (concurrent) | Concurrent build-up from many threads (compilers, parsers, log/label ingestion) with **leading concurrent-insert throughput** (fastest at every thread count 2–8) and **fastest concurrent lookup** via the frozen `Reader`; upgradable-read hits scale reuse too; cheap `Clone` `Arc` handle; safe interning path | Needing to **resolve while still interning** — it's fill-then-freeze, so resolve happens on the frozen `Reader`; a single string **> ~4 GB** (its shard's buffer) |
| **internity** `Reader` (frozen) | Intern once, then resolve forever, lock-free and atomic-free, keeping existing `Sym`s valid; frozen `Lexicon` resolve **edges out `lasso`** (35 vs 36 instr) at ¼ its index memory | Any further interning (it's immutable); still behind `ustr`'s pointer-*is*-the-string resolve |
| **lasso** `Rodeo` | Single-threaded interning with a clean, fallible API, tunable key width, and a **hard memory cap** | Concurrent writers (needs `ThreadedRodeo`); resolve-latency-critical paths where `ustr`/flat arrays win |
| **lasso** `ThreadedRodeo` | Shared concurrent interner with both directions and fallible ops | **High core counts** — the dual-`DashMap` design scales poorly past ~24 threads and trails internity/symbol_table ~1.4–2.8× on concurrent insert; heaviest memory of the lasso family |
| **lasso** `RodeoReader` / `RodeoResolver` | Freezing after a build phase to shed memory and share read-only across threads; `Resolver` is the smallest-footprint resolve-only option | Workloads that still intern; `Resolver` when you also need string→key lookups (it drops that map) |
| **string-interner** `StringBackend` | Cache-coherent single-threaded fill/hit (fill a hair behind internity's `Lexicon`, hit tied); the default "just works" choice; serde | Any multithreading (no `Sync` interner); when you need stable `&str` refs (buffer relocates) or `&'static` static interning |
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
