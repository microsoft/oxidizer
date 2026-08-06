# g1-multitude findings

Scope: `crates/multitude`, `crates/multitude_macros`, `crates/multitude_macros_impl`.

Environment caveat that applies to every finding below: this container has no
network egress to `index.crates.io` / `static.crates.io`, no populated cargo
registry cache and no prebuilt `target/`. `cargo build`, `cargo bench`,
`cargo clippy`, `cargo build --offline` and every `just` recipe fail at
dependency resolution. Consequently **no finding in this document was produced
by running the repository's own benchmarks**. Findings are labelled
`inferred from code reading` unless stated otherwise, and each names the
specific benchmark or measurement that would confirm or refute it.

The one empirical technique that does work here is a dependency-free
`rustc`-compiled layout probe: a standalone program containing
layout-identical replicas of the types under discussion, printing
`size_of` / `align_of`. Findings labelled `empirically verified (layout probe)`
were measured that way on `x86_64-unknown-linux-gnu`, rustc default settings.
The probe program lived in `/tmp` and was deleted; it is not part of the repo.

---

## Crate: multitude

### Summary

`multitude` is the workspace's arena allocator: a bump allocator over
64 KiB-aligned chunks, with thin (single-pointer, 8-byte) `Box`/`Arc`/`Rc`
smart pointers whose length and strong count live in an allocation-resident
prefix, arena-backed `Vec`/`String`/`Utf16String`/`Cow`, an
`allocator_api2::Allocator` implementation, and an arena-aware Serde
deserialization stack (`de::`) including a `serde_json` front end and a
`DeserializeIn` derive. At ~64k LOC it is the largest crate in the workspace.

Its performance posture is genuinely strong and clearly deliberate. The
allocation hot path is a three-instruction bump (`chunk_mutator.rs:152-186`)
with `hint::assert_unchecked` used to remove overflow checks from the alignment
arithmetic on 64-bit; refill, oversize and growth paths are consistently
`#[cold] #[inline(never)]`; there is no `format!`, `to_string()`, `vec![`,
`String::new` or `.collect()` anywhere in non-test `src/`; and
`LARGE_SHARED_REF_SURPLUS` (`arena/mod.rs:41`) is a first-rate optimisation
that converts the per-allocation smart-pointer refcount bump from an atomic RMW
into a non-atomic `Cell<u32>` increment. The findings below are therefore
mostly second-order: type layout, atomic ordering, two-pass transcoding, a
handful of hot/cold-split asymmetries, and — most significantly — benchmark
blind spots over entire feature areas.

### Findings

#### F1. `de::Value` and `de::Number` are inflated to 32 bytes / align 16 by the `i128`/`u128` variants

- **Location:** `crates/multitude/src/de/value/number.rs:11-37`,
  `crates/multitude/src/de/value/dynamic_value.rs:18-47`,
  `crates/multitude/src/de/value/entry.rs:20-45`
- **Issue:** `Number` carries `I128(i128)` and `U128(u128)`. Those two variants
  force `align_of::<Number>() == 16` and therefore
  `size_of::<Number>() == 32`. Because `Value::Number(Number)` embeds `Number`
  by value, the *entire* `Value` enum inherits align 16 and grows to 32 bytes,
  even though every other variant is at most 16 bytes (multitude's whole point
  is that `Box<str, A>`, `Box<[Value], A>` and `Map<A>` are 8-byte thin
  handles). Measured on a layout replica:

  | type | actual | with 128-bit variants removed | with 128-bit payloads boxed |
  |---|---|---|---|
  | `Number` | 32 B, align 16 | 16 B, align 8 | — |
  | `Value` | **32 B, align 16** | 24 B, align 8 | **24 B, align 8** |
  | `Entry` (`{key: Value, value: Value}`) | 64 B | 48 B | 48 B |
  | `[Value; 8]` | 256 B | 192 B | 192 B |

  A JSON document deserialised into `Value` is a `Box<[Value], A>` sequence and
  a `Map<A> = Box<[Entry<A>], A>`. Every element is 33 % larger and every
  `Entry` occupies exactly one full cache line (64 B) rather than fitting 4
  per 3 lines. 128-bit integers essentially never appear in JSON, CBOR,
  MessagePack or any mainstream wire format, so almost all users pay this
  permanently for a case they never hit. Note also that the arena bump
  allocator must honour align 16 for every `Value` allocation, adding up to
  8 bytes of alignment padding per scalar `Box<Value>` on top.
- **Impact:** High — this is the crate's headline "dynamic value into arena"
  workload (`multitude_serde/dynamic`, `multitude_record_batch/decode`) and the
  regression is a straight 33 % increase in bytes touched and cache lines
  fetched during both construction and traversal. It also inflates arena
  footprint, which is the metric the crate is sold on.
- **Remediation:** Two options, in increasing order of disruption.
  (a) Surgical and layout-optimal: change `Number::I128`/`Number::U128` to hold
  the value indirectly or as a byte-array/pair-of-`u64` newtype with accessor
  methods, e.g. `I128(I128Bits)` where
  `struct I128Bits([u8; 16])` with `const fn get(self) -> i128`. This drops
  `Number` to 16 B / align 8 and `Value` to 24 B / align 8 while keeping the
  variants. It is a breaking change to a public enum, but the enum is already
  `#[non_exhaustive]`-adjacent in spirit and the variants are pattern-matched
  by name.
  (b) Alternative measured above: box the 128-bit payloads into the arena
  (`I128(Box<i128, A>)`), which also yields 24 B / align 8 — allocation only on
  the (vanishingly rare) 128-bit path.
  Either way, verify with a `size_of` assertion test so the layout cannot
  silently regress.
- **Evidence:** empirically verified (standalone `rustc` layout probe with
  replicas of `Number`, `Value`, `Entry` and an 8-byte thin-pointer stand-in;
  numbers in the table above). The *performance consequence* of the layout is
  inferred; confirm with `multitude_serde_cg`'s `dynamic` group
  (`benches/multitude_serde_cg.rs:112`) instruction counts plus a Callgrind
  `D1mr`/`DLmr` comparison before and after.
- **Philosophy note:** `docs/performance.md` says optimisations must be
  motivated by a real user-facing scenario and prefers surgical changes over
  representation changes. This *is* a representation change to a public type,
  so it is on the wrong side of that guidance — but the motivating scenario
  (JSON→arena `Value`) is the crate's flagship benchmark, and option (a) is a
  single-enum edit rather than a multi-file restructuring. Reporting it as a
  finding, not as a done deal.

#### F2. `ChunkProvider::reserve_bytes` runs a CAS retry loop on every chunk acquisition even when the byte budget is unbounded

- **Location:** `crates/multitude/src/internal/chunk_provider.rs:469-489`;
  default budget set at `crates/multitude/src/arena/mod.rs:298`
- **Issue:** `reserve_bytes` is called for every chunk taken from the backing
  allocator. It uses `AtomicUsize::fetch_update(AcqRel, Relaxed, ...)`, which
  expands to a `load` + closure + `compare_exchange_weak` retry loop. The
  closure's only job is `checked_add` plus a comparison against
  `self.config.byte_budget`. The default configuration sets
  `byte_budget = usize::MAX`, in which case the comparison can never fail and
  the `checked_add` can only fail on an overflow that would require
  `usize::MAX` live bytes. So in the default configuration the loop is
  unconditionally a more expensive way to spell `fetch_add`. The comment above
  it explains why `fetch_update` was chosen (coverage of the contention arm),
  not that it is required.
- **Impact:** Medium — this is on the chunk-refill path, not the per-allocation
  path, so it is amortised over ~64 KiB of allocations. But refill *is* the
  measured cost in `multitude_teardown`, in `criterion_alloc/arena_lifecycle`
  and in every `reset`-then-refill workload, and a CAS loop is meaningfully
  more expensive than an unconditional `lock xadd` when the line is contended
  by a foreign thread returning a chunk.
- **Remediation:** Surgical: add an unbounded fast path.
  ```rust
  if self.config.byte_budget == usize::MAX {
      self.bytes_outstanding.fetch_add(n, Ordering::Relaxed);
      return Ok(());
  }
  ```
  keeping the existing `fetch_update` for the bounded case. This preserves the
  defensive budget check where a budget actually exists, which is what
  `docs/performance.md` asks for.
- **Evidence:** inferred from code reading. Confirm with
  `benches/multitude_teardown_cg.rs` instruction counts (the groups at
  `:61-73`) and `criterion_alloc/arena_lifecycle`
  (`benches/criterion_alloc.rs:23-27`), which are the two benchmarks dominated
  by chunk acquisition rather than bump arithmetic.

#### F3. Atomic orderings stronger than the data flow requires on pure counters

- **Location:** `crates/multitude/src/internal/chunk_provider.rs:491-499`
  (`release_bytes`, `release_reservation`), `:615-619` (`record_allocation`)
- **Issue:** `release_bytes` and `release_reservation` do
  `fetch_sub(n, Ordering::AcqRel)` on `bytes_outstanding`. These are pure
  accounting counters: nothing is published or acquired through them, and no
  other memory is ordered against them. `AcqRel` on a `fetch_sub` emits a full
  `lock`-prefixed RMW with acquire *and* release semantics; on x86 the RMW is
  already sequentially consistent so the cost is compile-time reordering
  barriers, but on AArch64 (`ldaxr`/`stlxr` vs plain `ldxr`/`stxr`) the
  difference is real. `record_allocation` (stats only) likewise uses `AcqRel`
  on `allocated_bytes` while its two sibling counters on the following lines
  correctly use `Relaxed` — the inconsistency suggests the `AcqRel` is not
  deliberate. `#[cfg(feature = "stats")] allocated_bytes.fetch_sub(n, AcqRel)`
  in `release_bytes` has the same shape.
- **Impact:** Low on x86-64, Medium on AArch64 — chunk-cadence, not
  allocation-cadence, and only barrier strength rather than an extra RMW.
- **Remediation:** Downgrade the pure counters to `Relaxed`. If any of them
  ever needs to order chunk *contents*, that ordering belongs on the chunk
  refcount (`Chunk::ref_count`), not on a byte counter; a comment saying so
  would be worth more than the `AcqRel`.
- **Evidence:** inferred from code reading. Confirm by reading the disassembly
  on `aarch64-unknown-linux-gnu` (`ldaxr`→`ldxr`) and re-running
  `multitude_teardown_cg`. Also worth running the existing `loom` feature's
  model checks after the change to prove the weaker orderings are sound.

#### F4. Missing cold-path split on the `Arc`/`Rc`/`Box` and `Allocator::allocate` allocation entry points, unlike the `Alloc` path

- **Location:** `crates/multitude/src/arena/alloc_value.rs:899-920`
  (`alloc_smart_prefixed_with_raw`), `:933-960` (`impl_alloc_smart_with`),
  `crates/multitude/src/allocator_impl.rs:31-74`
  (`<&Arena<A> as Allocator>::allocate`); contrast with the correct shape at
  `crates/multitude/src/arena/alloc_value.rs:809-818`
  (`alloc_value_with_raw` delegating to a `#[cold] #[inline(never)]`
  `alloc_value_refill_with`)
- **Issue:** The `Alloc` scalar path gets it right: try the bump once, and on
  failure tail-call into a `#[cold] #[inline(never)]` continuation that owns the
  refill loop and the oversized branch. The `Arc`/`Rc`/`Box` prefixed path and
  the `Allocator::allocate` impl instead keep the whole `loop { try_alloc;
  if is_oversized { ... }; refill }` body inline in the caller-visible function.
  `allocator_impl.rs:31-74` is one function containing the ZST branch, the
  alignment-ceiling rejection, the bump attempt, a full `alloc_oversized_shared_with`
  closure with its own `expect`, and the refill call. Every call site that
  inlines it (and it is generic, so it is an inlining candidate everywhere)
  receives the cold code too — icache pressure and register pressure paid on
  the hot path for code that runs once per 64 KiB.

  `alloc_smart_prefixed_with_raw` additionally wraps the caller's initialiser
  closure as `Option<F>` and does `f.take().expect("closure taken twice")` on
  the *success* path, so every `Arc`/`Rc` allocation executes a discriminant
  test and carries a panic landing pad that the `Alloc` path does not. It is
  also `#[inline(always)]`, which multiplies both costs across call sites.
- **Impact:** Medium — code size and icache rather than instruction count on
  the straight-line path; `docs/callgrind-benchmarks.md`-style instruction
  counts will barely move, but `criterion_arc_array` / `criterion_rc_array`
  wall-clock in a realistically-sized program should.
- **Remediation:** Mirror the `alloc_value_with_raw` shape: extract the
  `is_oversized` + `refill` continuation of each of the three functions into a
  `#[cold] #[inline(never)]` helper. For the `Option<F>` wrapper, hoist the
  `take()` into the cold continuation so the first (successful) attempt
  consumes `F` directly.
- **Evidence:** inferred from code reading. Confirm with
  `criterion_arc_array_cg` / `criterion_rc_array_cg`
  (`benches/criterion_arc_array_cg.rs:44`) instruction counts and, more
  tellingly, by measuring generated function size (`nm --size-sort` on the
  benchmark binary) before and after.
- **Philosophy note:** `#[inline(always)]` on `alloc_smart_prefixed_with_raw`
  is exactly the "advanced tuning knob" `docs/performance.md` rule 3 says not
  to reach for without justification; here it is applied to a function whose
  body is *large*, which is the opposite of the case that justifies it.
  However, the surrounding code has a documented convention of pairing
  `#[inline(always)]` fast paths with `#[inline(never)]` cold ones (see
  `strings/string.rs:505-509`, which does justify it in an `#[expect]` reason
  string), so the fix is to complete the pattern rather than to remove it.

#### F5. `Vec::retain_mut` uses bounds-checked indexing and a 3-move `swap` where a 1-move copy suffices

- **Location:** `crates/multitude/src/vec/mutate.rs:382-396`
- **Issue:**
  ```rust
  for read in 0..len {
      let keep = f(&mut slice[read]);
      if keep {
          if write != read { slice.swap(write, read); }
          write += 1;
      }
  }
  ```
  Three problems. (1) `slice[read]` is a bounds-checked index; LLVM can often
  prove `read < len` from the range, but `f` is an opaque `FnMut` call between
  iterations so the check is not always hoisted. (2) `slice.swap(write, read)`
  bounds-checks *both* indices and then performs a genuine three-move swap
  (`tmp = a; a = b; b = tmp`). The element at `read` is logically dead after
  the move — nothing will ever read it again — so a single
  `ptr::copy_nonoverlapping(read_ptr, write_ptr, 1)` is correct and does one
  third of the memory traffic. For a `T` of 64 bytes (e.g. `de::Entry`) that is
  128 wasted bytes of traffic per retained-and-shifted element. (3) `std`'s
  `Vec::retain_mut` is written exactly this way (raw pointer walk plus a
  `BackshiftOnDrop` guard), so this is a deviation from the ecosystem
  implementation without a stated justification, which `docs/performance.md`
  asks authors to justify.
- **Impact:** Medium — `retain`/`retain_mut` is a natural fit for the crate's
  "filter a decoded batch" story (`multitude_record_batch/sparse_retention`
  benchmarks the standard-library equivalent of this operation), and the cost
  scales with `size_of::<T>()`.
- **Remediation:** Adopt the `std` shape: iterate with raw pointers, use
  `ptr::copy_nonoverlapping` for the compaction move, and keep a drop guard so
  a panic in `f` still leaves the vector in a valid state. This preserves the
  panic-safety property the current code gets for free from `swap`. That is
  ~20 lines in one function — surgical by the house definition.
- **Evidence:** inferred from code reading. There is no benchmark for
  `retain`/`retain_mut` at all (see Benchmark coverage below); confirm by
  adding a Criterion group `criterion_vec/retain` over `u64` and over a
  64-byte payload, paired with a `_cg` Callgrind counterpart per
  `docs/naming.md`.

#### F6. `Vec::try_resize` / `try_resize_with` push element-at-a-time through a fallible, panicking `push_within_cap`

- **Location:** `crates/multitude/src/vec/mutate.rs:637-648` (`try_resize`),
  `:695-712` (`try_resize_with`)
- **Issue:** Both functions reserve the full capacity up front (correct), then
  fill with
  ```rust
  for _ in 0..added {
      guard.buf.push_within_cap(f()).ok().expect("capacity reserved above");
  }
  ```
  `push_within_cap` re-checks `len < cap` on every iteration, constructs a
  `Result`, `.ok()` converts it to an `Option`, and `.expect` installs a panic
  landing pad *inside the loop body*. The capacity check is provably redundant
  — the reservation two lines above guarantees it — and the landing pad
  inhibits LLVM from vectorising the fill for `T: Copy`, which is the case
  `resize` is overwhelmingly used for. `std`'s `Vec::resize_with` uses
  `extend_with` / `SetLenOnDrop`, writing through a raw pointer with no
  per-element branch. `try_resize` also clones `value` per element (correct and
  necessary) and consumes the original on the last push (a nice touch).
- **Impact:** Medium — `resize`/`resize_with` are the canonical way to
  pre-size a decode buffer, which is a design-target workload for this crate.
  The per-element branch is small in absolute terms but blocks the
  memset/vector-store the compiler would otherwise emit.
- **Remediation:** Keep the `ResizeGuard` (it is the defensive
  panic-safety mechanism `docs/performance.md` says to preserve) but replace
  the loop body with a raw-pointer write that bumps the guard's length,
  hoisting the capacity assertion to a single `debug_assert!` before the loop.
- **Evidence:** inferred from code reading. No `resize` benchmark exists;
  confirm with a new `criterion_vec/resize` group over `u64` and a non-`Copy`
  type, plus its `_cg` pair, comparing against `bumpalo`'s `Vec::resize` as the
  existing benchmarks do for other operations.

#### F7. UTF-16 transcoding from `&str` walks the input twice

- **Location:** `crates/multitude/src/arena/alloc_utf16.rs:396-402`
  (`impl_alloc_utf16_prefixed_from_str`), `:454-460`
  (`alloc_utf16_prefixed_from_str_raw`), and the one-`u16`-at-a-time inner loop
  at `:493-505` (`transcode_utf16_into`)
- **Issue:** Every `alloc_utf16_str_{arc,rc,box}_from_str` first computes the
  exact output length with
  `s.chars().try_fold(0usize, |acc, c| acc.checked_add(c.len_utf16()))` — a
  complete UTF-8 decode of the input — and then transcodes with
  `s.encode_utf16()`, a second complete UTF-8 decode. The input is therefore
  scanned twice end to end. For a bump allocator this pre-walk is avoidable:
  `s.len()` (bytes) is always a valid *upper bound* on the UTF-16 unit count
  (1-byte→1 unit, 2-byte→1, 3-byte→1, 4-byte→2 units for 4 bytes), so the
  allocator can reserve `s.len()` units, transcode in a single pass, and then
  shrink. The crate already supports in-place shrink of the most recent
  allocation — `Allocator::shrink` at `allocator_impl.rs:143-149` returns the
  block unchanged, and `criterion_alloc/allocator_grow` benchmarks
  `shrink_in_place` — so the tail can be reclaimed or simply left as bump
  slack. The pre-walk is only strictly needed because the length is written
  into the allocation-resident prefix *before* the payload; that ordering can
  be inverted (write the payload, then back-patch the prefix).
  Separately, `transcode_utf16_into` writes one `u16` per loop iteration with
  no ASCII fast path; the common case (all-ASCII input) can be widened 8 or 16
  bytes at a time.
- **Impact:** Medium — halves the input scan for what is a Windows-interop
  design-target workload. The upper-bound reservation costs at most
  `s.len() - len_utf16` unused `u16` slots, which for ASCII is zero and for
  worst-case CJK is 0 (3-byte→1 unit means 3 bytes reserved, 1 used: 2 units
  of slack per char) — bounded and reclaimable, and bump slack is exactly what
  arenas are for.
- **Remediation:** Reserve `s.len()` units, transcode single-pass while
  counting, back-patch the length prefix, and shrink the bump cursor by the
  unused tail. Keep the two-pass path for the `Utf16String::push_from_str`
  growth case where the destination is not the most recent allocation. Treat
  the ASCII widening separately and only if a benchmark motivates it.
- **Evidence:** inferred from code reading. **There is currently no UTF-16
  benchmark of any kind** (see Benchmark coverage), so this cannot be confirmed
  without first adding one: a `criterion_utf16/from_str` group over ASCII,
  Latin-1, CJK and emoji inputs, paired with a `_cg` Callgrind file per
  `docs/naming.md`.
- **Philosophy note:** This is closer to an architectural change than a
  surgical one (it reorders prefix/payload initialisation). Flagged
  accordingly — it should not land without the benchmark that motivates it.
#### F8. `ChunkProvider` interleaves the owner-read configuration with the cross-thread-written cache head and stats counters on the same cache lines

- **Location:** `crates/multitude/src/internal/chunk_provider.rs:126-181`
- **Issue:** Field order is `allocator: Arc<A>` (offset 0),
  `config: ChunkProviderConfig` (8), `weak_self: Weak<Self>` (24),
  `bytes_outstanding: AtomicUsize` (32), `[stats] allocated_bytes` (40),
  `cache: AtomicPtr<u8>` (48), `cache_class: AtomicU8` (56), then eleven
  `#[cfg(feature = "stats")]` atomics. Measured sizes: 56 bytes without
  `stats`, **152 bytes with** `stats` — spanning three cache lines.

  The problem is the first line. `cache` is a Treiber-stack head that **any**
  thread may `compare_exchange` into (the doc comment at `:139-141` says so
  explicitly: an escaped `Arc`/`Box` handle dropped on a foreign thread pushes
  its chunk back here). `bytes_outstanding` is likewise decremented by foreign
  threads via `release_bytes`. Both sit in the same 64-byte line as
  `allocator`, `config` and `weak_self`, which the *owning* thread reads on
  every refill. So every foreign-thread chunk return invalidates the owner's
  copy of its own immutable configuration — textbook false sharing between
  read-mostly and write-hot data.
- **Impact:** Medium in the multi-threaded escape scenario the design
  explicitly supports; zero in the single-threaded case. Importantly, **no
  benchmark in the crate is multi-threaded**, so this cost is currently
  invisible to the repo's numbers.
- **Remediation:** Surgical and non-behavioural: reorder the struct so the
  read-mostly fields (`allocator`, `config`, `weak_self`) are grouped and the
  cross-thread-written fields (`bytes_outstanding`, `cache`, `cache_class`) are
  separated from them, ideally by a `#[repr(align(64))]` wrapper or explicit
  padding on the written group. `ChunkProvider` is allocated once per arena, so
  padding to 128 or 192 bytes costs nothing measurable. Note the whole struct
  is `pub(crate)`, so this is invisible to the public API.
- **Evidence:** empirically verified for the *layout* (standalone `rustc`
  layout probe: 56 B without stats, 152 B with stats, `cache` at offset 48 in
  the same line as `config` at 8). The *contention* consequence is inferred;
  confirm by adding a multi-threaded benchmark (N threads allocating from their
  own arenas but dropping handles onto a shared consumer thread) and measuring
  with `perf c2c` or Callgrind's cache simulation.

#### F9. `Chunk`'s cross-thread refcount shares a cache line with the owner-read capacity

- **Location:** `crates/multitude/src/internal/chunk.rs:42-61`
- **Issue:** `#[repr(C)] struct Chunk<A> { allocator: Arc<A>, provider: Weak<ChunkProvider<A>>, capacity: usize, ref_count: AtomicUsize, next: AtomicPtr<u8>, [stats] wasted_at_retire: AtomicU32, data: [UnsafeCell<u8>] }`.
  Measured header size: 40 bytes (48 with `stats`). Offsets: `capacity` at 16,
  `ref_count` at 24, `next` at 32 — all inside the first 64-byte line, which
  also holds `allocator` and `provider`. `ref_count` is decremented by any
  thread dropping an escaped handle; `capacity`, `allocator` and `provider` are
  read by the owner on retire/footprint paths. Same false-sharing shape as F8.

  This one is *much* harder to fix than F8 and I recommend against fixing it:
  the header is `repr(C)` because smart pointers recover it by masking the
  value pointer down to the `CHUNK_ALIGN` boundary, and padding it out to a
  cache line per group would waste 64+ bytes of every chunk and, worse, change
  where the payload starts. Note also that this is a per-chunk (64 KiB), not
  per-allocation, cadence.
- **Impact:** Low — real but rare, and the fix costs more than the problem.
- **Remediation:** Recommend **no change**. Documented here so the next person
  who profiles a cross-thread-drop workload and sees the line bouncing knows
  the tradeoff was considered. If it ever does show up, the cheap half-measure
  is to move `ref_count` to the *end* of the sized header (after `next` and
  `wasted_at_retire`) so it at least does not share a line with `allocator` and
  `provider` once `stats` pushes the header past 48 bytes.
- **Evidence:** empirically verified (layout probe: header 40 B / 48 B with
  stats, field offsets as stated). Contention consequence inferred.

#### F10. `Chunk::teardown_and_release` performs a `Weak::upgrade` per chunk release

- **Location:** `crates/multitude/src/internal/chunk.rs:412-421`
- **Issue:** Releasing a chunk calls `self.provider.upgrade()` to get back to
  the `ChunkProvider`. `Weak::upgrade` is a `compare_exchange` loop on the
  strong count, and the resulting `Arc` is dropped immediately afterwards,
  costing a second atomic decrement. So each chunk release pays two atomic RMWs
  purely to traverse a back-pointer. The `Weak` (rather than `Arc`) is required
  to break the provider↔chunk cycle, so it cannot simply become an `Arc`.
- **Impact:** Low-to-Medium — chunk cadence, and `multitude_teardown` is
  explicitly one of the crate's benchmark families, so this shows up there.
  `docs/performance.md` explicitly deprioritises teardown optimisation, which
  caps the value of fixing it.
- **Remediation:** If measurement justifies it, hold the upgraded `Arc` across
  the whole teardown of a chunk *list* rather than upgrading per chunk — the
  retired-local list (`arena/retired_local.rs`) is drained in a loop, so one
  upgrade for the whole drain replaces N. That is a surgical change confined to
  the drain loop.
- **Evidence:** inferred from code reading. Confirm with
  `benches/multitude_teardown_cg.rs` (groups at `:61-73`) — the instruction
  delta should scale with chunk count.
- **Philosophy note:** `docs/performance.md` says to deprioritise teardown
  optimisations. Flagged: this finding is deliberately low priority and is
  recorded for completeness, not proposed for action.

#### F11. `ArenaBuf`'s `freeze_prefix: bool` costs 8 bytes of padding in every arena `Vec` and `String`

- **Location:** `crates/multitude/src/internal/arena_buf.rs:29-40`
- **Issue:** `ArenaBuf<T, A>` is `{ ptr: NonNull<T>, len: usize, cap: usize, freeze_prefix: bool, _marker }`. The trailing `bool` forces the struct to
  32 bytes where the three pointer-sized fields alone would be 24 — 8 bytes
  of pure padding, 25 % overhead. `Vec<'a, T, A>` wraps that plus an
  `&'a Arena<A>`, giving **40 bytes** against `alloc::vec::Vec`'s 24. Measured:

  | type | actual | without `freeze_prefix` |
  |---|---|---|
  | `ArenaBuf<u8>` | 32 B | 24 B |
  | `Vec<'_, u8>` | **40 B** | 32 B |

  The flag records whether the buffer was allocated with a smart-pointer length
  prefix so it can later be frozen into a `Box`/`Arc`/`Rc` in place. It is one
  bit of information.
- **Impact:** Low-to-Medium — `Vec` is usually a stack local or a field, and
  40 vs 32 bytes matters mainly when many are held (a `Vec<Vec<T>>`-shaped
  decode, or the per-field `Option<T>` locals the `DeserializeIn` derive
  generates). It also means `Vec` no longer fits in three registers for return
  values.
- **Remediation:** Encode the flag in a spare low bit of `cap` (capacities are
  always multiples of `align_of::<T>()` for `T` with align > 1, and for
  `align_of::<T>() == 1` the top bit of `cap` is always free because a capacity
  of `usize::MAX/2` is unreachable), or in a spare bit of `ptr` via
  tagged-pointer helpers. Both are contained to `arena_buf.rs` behind the
  existing accessors, so no caller changes. Weigh against readability: this is
  the kind of trick `docs/performance.md`'s "stay idiomatic" guidance pushes
  back on, so it needs a measurement first.
- **Evidence:** empirically verified (layout probe, numbers above). Performance
  consequence inferred; confirm with `criterion_alloc/vec_builder`
  (`benches/criterion_alloc.rs:441`) and a new benchmark that holds many live
  `Vec`s at once.
- **Philosophy note:** Bit-packing a `bool` into a pointer or capacity is a
  deviation from idiomatic Rust that `docs/performance.md` asks be justified.
  Flagged as such; do not do it on the strength of the layout number alone.

#### F12. `cow.rs` has no `#[inline]` anywhere and `to_mut` recurses instead of matching

- **Location:** `crates/multitude/src/cow.rs:80-106` (`to_mut`, `try_to_mut`);
  whole file for the `#[inline]` observation (20 public functions, 0
  `#[inline]` attributes — the only such file in the crate's public surface)
- **Issue:** Two separate observations.
  (a) `to_mut` / `try_to_mut` handle the `Borrowed` case by replacing `*self`
  with an owned variant and then **calling themselves again** to take the
  `Owned` branch. The recursion is a tail call that LLVM will almost certainly
  turn into a loop, but the resulting code has a branch that can provably never
  be taken twice, and it prevents the function from being a candidate for the
  simple `match self { Owned(v) => v, Borrowed(_) => unreachable }` shape that
  `alloc::borrow::Cow` uses. It also means the borrow checker forces a
  re-discriminant-check that the ecosystem implementation avoids.
  (b) Every function in the file is generic over `T` and `A` (they live in
  `impl<'a, T, A>`), so per `docs/performance.md` rule 2 they are *already*
  inlining candidates and the missing `#[inline]` is not automatically a defect.
  I am recording it because `cow.rs` is the one conspicuous outlier in a crate
  with otherwise excellent inline density (716 `#[inline]` against 407 public
  functions), which suggests the file was simply not swept rather than
  deliberately left alone.
- **Impact:** Low — the recursion is very likely optimised away, and the
  `#[inline]` half is speculative by the crate's own standard.
- **Remediation:** For (a), restructure to the `std` shape:
  ```rust
  if let Self::Borrowed(b) = *self { *self = Self::Owned(b.to_owned_in(self.arena)); }
  match self { Self::Owned(o) => o, Self::Borrowed(_) => unreachable!("just replaced above") }
  ```
  preserving the `unreachable!` as the defensive check `docs/performance.md`
  asks for. For (b), do nothing without a measurement.
- **Evidence:** inferred from code reading. **There is no `Cow` benchmark at
  all**; confirm by adding a `criterion_cow/to_mut` group (borrowed-then-mutate
  and already-owned cases) with a `_cg` pair.
- **Philosophy note:** part (b) explicitly does *not* meet the bar in
  `docs/performance.md` rule 2 (no measurement showing the default inlining
  decision is wrong). Reported as an observation, not a recommendation.

#### F13. `<&Arena<A> as Allocator>` has no `#[inline]` while its own forwarding shims do

- **Location:** `crates/multitude/src/allocator_impl.rs:30-168` (the
  `allocator_api2` 0.4 impl: `allocate` `:31`, `deallocate` `:76`, `grow` `:89`,
  `grow_zeroed` `:118`, `shrink` `:143` — none annotated) versus `:170-230`
  (the `allocator_api2_02` bridge, where all five forwarding one-liners carry
  `#[inline]`)
- **Issue:** The file contains exactly five `#[inline]` attributes and all five
  are on the trivial forwarding shims that delegate to the *unannotated* real
  implementation. That is backwards from the usual intent: the shims would be
  inlined anyway (they are one-line delegations), while the real `allocate` is
  the entry point every `hashbrown`, `allocator_api2::vec::Vec` and
  `Box::new_in` allocation goes through. The asymmetry looks accidental rather
  than reasoned.
- **Impact:** Low-to-Medium, and genuinely uncertain — see the philosophy note.
- **Remediation:** Consider `#[inline]` on `deallocate`, `grow`, `grow_zeroed`
  and `shrink` (all small once F4's cold split is applied to `allocate`). Do
  *not* apply it to `allocate` in its current form; fix F4 first so there is a
  small body worth inlining.
- **Evidence:** inferred from code reading. Confirm with
  `criterion_arena_vs_allocator` (`benches/criterion_arena_vs_allocator.rs:81`),
  which is precisely the benchmark that exercises this trait — but note that
  `[profile.bench]`'s `lto = "fat"` will hide the difference entirely, so the
  measurement must be run with a `release`-like profile (`lto` off,
  `codegen-units = 16`) to be meaningful.
- **Philosophy note:** These functions are generic (`impl<A: Allocator + Clone>`),
  so `docs/performance.md` rule 1 does *not* apply and rule 2 requires
  measurement first. Flagged: this is an observation about an internal
  inconsistency, not a licence to sprinkle `#[inline]`. It is also the clearest
  instance in my crates of the workspace-level problem g9 identified — the
  benchmark profile's fat LTO makes every `#[inline]` question unanswerable
  with the repo's own tooling.

#### F14. `stats` is a default-off feature whose cost is nevertheless in every published benchmark number

- **Location:** `crates/multitude/Cargo.toml` (feature `stats`);
  counters at `crates/multitude/src/internal/chunk_provider.rs:143-181`,
  `:600-620`; arena-side counters in `crates/multitude/src/arena/mod.rs`;
  bench-side conditional at `crates/multitude/benches/multitude_record_batch.rs:15,25,164-176`
- **Issue:** With `stats` on, `ChunkProvider` grows from 56 to 152 bytes
  (layout probe), the `Chunk` header from 40 to 48 bytes, and the acquire /
  release / retire paths each execute several additional atomic RMWs
  (`record_allocation` alone does a `fetch_add`, a `fetch_add` and a
  `fetch_max` — `chunk_provider.rs:615-619`). The feature is correctly
  default-off. However, per g9's round-1 workspace finding, benchmarks are
  built with `--all-features`, which turns `stats` **on**. So every number the
  repository reports for `multitude` includes counter overhead that no default
  consumer pays, and no benchmark measures `stats` on versus off. Only
  `multitude_record_batch` even acknowledges the feature, and only to add an
  extra `arena_stats_snapshot` case.
- **Impact:** Medium — this is a measurement-validity problem rather than a
  code defect. It biases every comparison against `bumpalo` and `std` (neither
  of which has an equivalent counter set enabled) in *multitude's* disfavour,
  and it means any regression introduced on the `stats`-off path could go
  unnoticed.
- **Remediation:** Benchmark `multitude` with its default feature set (or add
  an explicit `stats`-off benchmark configuration) so the headline numbers
  reflect what consumers get. Optionally add a small paired benchmark
  (`criterion_alloc/stats_overhead`) that reports the cost of the counters
  explicitly, so the price of turning them on is a documented number rather
  than folklore.
- **Evidence:** layout portion empirically verified (probe: `ChunkProvider`
  56 B → 152 B; `Chunk` header 40 B → 48 B). The `--all-features` benchmark
  policy is reported by g9 (workspace group). The runtime cost is inferred;
  confirm by running any `_cg` benchmark twice, once with `--features stats`
  and once without, and diffing instruction counts.

#### F15. Generic `impl AsRef<...>` parameters on high-arity public APIs multiply monomorphisations

- **Location:** `crates/multitude/src/strings/string.rs:493`
  (`push_str(&mut self, s: impl AsRef<str>)`),
  `crates/multitude/src/arena/alloc_utf16.rs:40,67,88,112,145,169,193,220,241,265,283,307`
  (twelve `impl AsRef<widestring::Utf16Str>` / `impl AsRef<str>` entry points)
- **Issue:** `impl AsRef<T>` in argument position is ergonomic but produces a
  distinct monomorphisation per argument type at every call site, on top of the
  existing `A: Allocator + Clone` type parameter. `alloc_utf16.rs` alone exposes
  twelve such entry points, each already generic in `A`; a consumer that passes
  `&str`, `String`, `&String` and `Cow<str>` to the same function gets four
  codegen'd copies per allocator. `std` deliberately takes `&str` on
  `String::push_str` for exactly this reason.
- **Impact:** Low — compile time and binary size, not runtime; and each
  instantiation is small. Recorded because compile-time cost is in scope and
  because it interacts with F4 (`#[inline(always)]` on a large body multiplies
  across these).
- **Remediation:** No change recommended for the existing API (it is a
  breaking change for marginal benefit). Worth noting as a design guideline for
  future additions: prefer `&str` / `&Utf16Str` and let callers deref.
- **Evidence:** inferred from code reading. Confirm with
  `cargo llvm-lines -p multitude` or by counting symbols in the benchmark
  binary — neither is runnable here.

### Benchmark coverage

**What is covered.** 23 files under `crates/multitude/benches/`, organised as
Criterion / Callgrind (`_cg`) pairs per `docs/naming.md`. Coverage is unusually
thorough for the paths it does cover:

| family | Criterion | Callgrind pair | what it measures |
|---|---|---|---|
| `criterion_alloc` | ✅ `criterion_alloc.rs` | ✅ `criterion_alloc_cg.rs` | arena lifecycle (`multitude_new` vs `bumpalo_new`); `alloc_u64` scalar; `alloc_str`; `alloc_slice` across `Box`/`Arc`/`Rc` × copy/clone/fill_with/fill_iter/uninit/zeroed (24 cases); `string_builder` with and without capacity; `vec_builder` with and without capacity; `allocator_grow` in-place / zeroed / shrink. Nearly all cases have a `bumpalo` comparison arm. |
| `criterion_arc_array` | ✅ | ✅ | `Arc<[T]>` construction, arena vs global, including `try_into_arc_slice` freeze |
| `criterion_rc_array` | ✅ | ✅ | ditto for `Rc<[T]>` |
| `criterion_drop` | ✅ | ✅ | `drop` across many payload shapes; `clone` for `Rc<u64>` / `Arc<u64>` |
| `criterion_arena_vs_allocator` | ✅ | ❌ **no `_cg` pair** | `&Arena` used through the `Allocator` trait vs the system allocator, with `reset` in the loop |
| `multitude_serde` | ✅ | ✅ | typed and dynamic deserialization vs `serde_json`; typed and batch lifecycle vs `serde_json` and `bumpalo` |
| `multitude_record_batch` | ✅ | ✅ (21 library benchmarks, 7 groups) | decode into `Vec` / `Box<[T]>`; escaped vs unescaped strings; reuse with and without `reset`; sparse retention; lazy raw strings; error and resource-limited paths; stats snapshot; a five-way "refresh workload" comparison |
| `multitude_teardown` | ✅ | ✅ | teardown at several element counts, vs `std` and `bumpalo` |

That is a strong baseline. The `bumpalo` / `std` / `serde_json` comparison arms
in particular are exactly the "is this actually better than the alternative"
discipline `docs/performance.md` asks for.

**Gaps — perf-critical operations with no benchmark at all.** Verified by
grepping the whole `benches/` tree for the relevant identifiers:

1. **Anything multi-threaded.** There is no `std::thread::spawn` anywhere in
   `benches/`. Yet cross-thread handle escape is a *designed-for* scenario:
   `ChunkProvider::cache` is documented as "any thread may push"
   (`chunk_provider.rs:139-141`), `Chunk::ref_count` is an `AtomicUsize`
   precisely so foreign threads can drop handles, and
   `LARGE_SHARED_REF_SURPLUS` exists to make the single-threaded case cheap
   *in the presence of* that capability. The contended Treiber-stack push,
   the cross-thread `Arc` drop, and the false sharing in F8 are all
   structurally unmeasurable today. **This is the single biggest coverage
   gap.**
2. **The entire `utf16` feature.** `strings/utf16_string.rs` (~1600 lines) and
   `arena/alloc_utf16.rs` (~510 lines) have zero benchmarks. F7 cannot be
   validated without adding one.
3. **`Cow`** (`cow.rs`, 368 lines, 20 public functions) — zero benchmarks.
4. **`Vec` mutation operations.** `vec/mutate.rs` (retain, retain_mut,
   resize, resize_with, dedup, split_off, insert, remove), `vec/splice.rs` and
   `vec/drain.rs` have no benchmarks. Only construction (`vec_builder`) and the
   growth path are covered. F5 and F6 both live here.
5. **`dst` / unsized coercion.** `arena/alloc_unsized.rs` (~900 lines) and
   `dst/` are unbenchmarked; `Arc<dyn Trait>` clone/drop in particular goes
   through `align_of_val` on a vtable, which is the one case where that call
   is not constant-folded.
6. **The `zerocopy`, `bytemuck`, `bytes` and `bytesbuf` integrations**
   (`zerocopy.rs` 449 lines, `bytemuck.rs` 452, `bytesbuf.rs` 177,
   `bytes.rs` 59) — zero benchmarks. These are zero-copy view APIs, i.e.
   exactly the kind of thing whose whole justification is performance.
7. **`hashbrown`-in-arena** — the `hashbrown` feature has no benchmark;
   `criterion_arena_vs_allocator` covers the `Allocator` trait generically but
   not a hash-table workload, which has a very different allocation size
   distribution.
8. **`stats` on vs off** — see F14.
9. **`Arena::reset` in isolation.** `reset` appears inside other benchmarks'
   setup (`criterion_arena_vs_allocator.rs:96,102`,
   `multitude_teardown/shared.rs:47`, `multitude_alloc_common/mod.rs:55,63`)
   but is never the measured operation, so the cost of the retired-chunk drain
   and the size-class ratchet is never reported on its own.
10. **Chunk refill / size-class growth.** No benchmark deliberately crosses
    chunk boundaries a controlled number of times, so F2 and F10 are measurable
    only indirectly through teardown numbers.
11. **Missing `_cg` pair:** `criterion_arena_vs_allocator.rs` has no
    `criterion_arena_vs_allocator_cg.rs`, breaking the pairing convention in
    `docs/naming.md` that every other family in the crate follows.

**Cross-cutting measurement-validity issues** (both originally reported by g9
at workspace level; restating because they determine what my findings can and
cannot be confirmed with):

- `[profile.bench]` sets `lto = "fat"` + `codegen-units = 1` while
  `[profile.release]` sets neither. Every number this crate publishes comes
  from a build no consumer gets, and fat LTO makes the benchmarks structurally
  blind to *every* `#[inline]`-related question — including F13, which is
  otherwise directly testable.
- `just bench` and `just bench-cg` are referenced by `docs/performance.md:29`
  and `docs/callgrind-benchmarks.md:10,153` but do not exist as recipes, so the
  verification procedure the guidelines prescribe cannot be followed as written.

### Considered and ruled out

Things I specifically checked in `multitude` and found to be fine:

- **No hidden allocation on any hot path.** Grepped all non-test `src/` for
  `format!`, `to_string()`, `.collect()`, `vec![`, `String::new`,
  `Box::new` — zero occurrences outside tests and doc examples. This is
  unusually clean and directly satisfies `docs/performance.md`'s
  no-allocation-on-the-hot-path rule.
- **The bump allocator itself.** `ChunkMutator::try_alloc`
  (`internal/chunk_mutator.rs:152-186`) is about as tight as it can be: a
  `hint::assert_unchecked` removes the overflow check from the alignment
  round-up on 64-bit targets, the ZST case is folded into a `size.max(1)`
  probe rather than a branch, and the failure path returns `None` for the
  caller's cold continuation. No change proposed.
- **`LARGE_SHARED_REF_SURPLUS`** (`arena/mod.rs:41`, used by
  `acquire_current_chunk_ref` `:694-703`, reconciled at `:740-760`). The chunk
  refcount is pre-credited by `1 << 30` when the chunk is installed so that
  per-allocation handout is a non-atomic `Cell<u32>` bump, reconciled with one
  `fetch_sub` at retire. This is excellent and I could not improve on it.
- **Thin 8-byte smart pointers with allocation-resident metadata**
  (`internal/thin_dst.rs`, `thin_smart_ptr_common.rs`, `internal/chunk_ref.rs`).
  The trade — one extra load to reach the length/count versus halving the
  handle size — is deliberate, documented, and correct for a crate whose whole
  premise is packing many handles into an arena.
- **`Vec` growth.** `Vec::try_grow_to` (`vec/mod.rs:126-209`) is correctly
  `#[cold] #[inline(never)]`, tries in-place bump extension first, and
  `grow_target` is proper amortised doubling. `push_within_cap`
  (`internal/arena_buf.rs:142-155`) is a clean capacity-checked fast path.
  `internal/arena_buf.rs:205,241,303,329` all use `ptr::copy_nonoverlapping` /
  `ptr::copy` correctly rather than element loops.
- **Serde sequence deserialization** does the right thing with `size_hint`:
  `de/containers.rs:437-452` reserves from the hint and then grows amortised,
  and `de/limits.rs:412-417` correctly clamps the forwarded `size_hint` by the
  remaining sequence budget so a malicious hint cannot cause a huge reservation.
- **`DeserializationLimits` enforcement** (`de/limits.rs`). Well built: every
  check is guarded by a `usize::MAX` sentinel fast-out (`:77-80`, `:391-393`),
  `LimitState` is shared by `&` rather than copied into each wrapper (the
  struct is 40 bytes and Copy, so copying it per nesting level would have been
  a real cost), `reject` is `#[cold]`, and depth is a `Cell<usize>` with a
  `DepthGuard` rather than an atomic. Crucially the *unlimited* API path
  (`de/json.rs:174-187`, `de/mod.rs`) bypasses the whole wrapper stack rather
  than passing `unlimited()` through it, so users who do not want limits pay
  nothing — including no extra monomorphisation.
- **`Allocator::grow` / `grow_zeroed` / `shrink`**
  (`allocator_impl.rs:89-149`) all have correct in-place fast paths;
  `shrink` correctly keeps the larger block instead of reallocating, which is
  the right call for a bump allocator.
- **`de::Map` is a `Box<[Entry<A>], A>`** (`de/value/map.rs:25`), i.e. an
  ordered vector with O(n) lookup and duplicate keys preserved. That is a
  deliberate, documented choice matching `serde_json`'s `preserve_order` mode
  and the right one for a decode-once-traverse-once workload; a hash map would
  cost more to build than it saves. No change proposed. (The 64-byte `Entry`
  size is F1's problem, not this one's.)
- **`strings::String::try_push_str`** (`strings/string.rs:505-509`) — the
  `#[inline(always)]` here *is* justified, and the `#[expect]` reason string
  says exactly why (small bump-then-memcpy body, cold grow branch is
  `#[inline(never)]`). This is the pattern F4 asks the `Arc`/`Rc`/`Box` paths
  to adopt, not a defect.
- **`strings::format!`** (`strings/format_macro.rs:24-34`) starts from a
  zero-capacity arena `String` with no size hint, unlike `std`'s `format!`
  which estimates. I initially flagged this, then ruled it out: because growth
  happens in place at the bump cursor whenever the string is the most recent
  allocation, the reallocation `std` is avoiding mostly does not occur here.
- **`#[inline]` density generally.** 716 `#[inline]` attributes against 407
  public functions. Per `docs/performance.md` rules 1 and 2, and given that
  almost everything in the crate is generic in `A`, I found no case worth
  proposing on inline grounds alone. `cow.rs` (F12b) and `allocator_impl.rs`
  (F13) are recorded as inconsistencies, explicitly flagged as not meeting the
  measurement bar.
- **Atomic orderings on the chunk refcount** (`internal/chunk.rs:286-402`) —
  these are the orderings that actually publish chunk contents and they are
  correct (`Release` on decrement, `Acquire` fence before teardown, the
  standard `Arc` pattern). Only the *counters* in F3 are over-strong.
- **`de::Value` recursion depth.** `Value` is recursively `Box`ed rather than
  flattened into an arena-side arena-of-nodes; combined with the depth limit
  this is safe and the indirection is only 8 bytes per level thanks to the thin
  pointers. No change proposed.

---
## Crate: multitude_macros

### Summary

`multitude_macros` is a 21-line `proc-macro` shim
(`crates/multitude_macros/src/lib.rs`). Its only content is
`#[proc_macro_derive(DeserializeIn, attributes(serde, multitude))]`, which
builds a `syn::Path` for `::multitude::de` and forwards everything to
`multitude_macros_impl::derive_deserialize_in`. There is no runtime code and no
generated code originating here. The only performance dimension available is
downstream compile time, and the only lever is the crate's dependency
declaration.

### Findings

#### F16. `syn` is declared with the full heavy feature set, including `extra-traits`, for a 21-line shim that uses `Path` and `parse_quote` only

- **Location:** `crates/multitude_macros/Cargo.toml:27`
  (`syn = { workspace = true, features = ["full", "derive", "printing", "parsing", "extra-traits", "proc-macro", "clone-impls"] }`);
  the crate's entire use of `syn` is `crates/multitude_macros/src/lib.rs:13,17`
  (`use syn::{Path, parse_quote};` and one `parse_quote!(::multitude::de)`)
- **Issue:** The workspace declares `syn = { version = "3.0.2", default-features = false }`
  (root `Cargo.toml:207`) so each consumer opts into features explicitly, which
  is the right design. This crate opts into all seven. `extra-traits` is the
  expensive one: it generates `Debug`, `Eq`, `PartialEq` and `Hash`
  implementations for every node in `syn`'s AST, and is a well-known
  significant contributor to `syn`'s own compile time. `full` (support for
  parsing whole items and statements, not just derive input) is also not needed
  by a shim that parses nothing.

  Cargo unifies features across a dependency graph, so as long as
  `multitude_macros_impl` requests the same set the practical cost in a
  workspace build is nil — this is why I rate it Low. It matters for the
  declaration's honesty and for the case where the impl crate's own request is
  trimmed (F17): the shim would then silently keep `extra-traits` alive for
  everyone.
- **Impact:** Low — no runtime cost; downstream compile time only, and
  currently masked by feature unification with `multitude_macros_impl`.
- **Remediation:** Reduce to what the file uses:
  `features = ["parsing", "printing", "proc-macro", "derive", "clone-impls"]`,
  and drop `full` and `extra-traits`. Do this together with F17 or it has no
  effect.
- **Evidence:** inferred from code reading (the file is 21 lines; I read all of
  it). Confirm with `cargo build --timings -p multitude_macros` on a clean
  target directory, comparing the `syn` bar before and after — not runnable in
  this container.

### Benchmark coverage

There is no `benches/` directory and none is warranted: the crate contains no
runtime code. The meaningful measurement for a proc-macro crate is compile
time, for which the ecosystem tool is `cargo build --timings` /
`cargo llvm-lines` on a *consumer*, not a Criterion benchmark. The crate has no
dev-dependency on `criterion` and should not acquire one.

The gap worth noting is that **nothing in the repository tracks the compile-time
cost this derive imposes on consumers**. `docs/benchmarks.md` and
`docs/callgrind-benchmarks.md` are entirely about runtime. A cheap improvement
would be a CI check that compiles a fixture crate with N derived structs and
fails on a large regression, but that is a workspace-level tooling question
(g9's territory) rather than something this crate can fix alone.

### Considered and ruled out

- **The shim itself is optimal.** `derive_deserialize_in` does exactly one
  thing per invocation: `parse_quote!(::multitude::de)` to build the root path,
  then delegate. There is no parsing, no cloning of the token stream beyond the
  mandatory `TokenStream` ↔ `TokenStream2` conversions, and no per-invocation
  allocation that could be avoided.
- **`#[cfg_attr(test, mutants::skip)]`** on the entry point (`lib.rs:16`) is
  test tooling with no release-build cost.
- **Splitting the proc-macro into a shim plus an `_impl` crate** is the
  workspace's consistent convention (`fundle_macros`, `internity_macros`,
  `thread_aware_macros` all do it) and is the right structure: it lets the
  logic be unit-tested without a `proc-macro` crate type. It does mean one
  extra crate in the build graph, but the two compile in sequence anyway.
  No change proposed.

---

## Crate: multitude_macros_impl

### Summary

`multitude_macros_impl` (1908 lines in `src/lib.rs`, 642 in `src/attrs.rs`)
contains the real `DeserializeIn` derive: attribute parsing for both
`#[serde(...)]` and `#[multitude(...)]`, generic-parameter and lifetime
synthesis, where-clause inference, and code generation for structs (named,
tuple, unit) and enums, covering map, sequence, ordinal and identifier
deserialization paths. Two performance dimensions apply: the compile-time cost
the derive imposes on downstream crates, and the runtime efficiency of the code
it emits. Both are in good shape — the generated code closely tracks
`serde_derive`'s well-optimised shape, and the expansion logic is
single-pass with no obvious quadratic behaviour.

### Findings

#### F17. `syn`'s `extra-traits` feature is enabled but nothing in the crate needs it

- **Location:** `crates/multitude_macros_impl/Cargo.toml:36`
- **Issue:** `extra-traits` exists to provide `Debug`/`Eq`/`PartialEq`/`Hash`
  on `syn`'s AST types. Searching both source files, the crate never derives or
  requires any of those on a `syn` type: the only `derive`s are
  `#[derive(Clone)]`, `#[derive(Default, Clone)]`, `#[derive(Clone, Copy)]` and
  `#[derive(Default)]` on the crate's *own* types (`attrs.rs:7,13,25,105`,
  `lib.rs:41,1228`), and there is no `{:?}` formatting of a `syn` node
  anywhere. Where the crate needs to compare or hash types it deliberately
  routes through a rendered string instead —
  `let key = ty.to_token_stream().to_string();` at `lib.rs:273` and `:281`,
  keyed into a `HashSet<(String, &str)>` — precisely the workaround one writes
  *because* `extra-traits` is unavailable. So the feature appears to be
  vestigial.
- **Impact:** Low-to-Medium on downstream clean-build compile time; zero at
  runtime. `extra-traits` is one of the larger single contributors to `syn`'s
  compile time. Note the same feature is requested by seven other macro crates
  in this workspace (`fundle_macros_impl`, `internity_macros`,
  `internity_macros_impl`, `multitude_macros`, `ohno_macros`,
  `thread_aware_macros`, `thread_aware_macros_impl`), so within a workspace
  build feature unification makes any single crate's change ineffective — this
  is really a workspace-wide cleanup and should be coordinated with g9. For a
  downstream consumer that depends only on `multitude`, though, trimming these
  two crates is sufficient and does help.
- **Remediation:** Drop `extra-traits` (and probably `full`, though `full` is
  more likely genuinely needed for parsing `WherePredicate` and general
  expressions in `#[multitude(...)]` attributes — verify before removing).
  Coordinate with the equivalent change in the other seven crates.
- **Evidence:** inferred from code reading (grepped both files for `Debug`,
  `{:?}` and `derive(` — results as stated). Confirm with
  `cargo build --timings` on a clean target directory for a fixture crate
  depending only on `multitude` with the `serde` feature.

#### F18. Type-identity deduplication for where-clause synthesis renders each type to a `String`

- **Location:** `crates/multitude_macros_impl/src/lib.rs:267-299` (the
  `seen: HashSet<(String, &str)>` loop; keys built at `:273` and `:281`)
- **Issue:** For every field, the code does
  `let key = ty.to_token_stream().to_string();` — a full `quote` render of the
  field's type into a `TokenStream2` followed by a `String` allocation — to
  deduplicate the where-clause predicates it emits. For a struct with `n`
  fields this is `n` (or `2n`, since `:273` and `:281` each compute a key)
  token-stream renders plus `String` allocations, per derived type, per
  compilation. The rendered strings are then thrown away.
- **Impact:** Low — bounded by field count, and a `syn::Type` render is cheap
  relative to the rest of the expansion. Recorded because compile-time cost is
  explicitly in scope for this crate and because the fix is nearly free.
- **Remediation:** Compute the key once per field and reuse it for both
  `seen.insert` calls (the two are computed from the same `ty` a few lines
  apart, so this alone halves the work). If more is wanted,
  `syn::Type` implements `Hash`/`Eq` when `extra-traits` is on — but F17 argues
  for removing that feature, so the string key is the right *design*; just do
  it once. Note also that `HashSet<(String, &str)>` allocates the key `String`
  even on the (common) duplicate-hit path; `HashSet::contains` with a borrowed
  key first, inserting only on miss, avoids that — though it costs a second
  hash.
- **Evidence:** inferred from code reading. Confirm with
  `cargo build --timings` on a fixture crate deriving `DeserializeIn` for a
  struct with many distinct field types, or by instrumenting the derive
  directly with `-Z self-profile`.

#### F19. Generated code volume per derived type is substantial, and there is no `#[inline]` guidance on the generated visitor methods

- **Location:** generated by
  `crates/multitude_macros_impl/src/lib.rs:537-680` (`field_enum`),
  `:680-960` (`named_visitor`), `:1265-1400` (the enum equivalents)
- **Issue:** Each derived type emits: a `Field0..FieldN + Ignore` enum, a
  `Deserialize` impl for it containing a `__FieldVisitor` with `visit_str`,
  `visit_bytes` and `visit_u64`, a `const FIELDS: &[&str]`, the main visitor
  struct with both `visit_map` and `visit_seq`, a per-field custom seed struct
  where `#[serde(with)]`/`#[multitude(with)]` is used, and a helper impl with
  synthesised lifetimes. None of the generated methods carry `#[inline]`.

  For `visit_map`, `visit_seq` and the field visitor this is almost certainly
  right — they are large and called once per value, and `serde_derive` does not
  annotate them either. I am recording it rather than recommending it because
  the *field* visitor's `visit_str` is small for small structs, is called once
  per key (i.e. the hottest generated function by call count), and lives in the
  consumer's crate where it is a normal inlining candidate anyway. So the
  default is probably correct, but it is the one generated function where a
  measurement might say otherwise.
- **Impact:** Low — and genuinely uncertain in sign. Recorded for completeness.
- **Remediation:** No change recommended without measurement. If it is ever
  investigated, `multitude_serde_cg`'s `typed` group
  (`benches/multitude_serde_cg.rs:108`) is the right instrument, and it must be
  run with LTO disabled (see the note in F13) or the result is meaningless.
- **Evidence:** inferred from code reading.
- **Philosophy note:** Adding `#[inline]` here would violate
  `docs/performance.md` rule 2 (no measurement showing the default is wrong).
  Explicitly flagged as an observation, not a recommendation.

### Benchmark coverage

No `benches/` directory, and correctly so — the crate has no runtime code.
Coverage of the crate's *output* is what matters, and that is good: the derive
is exercised end-to-end by `multitude_serde` (typed path,
`benches/multitude_serde.rs:22-34`) and heavily by `multitude_record_batch`
(21 Callgrind library benchmarks across 7 groups), both with `serde_json`
comparison arms. So the generated code's runtime cost *is* measured, even
though the crate itself has no benchmarks.

Two gaps:

1. **Compile-time cost of the derive is not tracked anywhere.** No fixture, no
   `--timings` baseline, no regression gate. Since a derive macro's principal
   cost to users is compile time, this is the one measurement dimension for
   this crate and it is absent. Same observation as for `multitude_macros`;
   the fix belongs at workspace level.
2. **Only the "happy shape" is benchmarked.** The derived code paths for
   `#[serde(default)]` at container level (which constructs a full
   `Default::default()` of the target type before deserializing — `lib.rs:715-745`),
   for `#[serde(with)]` custom seeds, for `deny_unknown_fields`, and for enums
   (`lib.rs:1265+`) are exercised by the unit tests but not by any benchmark.
   The container-`default` path in particular has a real per-value cost that
   nobody is measuring.

The crate does have `prettyplease` as a dev-dependency and uses it to snapshot
generated output (`lib.rs:1851` and around), which is the right way to catch an
accidental explosion in generated code volume. That is a useful proxy for
compile-time regression and is already in place.

### Considered and ruled out

- **The generated field matcher is the right shape.** `visit_str` emits a flat
  `match __value { "field_a" => Ok(Field0), ... , _ => <unknown> }` over `&str`
  literals (`lib.rs:562-566`, emitted at `:648-656`). `rustc` compiles a `&str`
  match into a length switch followed by a small memcmp tree, which is what
  `serde_derive` relies on too. A perfect-hash or `phf` scheme would be a
  deviation from the ecosystem pattern with no evidence it wins at typical
  field counts. No change proposed.
- **Both `visit_bytes` and `visit_u64` are generated** alongside `visit_str`
  (`lib.rs:662-680`), so binary formats and ordinal-encoded field identifiers
  do not fall through to a slow path. Correct.
- **The per-field `Option<T>` accumulator plus duplicate-field check**
  (`lib.rs:700-708`) is exactly `serde_derive`'s pattern. The `is_some()` test
  per field is the standard cost of detecting duplicate keys, and removing it
  would remove a defensive check — which `docs/performance.md` explicitly says
  not to do.
- **Eager container-default construction** (`lib.rs:715-745`): with
  `#[serde(default)]` on the container, the generated code builds the whole
  default value up front and destructures it, even when every field is present
  in the input. This looked like a finding until I checked: `serde_derive` does
  the same thing (`let __default: Self::Value = Default::default();`), for the
  same reason — the alternative is per-field lazy defaults, which needs
  `Default` on each field type rather than on the container. Matching the
  ecosystem here is the right call.
- **`try_resize`-style per-element work in the expansion** — none. The
  expansion is a single pass over fields/variants building `Vec<TokenStream2>`
  fragments; `Vec::with_capacity` is not used but the vectors are field-count
  sized, so it does not matter.
- **`RenameRule` application** (`attrs.rs:36-70`): allocates a `String` per
  renamed field (`name.split('_').map(capitalize).collect()`), which is correct
  and unavoidable — the result is the wire name that must live in the output
  token stream.
- **No quadratic behaviour found.** The duplicate-name check
  (`lib.rs:559-566`, `:1280-1287`) and the reserved-generic check
  (`lib.rs:110-123`) both use `HashSet`, so they are linear rather than the
  nested-loop shape one often finds in derives.
- **Error paths** all return `syn::Error` rather than panicking, and the
  formatting (`format!` at `lib.rs:123`, `:562`, `:1283`) only runs on the
  error path. Correct.
- **`allowed_external_types` metadata** (`Cargo.toml:22-29`) confines the
  public surface to `proc_macro2` and a few `syn` modules, which keeps the
  crate's API from forcing `syn` version churn on consumers. Good hygiene, no
  performance angle.

---

## Appendix: measurement recipes for the container-blocked findings

For whoever picks these up with a working toolchain. Every entry names the
smallest experiment that decides the finding.

| finding | experiment |
|---|---|
| F1 `Value` layout | Add `const _: () = assert!(size_of::<Value>() == 24);` after the change; then diff `multitude_serde_cg`'s `dynamic` group and `multitude_record_batch_cg`'s decode groups on instruction count and `DLmr`. |
| F2 `reserve_bytes` CAS | `multitude_teardown_cg` groups at `:61-73`; also `criterion_alloc/arena_lifecycle`. Expect a per-chunk delta, so scale the element count until chunk count dominates. |
| F3 atomic orderings | Disassemble on `aarch64-unknown-linux-gnu` and diff `ldaxr`/`stlxr` against `ldxr`/`stxr`. Re-run the crate's `loom` tests to prove soundness. |
| F4 cold split | `nm --size-sort` on the benchmark binary before/after for function size; `criterion_arc_array_cg` / `criterion_rc_array_cg` for instruction counts. |
| F5 `retain_mut` | New `criterion_vec/retain` group + `_cg` pair, over `u64` and a 64-byte payload. |
| F6 `resize` | New `criterion_vec/resize` group + `_cg` pair; check whether the fill vectorises by inspecting the disassembly for `movdqu`/`stp`. |
| F7 UTF-16 two-pass | New `criterion_utf16/from_str` group + `_cg` pair over ASCII / Latin-1 / CJK / emoji inputs. |
| F8 provider false sharing | New multi-threaded benchmark (N producer threads, shared consumer dropping handles); `perf c2c` or Callgrind cache simulation. |
| F9 chunk header | Same benchmark as F8; expect a smaller effect. No fix proposed. |
| F10 `Weak::upgrade` | `multitude_teardown_cg`; delta should scale with chunk count. |
| F11 `ArenaBuf` padding | `criterion_alloc/vec_builder` plus a new benchmark holding many live `Vec`s. |
| F12 `Cow` | New `criterion_cow/to_mut` group + `_cg` pair. |
| F13 `Allocator` inline | `criterion_arena_vs_allocator` **with LTO disabled** — the default bench profile's fat LTO makes this unmeasurable. |
| F14 `stats` overhead | Run any `_cg` benchmark twice, `--features stats` and without, and diff. |
| F15 monomorphisation | `cargo llvm-lines -p multitude`; count symbols in the benchmark binary. |
| F16–F18 compile time | `cargo build --timings` on a clean target dir for a fixture crate depending only on `multitude` with `serde`; compare the `syn`, `multitude_macros_impl` and consumer bars. |
| F19 generated inline | `multitude_serde_cg` `typed` group, LTO disabled. |
