# g3-bytesbuf-cachet findings

Scope: `crates/bytesbuf`, `crates/bytesbuf_io`, `crates/cachet`,
`crates/cachet_memory`, `crates/cachet_tier`, `crates/cachet_service`.

Analysis round 2. Repository HEAD `d799037`.

**Environment caveat (applies to every finding).** This container has no egress to
`index.crates.io`/`static.crates.io`, no cargo registry cache and no prebuilt
`target/`. `cargo metadata --offline` fails with
`no matching package named 'tokio' found`, so `cargo build`, `cargo bench`,
`cargo clippy` and every `just` recipe are unavailable. Consequently almost every
finding below is labelled `inferred from code reading`, and each one names the
specific benchmark that would confirm or refute it. Two classes of claim were
verified empirically anyway:

* **Type layouts** — measured by compiling a dependency-free, layout-identical
  replica program with plain `rustc -O` in `/tmp` and printing `size_of`/
  `align_of`. The replica and its binary were deleted afterwards and never added
  to the repository.
* **Census-style claims** (`#[inline]` counts, benchmark file inventories,
  feature tables) — verified by direct `grep`/`glob` over the checkout.

**House-philosophy alignment.** `docs/performance.md`, `docs/benchmarks.md`,
`docs/callgrind-benchmarks.md`, `docs/naming.md`, the root `AGENTS.md` and
`crates/bytesbuf/AGENTS.md` were read before analysis. Findings that contradict
house philosophy (architectural rather than surgical, un-idiomatic, or
optimising a path the house rules say not to optimise) are still reported but
carry an explicit **Philosophy note**.

**Cross-group context that conditions everything below.** The workspace
`[profile.bench]` sets `lto = "fat"` and `codegen-units = 1` while
`[profile.release]` sets neither. Every benchmark number in this repository
therefore comes from a build configuration no consumer receives, and — critically
for this group — fat LTO plus a single codegen unit lets the compiler inline
across crate boundaries regardless of `#[inline]` annotations. **The repository's
own benchmarks are structurally incapable of detecting a missing `#[inline]`.**
Every `#[inline]` finding in this document is therefore unverifiable by the
existing harness as configured; confirming any of them requires either a
Callgrind run with `lto = false` or inspection of the generated `.rlib`. This is
noted once here rather than repeated on each finding.

---

## Crate: bytesbuf

### Summary

`bytesbuf` is the most performance-critical crate in this group and, on the
whole, the most carefully written. Its `AGENTS.md` documents a deliberate
thread-isolated architecture in which mutexes are expected never to be
contended, buffers are sized to one unit of work, and objects are made
deliberately large (inline span storage) to avoid heap traffic. Several
constructs that would look like defects in another crate — `nm` metric emission
on the allocation path, ~600-byte `BytesBuf` values, manual vtables behind
`unsafe` — are documented, justified design decisions and are recorded under
"Considered and ruled out" rather than as findings.

The real issues cluster in three places: (1) a systematic absence of `#[inline]`
on small non-generic public functions that cross the crate boundary, which
`docs/performance.md` rule 1 says should carry it — the crate clearly knows the
rule, because `view_get.rs` applies it thoroughly, so the gaps in `buf.rs`,
`view.rs` and `bytes_compat/view.rs` read as oversights rather than policy;
(2) one lock-scope defect in the global pool's multi-block path that contradicts
the comment sitting directly above it; and (3) `SmallVec` construction without a
capacity hint on paths whose size is known in advance.

### Findings

#### F1. Global pool holds the pool mutex across multi-block buffer construction

- **Location:** `crates/bytesbuf/src/mem/global.rs:204-234`
- **Issue:** `allocate_uniform` has two paths. The single-block path
  (lines 216-227) deliberately narrows the critical section, and the comment at
  lines 217-220 explains that the lock must be released before the `BytesBuf` is
  built. The multi-block path does the opposite: `pool_arc.lock()` is taken at
  line 229 and is still held when `BytesBuf::from_blocks(blocks)` runs at
  line 233. `from_blocks` constructs a `SmallVec<[SpanBuilder; MAX_INLINE_SPANS]>`
  with `MAX_INLINE_SPANS == 8` (`crates/bytesbuf/src/constants.rs:10-33`), so any
  request needing more than eight blocks performs a heap allocation *while
  holding the pool lock*. A 1 MiB request against 64 KiB blocks needs sixteen
  blocks and hits this exactly; `crates/bytesbuf/benches/global_pool.rs` already
  exercises that shape via its `alloc_1mb` case.
- **Impact:** Medium — `bytesbuf/AGENTS.md` states the architecture is
  thread-isolated so pool mutexes should not be contended, which caps the blast
  radius. But the fix is free, the code already demonstrates the correct pattern
  ten lines above, and the current form also lengthens the window during which
  an allocator call (a syscall in the worst case) runs under a lock.
- **Remediation:** Collect the blocks under the lock, `drop` the guard (or end
  the scope) and only then call `BytesBuf::from_blocks`, mirroring lines 216-227.
  Purely surgical; no behaviour change.
- **Evidence:** inferred from code reading. Confirmable with
  `cargo bench -p bytesbuf --bench global_pool` comparing `alloc_1mb` before and
  after, ideally with a second thread allocating concurrently — though note that
  no existing bench in this crate is multi-threaded, so the contention component
  would need a new bench case.

#### F2. `BlockRef` reference-count `Clone`/`Drop` are not `#[inline]`

- **Location:** `crates/bytesbuf/src/mem/block_ref.rs:118-127` (`impl Clone`),
  `crates/bytesbuf/src/mem/block_ref.rs:129-134` (`impl Drop`),
  `crates/bytesbuf/src/mem/block_ref.rs:104-116` (`meta`)
- **Issue:** `BlockRef` is the shared-ownership handle for a memory block. Its
  `Clone` is a single atomic increment and its `Drop` a single atomic decrement
  plus a conditional release; `meta()` is a pointer dereference. These are the
  hottest functions in the crate — every `BytesView` clone, every span split,
  every buffer teardown goes through them — and every one of them is a
  small non-generic function that consumers reach across the crate boundary.
  None carries `#[inline]`. `docs/performance.md` rule 1 is explicit that small
  public functions crossing a crate boundary should be annotated.
- **Impact:** Medium — a non-inlined call around a single atomic RMW roughly
  doubles the instruction cost of the operation, and these run per span.
- **Remediation:** Add `#[inline]` to `Clone::clone`, `Drop::drop` and `meta`.
- **Evidence:** empirically verified (grep census of the file); the performance
  consequence is inferred from code reading. Confirmable with
  `cargo bench -p bytesbuf --bench view_cg` (instruction counts on clone-heavy
  scenarios) built with `lto = false`, since fat LTO masks the difference.

#### F3. `bytes_compat::view`'s `Buf` impl has no `#[inline]`, unlike its `BufMut` mirror

- **Location:** `crates/bytesbuf/src/bytes_compat/view.rs:11-42` — `remaining`
  (line 13), `chunk` (line 18), `chunks_vectored` (line 24), `advance` (line 39)
- **Issue:** This is the `bytes::Buf` adapter for `BytesView` — the interop
  surface every `bytes`-ecosystem consumer goes through, and the exact place
  where the compiler most needs help, because the calls arrive through a generic
  `B: Buf` bound in a *different* crate. Not one of the four methods is
  annotated. The mirror-image file `crates/bytesbuf/src/bytes_compat/buf.rs`
  annotates its `BufMut` methods at lines 13, 19 and 28. The asymmetry between
  two files written to the same pattern strongly suggests an oversight rather
  than a decision.
- **Impact:** Medium — `Buf::advance` and `Buf::chunk` are called in a tight
  loop by essentially every `bytes` consumer that reads a buffer.
- **Remediation:** Add `#[inline]` to the four `Buf` methods, matching
  `bytes_compat/buf.rs`.
- **Evidence:** empirically verified (grep census across both files).
  Confirmable with `cargo bench -p bytesbuf --bench bytesbuf_vs_bytes` built with
  `lto = false`.

#### F4. `BytesBuf`'s small accessors are not `#[inline]`

- **Location:** `crates/bytesbuf/src/buf.rs` — `len` (line 487), `is_empty`
  (line 508), `capacity` (line 543), `remaining_capacity` (line 583), `consume`
  (line 631), `first_unfilled_slice` (line 967)
- **Issue:** `buf.rs` defines 23 `pub fn` and carries exactly one `#[inline]`.
  `BytesBuf` is a concrete, non-generic type, so `docs/performance.md` rule 1
  applies directly: these are small public functions crossing a crate boundary,
  and without annotation they cannot be inlined into a consumer crate under a
  normal `release` build (which, per the cross-group note, sets no LTO).
  `len`/`is_empty`/`capacity` are field reads or trivial arithmetic and are
  called inside consumer loops.
- **Impact:** Medium — individually tiny, collectively a call-per-iteration tax
  on every downstream loop over a buffer.
- **Remediation:** Annotate the trivial accessors. Be judicious as
  `docs/performance.md` instructs — this is a request to apply rule 1 to the
  handful of genuinely small functions listed, not to blanket-annotate all 23.
- **Evidence:** empirically verified (grep census: 23 `pub fn` / 1 `#[inline]`
  in `buf.rs`). Confirmable with `cargo bench -p bytesbuf --bench buf_cg` under
  `lto = false`.

#### F5. `BytesView`'s small accessors are not `#[inline]`

- **Location:** `crates/bytesbuf/src/view.rs` — `len` (line 240), `is_empty`
  (line 250), `first_slice` (line 587), `advance` (line 730), `append`
  (line 790)
- **Issue:** Same as F4 for the read side: `view.rs` has 16 `pub fn` and one
  `#[inline]`. `BytesView` is the type consumers actually pass around, and
  `advance`/`first_slice` sit inside parse loops. The crate demonstrates it
  understands the rule in `view_get.rs`, which carries 7 `#[inline]` over 6
  `pub fn`, so the gap here is inconsistency rather than policy.
- **Impact:** Medium — same reasoning as F4, on the more frequently used type.
- **Remediation:** Annotate `len`, `is_empty`, `first_slice` and `advance`.
  `append` is larger; leave it unless measurement says otherwise (rule 2).
- **Evidence:** empirically verified (grep census: 16 `pub fn` / 1 `#[inline]`).
  Confirmable with `cargo bench -p bytesbuf --bench view_cg` under `lto = false`.

#### F6. `BytesBuf::peek` builds a `SmallVec` without a capacity hint

- **Location:** `crates/bytesbuf/src/buf.rs:437`
- **Issue:** `peek()` starts from `SmallVec::new()` and pushes spans as it walks
  the buffer. The number of spans that will be produced is bounded by, and
  usually exactly derivable from, the buffer's existing span count, which is
  known before the loop starts. When the result exceeds `MAX_INLINE_SPANS`
  (8) the `SmallVec` spills and then grows geometrically, costing one or more
  reallocation-plus-memcpy cycles on a path whose whole purpose is to avoid
  copying.
- **Impact:** Low — only bites for views with more than eight spans, which
  `bytesbuf/AGENTS.md` describes as possible ("0 to hundreds of spans") but not
  the common case.
- **Remediation:** Use `SmallVec::with_capacity(n)` where `n` is the known upper
  bound on the span count. Precedent exists elsewhere in the crate.
- **Evidence:** inferred from code reading. Confirmable by adding a
  many-span `peek` case to `cargo bench -p bytesbuf --bench buf`; no existing
  case covers >8 spans.

#### F7. `BytesView::range_checked` performs a two-pass span scan

- **Location:** `crates/bytesbuf/src/view.rs:377-506`
- **Issue:** `range_checked` walks the span list once to locate and validate the
  range boundaries and then walks it again to build the result. For a view with
  `n` spans this is O(n) per call, which is fine in isolation; the concern is
  aggregate. A caller that repeatedly narrows a view — the natural way to write
  an incremental parser — performs O(n) work per narrowing, giving O(n²) over a
  full traversal of a hundreds-of-spans view.
- **Impact:** Low — the constant is small and the common case (a handful of
  spans) is unaffected. Recording it as a design note rather than a defect: no
  surgical fix exists, and the alternative (a cursor type that remembers its
  position) is an API addition, not a tweak.
- **Remediation:** No surgical change recommended. If profiling ever implicates
  it, the answer is a cursor/iterator API that amortises the scan, not a rewrite
  of `range_checked`.
- **Evidence:** inferred from code reading. Confirmable by adding a
  repeated-narrowing scenario over a 128-span view to
  `cargo bench -p bytesbuf --bench view`; no existing case covers it.

#### F8. `NeutralBlock` colocates the atomic refcount with payload bytes

- **Location:** `crates/bytesbuf/src/mem/global.rs:284-320`
- **Issue:** `NeutralBlock<SIZE>` places its 16-byte `BlockMeta` — which contains
  the `AtomicUsize` reference count — immediately before the payload, so the
  refcount shares its 64-byte cache line with roughly the first 48 bytes of
  data. A thread mutating or reading the head of the payload and a thread
  adjusting the refcount will ping-pong that line between cores.
- **Impact:** Low — `bytesbuf/AGENTS.md` states explicitly that the architecture
  is thread-isolated and that blocks are not expected to be shared across
  threads under contention, which is precisely the scenario required for this to
  matter. Recorded for completeness; not worth padding to a cache line, since
  doing so would waste 48 bytes per block in the design's actual usage pattern.
- **Remediation:** None recommended. If a future design does share blocks across
  threads, revisit with `#[repr(align(64))]` on the metadata.
- **Evidence:** inferred from code reading plus empirically verified metadata
  size (16 bytes, `rustc` layout replica). Confirmable only by a new
  multi-threaded benchmark; none exists in this crate.

#### F9. No `#[inline]` on `lib.rs`'s public surface

- **Location:** `crates/bytesbuf/src/lib.rs`
- **Issue:** The single `pub fn` defined in `lib.rs` carries no `#[inline]`.
  Included for census completeness rather than as a material concern.
- **Impact:** Low.
- **Remediation:** Evaluate against rule 1 alongside F4/F5.
- **Evidence:** empirically verified (grep census: 1 `pub fn` / 0 `#[inline]`).
  Confirmable under `lto = false` Callgrind only.

#### F10. `buf_put.rs` annotates less than half its public surface

- **Location:** `crates/bytesbuf/src/buf_put.rs` (7 `pub fn`, 3 `#[inline]`)
- **Issue:** The `put_*` family is the write-side mirror of `view_get.rs`, which
  is thoroughly annotated (7 `#[inline]` over 6 `pub fn`). The write side is
  annotated for fewer than half its methods. Small fixed-width `put_u16`-style
  writes are exactly the case rule 1 targets: a few instructions wrapped in a
  cross-crate call.
- **Impact:** Low to Medium — depends on how write-heavy the consumer is; for a
  serialiser it is Medium.
- **Remediation:** Bring the small fixed-width `put_*` methods up to parity with
  `view_get.rs`.
- **Evidence:** empirically verified (grep census). Confirmable with
  `cargo bench -p bytesbuf --bench buf_cg` under `lto = false`.

#### F11. Buffer `unsafe advance` relies on caller-upheld invariants

- **Location:** `crates/bytesbuf/src/buf.rs:1054`
- **Issue:** Assessed per the task's instruction to justify every `unsafe`
  block. This one skips bounds validation that the safe path performs. It
  carries a `SAFETY:` comment naming the invariant, and it does buy real
  performance: the safe equivalent would re-validate an offset the caller has
  already computed, on a per-span path. Reported as *justified*, not as a
  defect.
- **Impact:** Low — no action needed.
- **Remediation:** None. Retain.
- **Evidence:** inferred from code reading.

#### F12. Benchmarks cover neither `bytes_compat` conversion nor >8-span shapes

- **Location:** `crates/bytesbuf/benches/` (7 files, enumerated below)
- **Issue:** See "Benchmark coverage". Summarised here because it interacts with
  F3, F6 and F7: the specific constructs those findings implicate are precisely
  the ones with no benchmark case, so none of them could be caught by the
  existing harness even if the environment permitted running it.
- **Impact:** Medium — coverage gap on the crate's interop surface.
- **Remediation:** See "Benchmark coverage" below.
- **Evidence:** empirically verified (bench file census).

### Benchmark coverage

Files present in `crates/bytesbuf/benches/`:

| File | Kind | Pairing |
|------|------|---------|
| `buf.rs` | Criterion | paired with `buf_cg.rs` |
| `buf_cg.rs` | Callgrind/Gungraun | paired ✅ |
| `view.rs` | Criterion | paired with `view_cg.rs` |
| `view_cg.rs` | Callgrind/Gungraun | paired ✅ |
| `global_pool.rs` | Criterion | paired with `global_pool_cg.rs` |
| `global_pool_cg.rs` | Callgrind/Gungraun | paired ✅ |
| `bytesbuf_vs_bytes.rs` | Criterion | comparison bench, no `_cg` pair |

All three `_cg.rs` files have a same-named Criterion counterpart, satisfying
`docs/naming.md`'s pairing rule. The unprefixed Criterion group names are
grandfathered. **This is the best-covered crate in the group by a wide margin.**

Gaps, in rough priority order:

1. **`bytes_compat` conversions are unbenchmarked.** Neither the `Buf` impl for
   `BytesView` (`bytes_compat/view.rs`) nor the `BufMut` impl for `BytesBuf`
   (`bytes_compat/buf.rs`) has a benchmark. This is the crate's interop
   surface and the subject of F3. `bytesbuf_vs_bytes.rs` compares the two
   libraries but does not exercise the adapter path.
2. **No multi-span (>8) scenarios.** Every scenario appears to stay within
   `MAX_INLINE_SPANS`, so the `SmallVec` spill path — the one that actually
   allocates, and the subject of F1, F6 and F7 — is never measured.
3. **`OpaqueMemory`/`CallbackMemory` indirection is unbenchmarked.** The manual
   vtable exists for performance; nothing measures whether it pays.
4. **Block release and free-list return are unbenchmarked.** `global_pool.rs`
   measures acquisition. Release is the other half and involves the atomic
   decrement of F2.
5. **No cross-thread release scenario.** Blocks allocated on one thread and
   released on another exercise the atomics of F2 and the layout of F8. Nothing
   covers it. (Note this is partly by design — `AGENTS.md` says the architecture
   is thread-isolated — but the API permits it.)
6. **No benchmark is multi-threaded**, so F1's lock scope and F8's false sharing
   are unmeasurable with the current harness.

**Harness-validity caveat (cross-group).** `[profile.bench]` sets `lto = "fat"`
and `codegen-units = 1`; `[profile.release]` sets neither. Benchmarks therefore
measure a build no consumer receives, and fat LTO inlines across crate
boundaries regardless of annotations, making findings F2, F3, F4, F5, F9 and
F10 **structurally invisible to this crate's own benchmarks**. Confirming any
`#[inline]` finding requires a Callgrind run with LTO disabled. Benchmarks also
build with `--all-features`, so any measurement includes optional-feature code
that a default consumer does not compile.

### Considered and ruled out

* **`nm` metric emission on the allocation path** (`mem/global.rs:171`, `:182`,
  `:212`). `crates/bytesbuf/AGENTS.md` states these metrics are low-overhead and
  explicitly must not be flagged. Ruled out on house authority.
* **Atomic orderings in `mem/vec.rs:90-125` and `mem/global.rs:412`, `:425`,
  `:433`.** Textbook `Arc`-style protocol: `Relaxed` increment, `Release`
  decrement, `Acquire` fence before the drop. Correct and already minimal; no
  ordering can be weakened.
* **`BytesView::hash` (`view.rs:1017-1042`).** Deliberately allocation-free with
  a documented single-span fast path. Nothing to improve.
* **`BytesView::from_views` (`view.rs:150-154`).** Skips a size-hint pre-pass
  with an in-code justification citing measurement. This is exemplary — the
  house rule asks for exactly this kind of documented deviation — and it is the
  reason F6's `with_capacity` suggestion is scoped narrowly to `peek` rather
  than applied crate-wide.
* **`view_get.rs:280` `get_array_buffered`.** Marked `#[cold]` and
  `#[inline(never)]` with a written rationale. Correct use of both attributes.
* **Debug-only O(n) consistency checks** (`buf.rs:496`, `buf.rs:588`,
  `view.rs:242` — the `calculate_len` family). These are `debug_assert`-gated,
  cost nothing in release, and are exactly the defensive runtime checks
  `docs/performance.md` says to preserve. Not a finding.
* **`BytesBuf` ≈ 600 bytes and `BytesView` ≈ 288 bytes.** Large by any normal
  standard, but this is the documented `MAX_INLINE_SPANS = 8` trade-off
  (`constants.rs:10-33`) that `AGENTS.md` calls out as intentional: objects are
  made large precisely so that spans need no heap allocation. Flagging it would
  be arguing with the crate's stated design.
* **Every `unsafe` block in the crate.** Each carries a `SAFETY:` comment and
  each exists to implement the manual-vtable / zero-allocation design. In each
  case a safe equivalent would either re-validate an already-proven invariant on
  a per-span path or force a heap allocation. They buy their keep.
* **Double-checked slice comparisons in `PartialEq` (`view.rs:860-936`).**
  Defensive checks; `docs/performance.md` says preserve them.
* **`SmallVec` as a dependency choice.** Correct ecosystem pick for this design;
  no justification-of-deviation issue.

---

## Crate: bytesbuf_io
### Summary

`bytesbuf_io` is a thin async I/O abstraction (1,424 source lines across seven
files, much of it doc comments and tests). Its core trait design is good:
`Read` and `ReadExt` both use `#[trait_variant::make(Send)]`
(`crates/bytesbuf_io/src/read.rs:49`, `crates/bytesbuf_io/src/read_ext.rs:16`),
which produces RPITIT-based futures with **no boxing at all** on the primary
path. That is the single most important performance decision in the crate and it
was made correctly.

The concerns are concentrated in the optional `futures-stream` adapter, which
pays for the `futures_core::Stream` object-safe signature with a heap allocation
per stream item, a whole-struct `Box::pin`, and a lifetime `transmute`. There is
also one allocation-shaped issue in the conditional-read loop, and the crate has
**no benchmarks whatsoever**.

### Findings

#### F13. `ReadAsFuturesStream::poll_next` heap-allocates a boxed future per stream item

- **Location:** `crates/bytesbuf_io/src/read_futures.rs:75-117`, specifically
  the `Box::pin(future)` at line 85 and the `mem::transmute` at lines 87-97;
  field declaration at line 35
- **Issue:** Each time the stream needs a new item and `active_read` is `None`,
  `poll_next` constructs an `async move` block over `&mut this.inner` and
  `Box::pin`s it into
  `Option<Pin<Box<dyn Future<Output = Result<BytesBuf, S::Error>> + Send>>>`.
  That is one heap allocation per stream item, on the crate's hottest loop, in
  direct tension with `docs/performance.md`'s "memory allocation is the root of
  all evil". The box is then `mem::transmute`d to extend a struct-bounded
  lifetime to `'static` — a genuinely unsound-looking construct held together
  only by the hand-maintained invariant documented at line 37 ("we can only touch
  this field if `active_read` is `None`") and the field-ordering comment at
  line 34.
  **Correction to a previous round's claim:** the allocation happens once per
  *item*, not once per *poll*. A `Pending` poll stashes the future back into
  `this.active_read` at line 114, so a slow source that polls ten times for one
  item allocates once, not ten times. The finding stands — it is still one
  allocation per item on a streaming path — but its magnitude is smaller than
  previously stated.
- **Impact:** High — for a byte-stream adapter, "one malloc per chunk" is the
  dominant cost at small chunk sizes, and small chunk sizes are exactly what the
  type's own security note (lines 20-29) says an adversarial source will produce.
- **Remediation:** Not surgical, and the obvious fixes each have a cost:
  (a) store the future inline in the struct as an
  `Option<ManuallyDrop<S::ReadAnyFuture>>` — requires the future type to be
  nameable, which `#[trait_variant::make]` does not currently expose;
  (b) reuse the box across items by writing the new future into the existing
  allocation — possible only if all iterations produce the same concrete future
  type, which they do, so this is the most promising route and would reduce the
  cost to one allocation per *stream* rather than per item;
  (c) leave as is and document the cost. Option (b) is the recommendation.
- **Evidence:** inferred from code reading (line-by-line trace of `poll_next`).
- **Philosophy note:** The house rule is "no allocation on the hot path" and
  this violates it, but every remediation is architectural rather than surgical —
  (a) changes the trait's public shape, (b) requires reworking the self-reference
  handling. Flagged as **conflicting**: the fix is larger than house guidance
  normally sanctions, so it needs an explicit decision rather than a quiet patch.
- **Confirming benchmark:** none exists. Would need a new
  `crates/bytesbuf_io/benches/read_futures.rs` streaming a fixed byte count from
  `testing::FakeRead` at several `max_read_size` values, paired with a
  `read_futures_cg.rs` per `docs/naming.md`; the instruction-count delta between
  a 2-byte and a 64 KiB `max_read_size` isolates the per-item allocation.

#### F14. `ReadAsFuturesStream::new` boxes the entire stream struct

- **Location:** `crates/bytesbuf_io/src/read_futures.rs:48-54`, surfaced through
  the public API at `crates/bytesbuf_io/src/read_ext.rs:103` and `:186`
- **Issue:** `new` returns `Pin<Box<Self>>` rather than `Self`, so constructing a
  stream always heap-allocates the adapter in addition to the per-item box of
  F13. The public `ReadExt::into_futures_stream` propagates this into the API
  signature, meaning callers cannot choose to stack-pin the stream with
  `std::pin::pin!` even when they could.
- **Impact:** Low — one allocation per stream, not per item, and
  `docs/performance.md` explicitly deprioritises first-insert/construction costs.
  Recorded mainly because it is a *public API shape* that forecloses the cheaper
  option for callers, which is the category the task asks about.
- **Remediation:** Return `Self` and let the caller pin (`pin!` on the stack, or
  `Box::pin` if they need `'static`). This is a breaking API change, so it should
  ride along with any F13 rework rather than be done alone.
- **Evidence:** inferred from code reading.
- **Confirming benchmark:** none exists; a construction-only case in the proposed
  `benches/read_futures.rs` would show it, though per house guidance this is
  precisely the kind of construction cost not worth chasing on its own.

#### F15. Conditional-read loop calls `BytesBuf::peek()` on every iteration

- **Location:** `crates/bytesbuf_io/src/read_ext.rs:169` (inside the
  `while into.len() < len` loop at lines 158-180)
- **Issue:** `read_at_most_into_while` calls `inspect_fn(into.peek())` once per
  read chunk. `BytesBuf::peek` builds a fresh `SmallVec` of spans from
  `SmallVec::new()` (`crates/bytesbuf/src/buf.rs:437`, see bytesbuf F6). The
  buffer grows by one or more spans per iteration, so a loop that reads a large
  `len` in small chunks will cross `MAX_INLINE_SPANS = 8` and thereafter
  heap-allocate a fresh, geometrically-growing span vector **on every
  iteration** — an O(n²) allocation pattern in the number of chunks. The crate's
  own tests deliberately drive this shape with `max_read_size(nz!(2))`
  (e.g. `read_ext.rs:520`, `:541`).
- **Impact:** Medium — quadratic in chunk count, and the trickle-feed scenario
  that triggers it is the one the crate's own security notes say to expect from
  untrusted sources.
- **Remediation:** Two options, both modest: give `peek()` a capacity hint
  (bytesbuf F6), which caps it at one allocation per call rather than several; or
  hoist the view construction so the inspect callback sees only the newly-read
  tail, which is usually all it needs. The former is the surgical one.
- **Evidence:** inferred from code reading, cross-referenced against
  `crates/bytesbuf/src/buf.rs:437`.
- **Confirming benchmark:** none exists. A new
  `crates/bytesbuf_io/benches/read_ext.rs` case reading 64 KiB through
  `FakeRead` with `max_read_size` of 2 versus 4096 would expose the quadratic
  term directly.

#### F16. `ReadInspectDecision::Failed` boxes on a path shared with the hot decision

- **Location:** `crates/bytesbuf_io/src/read_ext.rs:191-205`
- **Issue:** The enum's `Failed` variant carries a `Box<dyn Error>`, which is
  the right call for an error payload. Measured size of the whole enum is
  **24 bytes** (empirically verified). It is returned by value from the
  user-supplied `inspect_fn` on every loop iteration, including the overwhelmingly
  common `ContinueRead` case. 24 bytes is small enough that this is a non-issue —
  recorded to close it out explicitly, since "large enum variant" is on the
  task's checklist and this is the crate's only public enum.
- **Impact:** Low — no action needed. The box keeps the enum small; removing it
  would make things worse.
- **Remediation:** None. Retain.
- **Evidence:** empirically verified (`rustc` layout replica in `/tmp`:
  `ReadInspectDecision` = 24 bytes).

#### F17. `Error::caused_by` is invoked on every fallible step, including the success path's `map_err`

- **Location:** `crates/bytesbuf_io/src/read_ext.rs:116`, `:124`, `:162`;
  `crates/bytesbuf_io/src/error.rs`
- **Issue:** `.map_err(crate::Error::caused_by)` appears on every await in the
  extension methods. `map_err` on an `Ok` value is a no-op the optimiser removes
  entirely, so there is no cost on the success path. The construction cost is
  paid only when an error actually occurs, which is by definition cold. Recorded
  to close it out: this is *not* a finding, but it is the sort of pattern that
  looks like one at a glance.
- **Impact:** Low — no action needed.
- **Remediation:** None. Consider `#[cold]` on `Error::caused_by` if a future
  profile shows the error branch being laid out inline; not warranted today.
- **Evidence:** inferred from code reading.

#### F18. No `#[inline]` anywhere in the crate

- **Location:** `crates/bytesbuf_io/src/` (all seven files)
- **Issue:** The crate contains no `#[inline]` annotations. Unlike `bytesbuf`,
  this is largely *correct*: `ReadExt`'s methods are generic
  (`impl<T> ReadExt for T where T: Read`), so they are monomorphised into the
  consumer crate and are already inlinable — `docs/performance.md` rule 1 does
  not apply, rule 2 (annotate only with measurement) does. Recorded so that a
  reader does not mistake the zero count for the same defect flagged in
  `bytesbuf`. The one genuinely non-generic small function is
  `ReadAsFuturesStream::into_inner` (`read_futures.rs:57-66`), which is neither
  hot nor small enough to matter.
- **Impact:** Low — no action needed.
- **Remediation:** None.
- **Evidence:** empirically verified (grep census over all seven source files).

### Benchmark coverage

**`crates/bytesbuf_io/benches/` does not exist. The crate has zero benchmarks —
neither Criterion nor Callgrind.**

This is the most significant coverage gap in the group after `cachet_tier`,
because the crate sits directly on the I/O path and because its one genuinely
expensive construct (F13, the per-item boxed future) is invisible without one.

Recommended additions, in priority order, following `docs/naming.md` (Criterion
group names `<file-basename>/<subgroup>`, every `_cg.rs` paired with a
same-named Criterion file):

1. **`benches/read_futures.rs` + `benches/read_futures_cg.rs`** — stream a fixed
   payload through `ReadAsFuturesStream` over `testing::FakeRead` at
   `max_read_size` values spanning 2 B to 64 KiB. Groups:
   `read_futures/small_chunks`, `read_futures/large_chunks`,
   `read_futures/construction`. Directly measures F13 and F14. Note this bench
   requires the non-default `futures-stream` feature; per `docs/benchmarks.md`
   it should declare `required-features = ["futures-stream", "test-util"]`, and
   readers must be told the numbers do not represent a default build.
2. **`benches/read_ext.rs` + `benches/read_ext_cg.rs`** — `read_exactly`,
   `read_at_most` and `read_at_most_into_while` at varying chunk sizes. Measures
   F15's quadratic term.
3. **`benches/read.rs`** — the bare `Read` trait through `testing::Null` and
   `testing::FakeRead`, establishing the RPITIT no-boxing baseline that F13's
   adapter should be compared against. Without this baseline there is nothing to
   attribute the adapter's overhead to.

The `[profile.bench]` fat-LTO caveat noted in the preamble applies here too, and
with extra force: fat LTO can in principle devirtualise and elide the boxed
future of F13 in a benchmark where the concrete `Read` type is statically known,
making the adapter look free in a benchmark while costing a malloc per item in a
real consumer's `release` build. Any `read_futures` bench should be run with
`lto = false` as well as with the repo default, and the two numbers reported
side by side.

### Considered and ruled out

* **`#[trait_variant::make(Send)]` on `Read` (`read.rs:49`) and `ReadExt`
  (`read_ext.rs:16`).** This is the correct modern choice — RPITIT futures with
  no `Box` and no `dyn` — and stands in deliberate contrast to `cachet_tier`'s
  `dyn(box)` dynosaur erasure (see that crate's F40). Not a finding; a positive.
* **`unsafe` in `into_inner` (`read_futures.rs:57-66`).** `Pin::into_inner_unchecked`
  guarded by first clearing `active_read`, with a `SAFETY:` comment that
  correctly states the reasoning. Justified; a safe equivalent does not exist.
* **`unsafe { self.get_unchecked_mut() }` (`read_futures.rs:77`).** Standard,
  necessary for a manual `poll_next` on a `!Unpin` type. Justified.
* **`assert_eq!(buffer.len(), len)` in `read_exactly` (`read_ext.rs:135`) and
  `assert!(into.len() < len, ...)` (`read_ext.rs:158`).** Defensive runtime
  checks on a per-*call* (not per-iteration) basis. `docs/performance.md` says
  preserve them. Not a finding.
* **The `while` loop in `read_exactly` (`read_ext.rs:122-133`) recomputing
  `remaining` via `checked_sub`.** One subtraction per I/O operation; the I/O
  dominates by orders of magnitude. Not worth touching, and the `checked_sub`
  plus its `expect` message is exactly the documented-precondition style the root
  `AGENTS.md` asks for.
* **`futures-stream` and `test-util` are both non-default features
  (`crates/bytesbuf_io/Cargo.toml:26-29`).** Default feature set is empty, and
  `bytesbuf` is pulled with only `features = ["std"]`. Lean and correct; no
  heavy-default-feature concern in this crate.
* **Dependency choices (`Cargo.toml:31-35`): `bytesbuf`, `futures-core`
  (optional), `ohno`, `trait-variant`.** `futures-core` rather than the full
  `futures` crate for the non-dev dependency is the right call. No bloat.

---

## Crate: cachet

### Summary

`cachet` is the front-end of the caching stack: `Cache<K, V>` wraps a
`DynCacheTier`, adds telemetry, request-coalescing (`mergers`), an insert policy,
optional fallback tiering, and background refresh. It is 6,199 source lines
across 24 files and is the largest crate in this group.

The headline problem is that **the cache hit path is not allocation-free**, and
in the worst configuration performs several heap allocations per `get()`. Two
independent causes compound:

1. `Cache::get` accepts `&Q` with a `Q: Borrow`-style bound
   (`crates/cachet/src/cache.rs:228-231`) — the correct, idiomatic,
   allocation-free signature — and then immediately throws that away by calling
   `key.to_owned()` (lines 237 and 246) because the underlying
   `CacheTier::get` takes `&K` (`crates/cachet_tier/src/tier.rs:46`). For the
   overwhelmingly common `Cache<String, V>` looked up by `&str`, that is a
   `String` allocation on every single hit. The same pattern appears at eight
   further sites in the same file.
2. `DynCacheTier` erases the tier with `dyn(box)`
   (`crates/cachet_tier/src/tier.rs:38`), so the returned future is boxed once
   per tier per operation.

Beyond that: an unconditional global atomic increment per operation even when
telemetry is disabled; a thread-local write plus drop guard on *every poll* of
every cache future; a thread-local read performed before the check that would
make it unnecessary; two clock reads per tier per operation; and a coarse global
`Mutex<HashSet<K>>` guarding refresh de-duplication that clones the key before
discovering the key is already in flight.

### Findings

#### F19. `Cache::get` allocates an owned key on every lookup

- **Location:** `crates/cachet/src/cache.rs:228-255`, specifically
  `let owned = key.to_owned();` at line 237 (coalesced path) and line 246
  (direct path)
- **Issue:** The signature is
  `pub async fn get<Q>(&self, key: &Q) -> ... where K: Borrow<Q>, Q: Hash + Eq + ToOwned<Owned = K> + ?Sized + Send + Sync`.
  This is the textbook `HashMap::get` shape whose entire purpose is to let a
  caller look up a `String`-keyed map with a `&str` and pay nothing. The body
  then calls `key.to_owned()` unconditionally — producing a heap-allocated `K` —
  purely so it has something to hand to `CacheTier::get(&K)`. The API therefore
  advertises a zero-allocation lookup and delivers an allocating one.
  This is the group's top finding and it is a direct violation of
  `docs/performance.md`'s "memory allocation is the root of all evil".
- **Impact:** High — one malloc + memcpy + free per cache hit, on the hottest
  path of a crate whose reason to exist is to be faster than the thing it
  caches. For small values it can plausibly dominate the lookup itself.
- **Remediation:** The root cause is `CacheTier::get`'s `&K` parameter
  (see cachet_tier F39). Options: (a) generalise `CacheTier::get` to
  `get<Q>(&self, key: &Q) where K: Borrow<Q>` — but `dyn(box)` erasure requires
  object safety, and a generic method is not object-safe, so this needs the
  erasure strategy to change too; (b) add a separate non-erased fast path for
  the single-tier case; (c) accept the allocation and remove the misleading
  `Borrow` bound from `Cache::get`'s public signature so callers at least know
  what they are paying. Option (c) is the only surgical one and it is a
  documentation-honesty fix, not a performance fix.
- **Evidence:** inferred from code reading; line numbers verified directly.
- **Philosophy note:** **Conflicting.** The house rule is emphatic about
  allocation on hot paths, but every real remediation is architectural — it
  changes a public trait shape across three crates. `docs/performance.md`
  prefers surgical interventions and warns against architectural rewrites. This
  finding needs an explicit maintainer decision, not a patch.
- **Confirming benchmark:** `cargo bench -p cachet --bench operations` measures
  `get` today, but its storage is `cachet_tier::testing::MockCache`, which
  itself clones the key and takes three mutexes (see F31) — the signal is
  swamped. The clean measurement is a Callgrind bench (none exists for `cachet`)
  with a no-op tier, comparing `Cache<String, u64>::get(&str)` against a direct
  `HashMap` lookup; the malloc/free pair shows up unambiguously in instruction
  counts.

#### F20. The same `to_owned()` pattern repeats across eight more `Cache` methods

- **Location:** `crates/cachet/src/cache.rs:318` and `:327` (`invalidate`),
  `:466` (`get_or_insert`), `:557` (`get_or_insert_with`), `:637`
  (`try_get_or_insert_with`), `:714`, `:802`
- **Issue:** Identical to F19 — every method taking a borrowed `&Q` converts to
  an owned `K` before calling into the tier. `get_or_insert` and friends need an
  owned key for the insert half, so their allocation is partly justified; the
  read half still pays it even on a hit, which is the common case for a cache.
- **Impact:** High — `get_or_insert_with` is the idiomatic entry point for most
  cache users, so in practice this path is at least as hot as `get`.
- **Remediation:** Same as F19; a fix there fixes these. If F19 is resolved with
  a borrowed read path, `get_or_insert*` should call the borrowed path first and
  only `to_owned()` in the miss branch — that alone would remove the allocation
  from the hit path of the most-used API without any trait change, and **is**
  surgical.
- **Evidence:** inferred from code reading; all eight sites verified by grep.
- **Confirming benchmark:** as F19; add a `get_or_insert_with` hit-rate sweep
  (100% hit vs 0% hit) to `crates/cachet/benches/operations.rs`.

#### F21. An unconditional global atomic increment on every cache operation

- **Location:** `crates/cachet/src/telemetry/cache.rs:20` (the static),
  `:27-29` (`next_request_id`); called from `crates/cachet/src/cache.rs:233`,
  `:283`, `:314`, `:357`, `:463`, `:711`, `:799`
- **Issue:** Every public `Cache` method begins with
  `let request_id = next_request_id();`, which is
  `NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)` on a single process-wide
  `AtomicU64`. This happens **whether or not any telemetry handler is
  installed** and whether or not the `logs` feature is enabled. On a machine
  with many cores hammering one cache, that static is a single contended cache
  line: every operation on every thread performs a read-modify-write on the same
  64-byte line, serialising all of them. This is textbook false sharing — except
  it is not even false, it is true sharing of a counter nobody is reading.
- **Impact:** High — a `lock xadd` on one globally shared line is one of the few
  constructs that turns a linear-scaling cache into a flat one. At low core
  counts it is ~20 cycles; at 64 cores of contention it is orders of magnitude
  worse.
- **Remediation:** Surgical: make ID generation lazy. Either (a) skip it when
  `self.telemetry` has no handler and telemetry features are off — the ID is only
  ever consumed by `emit_tier_event`, which itself checks
  `if let Some(handler)` at `telemetry/cache.rs:130`; or (b) keep a
  thread-local counter and compose the ID from a per-thread prefix plus a
  thread-local sequence, removing cross-core traffic entirely. Option (a) is
  smaller and fully preserves behaviour when telemetry is on.
- **Evidence:** inferred from code reading; the unconditional call sites were
  verified by grep across `cache.rs`.
- **Philosophy note:** none — this is a surgical fix that preserves all
  observable behaviour.
- **Confirming benchmark:** none of the three existing `cachet` benches is
  multi-threaded, so the contention component is currently unmeasurable. A new
  `crates/cachet/benches/contention.rs` running N threads against one `Cache`
  would show it; a `cachet` Callgrind bench (also absent) would show the
  single-threaded ~20-cycle component.

#### F22. `WithRequestId::poll` writes a thread-local and constructs a drop guard on every poll

- **Location:** `crates/cachet/src/telemetry/cache.rs:55-63`, with the guard at
  `:45-53`; applied at `crates/cachet/src/cache.rs:253` (`.with_request_id(...)`)
  and the equivalent line in each other public method
- **Issue:** Every cache operation's future is wrapped in `WithRequestId`. Its
  `poll` does a `Cell::replace` on the `CURRENT_REQUEST_ID` thread-local,
  constructs a `RestoreRequestId` drop guard, polls the inner future, and then
  runs the guard's `Drop` which does a second thread-local write. That is two
  TLS accesses plus a drop-flag-carrying guard **per poll**, not per operation —
  a future that yields five times pays it five times. The doc comment at
  lines 32-38 justifies the per-poll behaviour (task migration between threads),
  which is correct reasoning, but the cost is paid unconditionally including when
  no handler exists and nothing will ever read the value.
- **Impact:** Medium — TLS access on most platforms is a few instructions, but
  this is multiplied by poll count and stacked on top of F21 for every operation,
  and again it is pure overhead in the no-telemetry configuration.
- **Remediation:** Gate the wrapper: when `self.telemetry` has no handler and
  the telemetry features are off, return the inner future unwrapped (e.g. an
  `Either`, or make `with_request_id` a no-op constructor in that case). The
  wrapper is `pub(crate)` so this is contained within the crate.
- **Evidence:** inferred from code reading.
- **Confirming benchmark:** `cargo bench -p cachet --bench operations` would show
  it, but only if run *without* the `logs` feature — and that bench currently
  declares `required-features = ["logs", "test-util"]`, so it cannot be. See F30.

#### F23. `record_*` reads the request-ID thread-local before checking whether anyone wants it

- **Location:** `crates/cachet/src/telemetry/cache.rs:205-380` — every
  `record_hit` (`:207`), `record_miss` (`:212`), `record_expired` (`:217`),
  `record_get_error` (`:223`), `record_inserted` (`:234`),
  `record_insert_error` (`:245`), `record_invalidated` (`:256`),
  `record_invalidate_error` (`:267`), `record_cleared` (`:277`),
  `record_clear_error` (`:283`), `record_refresh_hit` (`:295`),
  `record_refresh_miss` (`:307`), `record_insert_rejected` (`:321`),
  `record_eviction` (`:344`), `record_background_expired` (`:367`)
- **Issue:** Each of these fifteen call sites evaluates
  `Self::current_request_id()` — a thread-local read — as an *argument* to
  `emit_tier_event`. `emit_tier_event` (`:129-140`) then immediately does
  `if let Some(handler) = &self.handler` and, in the no-handler case, discards
  everything. Rust's evaluation order means the TLS read happens before the
  check, on every operation, in every configuration, for nothing.
- **Impact:** Medium — one wasted TLS read per tier per operation, and there are
  as many tiers as the stack is deep. Trivially avoidable.
- **Remediation:** Fully surgical and behaviour-preserving: move the
  `current_request_id()` call *inside* `emit_tier_event`, past the
  `if let Some(handler)` guard, and drop the `request_id` parameter from its
  signature. Fifteen call sites get shorter. This is the single best
  effort-to-benefit fix in the crate.
- **Evidence:** inferred from code reading; all fifteen sites verified by grep.
- **Confirming benchmark:** a `cachet` Callgrind bench (none exists) with no
  handler installed; the instruction-count delta is exact and small, which is
  precisely what Callgrind is good at and Criterion is not.

#### F24. Two clock reads per tier per operation

- **Location:** `crates/cachet/src/cache.rs:234` (`self.clock.stopwatch()`) and
  `watch.elapsed()` at `:251`; `crates/cachet/src/wrapper.rs:131-134`
  (`stopwatch()` then `elapsed()`), and again in `insert` (`:149`), `invalidate`
  (`:157`), `clear` (`:167`)
- **Issue:** `Cache::get` starts a stopwatch and reads it. The `CacheWrapper`
  wrapping the tier underneath *also* starts a stopwatch and reads it. So a
  single-tier stack performs at least four clock reads per `get`, and a
  fallback stack with a wrapper per tier performs more. Every one of them feeds
  a `duration` argument that `emit_tier_event` discards when no handler is
  installed (F23). A monotonic clock read is a `rdtsc`-class operation or, worse,
  a vDSO call — tens of nanoseconds, comparable to the entire cache lookup it is
  timing.
- **Impact:** Medium to High — for an in-memory tier this can be a significant
  fraction of total operation cost, and it scales with stack depth.
- **Remediation:** Same shape as F23: only start the stopwatch when something
  will consume the duration. `CacheWrapper` already holds `self.telemetry`, so
  the check is local. Keep the timing when a handler is present — the point is
  not to remove observability, only to stop paying for it when it is off.
- **Evidence:** inferred from code reading.
- **Confirming benchmark:** `cargo bench -p cachet --bench operations` with and
  without a handler installed — but the bench currently installs one implicitly
  via `required-features = ["logs"]` (F30), so this comparison cannot be made
  with the harness as it stands.

#### F25. `TimeToRefresh::try_start_refresh` clones the key before discovering it is already in flight

- **Location:** `crates/cachet/src/refresh.rs:79-81`
- **Issue:** `self.in_flight.lock().insert(key.clone())` clones `K`
  unconditionally. `HashSet::insert` returns `false` when the key was already
  present, meaning the clone is immediately dropped. The already-present case is
  precisely the case this function exists to detect, and it is the *common* case
  under a refresh storm — many concurrent readers all seeing the same stale entry
  and all calling `do_refresh` for the same key.
- **Impact:** Medium — one wasted `K` allocation per suppressed refresh, and
  suppressed refreshes are by design the majority.
- **Remediation:** Surgical: `let mut guard = self.in_flight.lock(); if guard.contains(key) { return false; } guard.insert(key.clone())`.
  One extra hash lookup in exchange for eliminating a clone on the dominant path;
  for `String` keys that is a clear win. (`HashSet::get_or_insert_owned` would be
  cleaner still but is unstable.)
- **Evidence:** inferred from code reading.
- **Confirming benchmark:** `cargo bench -p cachet --bench refresh` covers
  refresh, but is single-threaded, so it never exercises the contended
  already-in-flight path. A multi-threaded storm case would be needed.

#### F26. Refresh de-duplication uses one global `Mutex<HashSet<K>>` for all keys

- **Location:** `crates/cachet/src/refresh.rs:42` (the field), `:67`
  (construction), `:80` and `:85` (the two lock sites)
- **Issue:** Every key's refresh bookkeeping goes through a single
  `Mutex<HashSet<K>>`. Two threads refreshing two completely unrelated keys
  serialise on it, as does every reader that merely *checks* whether a refresh is
  needed. The lock is held across a hash + insert, which is short, but the
  contention is on a single lock word shared by the whole cache.
- **Impact:** Medium — bounded by how often refresh checks happen, but a
  `TimeToRefresh` with a short duration makes this every read.
- **Remediation:** Shard the set (an array of N mutex-guarded sets indexed by
  `hash(key) % N`) or use a concurrent set. Sharding is a contained,
  ~20-line change confined to `TimeToRefresh`, so it stays on the surgical side
  of the line.
- **Evidence:** inferred from code reading.
- **Confirming benchmark:** none exists; `--bench refresh` is single-threaded.
  A multi-threaded refresh bench is the gap.

#### F27. `do_refresh` clones the key twice more

- **Location:** `crates/cachet/src/refresh.rs:113-135`, specifically `:121`
  (`let key = key.clone();` before the spawn) and `:128`
  (`let key = key.clone();` inside the spawned task)
- **Issue:** After `try_start_refresh` has already cloned once (F25), the key is
  cloned again to move into the spawned future, and a third time inside it so the
  `DropGuard` can call `finish_refresh(&key)`. Three clones of `K` per actually-
  performed refresh. The third looks avoidable: the guard and the fetch could
  share one `Arc<K>`, or the guard could take the key by value at the end.
- **Impact:** Low — this is on the refresh path, which is by construction less
  frequent than the read path, and the spawn itself (an allocation plus a task
  registration) dominates a `String` clone.
- **Remediation:** Wrap the key in an `Arc<K>` once at line 121 and clone the
  `Arc` at line 128. Small and local.
- **Evidence:** inferred from code reading.
- **Confirming benchmark:** `cargo bench -p cachet --bench refresh`, comparing
  the "refresh actually fires" case before and after.

#### F28. `FallbackCache::insert` clones the key when it could move it

- **Location:** `crates/cachet/src/fallback.rs:141-148`, specifically `:143`
  and `:144`
- **Issue:** `insert` receives `key: K` **by value** and then writes
  `self.inner.primary.insert(key.clone(), entry.clone())` and
  `self.inner.fallback.insert(key.clone(), entry)`. The second `key.clone()` is
  redundant — nothing uses `key` afterwards, so it can simply be moved. The
  `join!` at line 142 means both futures are alive simultaneously, so one clone
  is genuinely required; the second is not.
- **Impact:** Low — one `K` allocation per fallback insert. Inserts are less hot
  than gets. But the fix is a five-character deletion with provably identical
  behaviour, which makes it a free win.
- **Remediation:** Delete `.clone()` from line 144.
- **Evidence:** inferred from code reading; verified by reading the full method
  body to confirm `key` has no later use.
- **Confirming benchmark:** none directly; `cargo bench -p cachet --bench operations`
  would need a `FallbackCache` insert case, which it lacks.

#### F29. `FallbackCache::get_from_fallback` clones both key and value to promote

- **Location:** `crates/cachet/src/fallback.rs:105-112`, specifically `:110`
- **Issue:** On a fallback hit, the entry is promoted into the primary tier with
  `self.inner.primary.insert(key.clone(), v.clone()).await`. Both clones are
  structurally required — the caller gets the value back and the tier takes
  ownership — so this is not a defect. Recorded because the promotion is also
  **awaited inline**, so a fallback hit's latency includes a full primary-tier
  insert before the caller sees the value. For a fallback tier that is remote
  and a primary that is local this is usually fine; if the primary is itself
  remote it doubles the tail latency of a fallback hit.
- **Impact:** Low — the comment at lines 106-108 shows the design was
  deliberate (errors swallowed, telemetry recorded by the wrapper).
- **Remediation:** Consider spawning the promotion fire-and-forget, as
  `do_refresh` already does (`refresh.rs:125`). This is a behaviour change
  (promotion becomes non-deterministic relative to the return), so it needs a
  decision rather than a patch. Not recommended without measurement.
- **Evidence:** inferred from code reading.
- **Confirming benchmark:** none exists; a fallback-hit latency case is missing
  from `--bench operations`.

#### F30. The main `cachet` benchmark requires a non-default feature that changes what it measures

- **Location:** `crates/cachet/benches/operations.rs`, via its
  `required-features = ["logs", "test-util"]` declaration in
  `crates/cachet/Cargo.toml`
- **Issue:** `logs` is **not** a default feature. The crate's primary
  operations benchmark therefore cannot run without turning on tracing
  instrumentation that a default consumer never compiles. Every number it
  produces includes tracing work no default build performs, which means
  (a) the published figures overstate the cost of a default `cachet`, and
  (b) more damagingly, the benchmark cannot be used to detect the *actual*
  no-telemetry overheads identified in F21, F22, F23 and F24 — the very
  measurements that would justify fixing them. Combined with the
  `--all-features` benchmark build noted in the preamble, the harness
  systematically measures a configuration nobody ships.
- **Impact:** Medium — a benchmark-validity defect rather than a runtime cost,
  but it blocks confirmation of four other findings.
- **Remediation:** Split the bench, or parameterise it: run the same scenarios
  with and without `logs` and report both. `docs/benchmarks.md`'s intent is that
  benchmarks reflect what consumers run.
- **Evidence:** empirically verified (read of `crates/cachet/Cargo.toml`'s
  `[[bench]]` sections).

#### F31. `cachet`'s benchmarks measure `MockCache`, not `cachet`

- **Location:** `crates/cachet/benches/dynamic.rs` and
  `crates/cachet/benches/operations.rs`, both built on
  `crates/cachet_tier/src/testing.rs:227-273`
- **Issue:** See cachet_tier F41 for the full analysis. In short: the storage
  tier both benchmarks use clones the key, acquires three separate mutexes per
  operation, and appends to an unbounded `Vec<CacheOp>` that grows for the
  entire benchmark run. The `Vec`'s geometric reallocation means measured
  per-operation cost *drifts upward* across a Criterion sample, and the three
  mutex acquisitions plus the key clone dwarf whatever `cachet` overhead the
  bench was written to isolate.
- **Impact:** High (as a benchmark-validity defect) — it means the crate's
  existing performance numbers cannot be trusted to reflect `cachet`, which in
  turn is why so many findings in this section are labelled "inferred" with no
  usable confirming benchmark.
- **Remediation:** Add a minimal no-op or `HashMap`-backed bench-only tier with
  no recording and no locking, and use it as the storage for all `cachet`
  benchmarks. `MockCache` remains the right tool for tests.
- **Evidence:** inferred from code reading of `testing.rs` and the two bench
  files.

#### F32. `encode` calls `BytesBuf::peek()` and clones the pool per encode

- **Location:** `crates/cachet/src/serialize/codec.rs:95-100`, specifically
  `pool.clone()` at `:96` and `writer.into_inner().peek()` at `:99`
- **Issue:** `pool.clone()` is a `GlobalPool` clone — an `Arc` bump, so cheap
  but not free, and it happens per encode. More notably, `.peek()` on the
  finished buffer constructs a fresh `SmallVec` of spans
  (`crates/bytesbuf/src/buf.rs:437`, bytesbuf F6); for a serialised payload
  large enough to span more than eight blocks that allocates.
- **Impact:** Low — serialisation cost (postcard) dominates, and the multi-span
  case needs a large value.
- **Remediation:** None urgent. Fixing bytesbuf F6 (`peek` capacity hint)
  removes the span-vector allocation here for free.
- **Evidence:** inferred from code reading.
- **Confirming benchmark:** none exists — there is no serialisation benchmark in
  `cachet` at all, despite `serialize` being an optional but non-trivial layer.

#### F33. `cachet` has zero Callgrind benchmarks

- **Location:** `crates/cachet/benches/` (three files, all Criterion)
- **Issue:** See "Benchmark coverage". Called out as a finding because the
  overheads this crate actually has — a TLS read (F23), an atomic increment
  (F21), a clock read (F24) — are each a handful of instructions. Criterion's
  wall-clock noise floor is far above that. `docs/callgrind-benchmarks.md`
  exists precisely for this case, and `bytesbuf` follows it with three paired
  `_cg.rs` files. `cachet` has none, so its small-constant overheads are
  structurally unmeasurable.
- **Impact:** Medium — blocks confirmation of the crate's most actionable
  findings.
- **Remediation:** Add `operations_cg.rs` paired with `operations.rs` per
  `docs/naming.md`.
- **Evidence:** empirically verified (bench directory census).

### Benchmark coverage

Files present in `crates/cachet/benches/`:

| File | Kind | Notes |
|------|------|-------|
| `operations.rs` | Criterion | `required-features = ["logs", "test-util"]` — see F30 |
| `dynamic.rs` | Criterion | measures `DynCacheTier` dispatch cost |
| `refresh.rs` | Criterion | single-threaded only |

**No Callgrind/Gungraun benchmarks exist** (F33), so the three paired-file
requirements of `docs/naming.md` are moot but the guidance in
`docs/callgrind-benchmarks.md` is unmet for a crate whose overheads are
instruction-scale.

Criterion group names do not follow `docs/naming.md`'s
`<file-basename>/<subgroup>` convention: the groups are named
`"cache_operations"`, `"dynamic_cache"`, `"wrapper_overhead"` and
`"refresh_overhead"` rather than `operations/...`, `dynamic/...`,
`refresh/...`. This is cosmetic but it defeats the cross-referencing that the
naming rule exists to enable.

Gaps:

1. **No multi-threaded benchmark whatsoever.** For a *caching library* this is
   the single largest coverage hole: F21 (global atomic), F26 (global refresh
   mutex) and all lock-granularity questions are unmeasurable.
2. **Benchmarks measure `MockCache`, not `cachet`** (F31).
3. **No no-telemetry configuration** is benchmarked (F30), so F21–F24 cannot be
   quantified.
4. **`serialize` and `transform` layers are unbenchmarked** despite both sitting
   directly in the value path when enabled.
5. **`FallbackCache` has no benchmark** — neither the primary-hit fast path
   (`fallback.rs:124-138`) nor the fallback-hit promotion path (F29).
6. **No hit-rate sweep.** Every scenario is either all-hit or all-miss; the
   interesting behaviour of `get_or_insert_with` is at intermediate hit rates.

### Considered and ruled out

* **`InsertPolicy::should_insert` (`crates/cachet/src/policy.rs:109-110`).**
  Already `#[inline]`, and it is `pub(crate)`. Correct as written.
* **`EvictionListener::handle` (`crates/cachet/src/eviction.rs:47`).** Checks
  an initialisation flag first and does nothing before `init`. No allocation, no
  locking. Fine.
* **`to_contiguous` (`crates/cachet/src/serialize/codec.rs:104-115`).**
  Genuinely zero-copy for the single-span case via `Cow::Borrowed`, and the
  multi-span fallback uses `Vec::with_capacity(view.len())` — exactly right.
  The doc comment even states the intent. This is a model of how the rest of the
  crate should handle borrowed data, and is worth citing when arguing F19.
* **`CacheWrapper::insert` checking `should_insert` before starting the
  stopwatch (`crates/cachet/src/wrapper.rs:142-147`).** Correctly ordered — the
  rejected path pays no timing cost. This shows the crate already knows the
  pattern F24 asks for; it is just not applied on the read path.
* **Absence of `#[inline]` throughout `cachet`.** Nearly every function here
  lives in a generic `impl<K, V, ...>` block, so it is monomorphised into the
  consumer and already inlinable. `docs/performance.md` rule 1 (small public
  functions crossing a crate boundary) does **not** apply; rule 2
  (measurement-first) does. Not a finding — and this is the key distinction
  between `cachet` and `bytesbuf`, where the concrete non-generic `BytesBuf`/
  `BytesView` types genuinely do need annotation.
* **`Ordering::Relaxed` on `NEXT_REQUEST_ID` (`telemetry/cache.rs:28`).** The
  ordering is correct — the counter carries no data dependency. F21 is about
  the existence of the atomic, not its ordering; strengthening or weakening the
  ordering would change nothing.
* **`RestoreRequestId` as a drop guard rather than a plain restore
  (`telemetry/cache.rs:45-53`).** Needed for panic-unwind correctness, as the
  comment says. Correct; F22 targets the wrapper's unconditional application,
  not the guard.
* **`join!` in `FallbackCache::insert` (`fallback.rs:142`).** Correct — the two
  tier inserts are independent and concurrency is the right call.
* **`DropGuard` in `refresh.rs` (`:145-152`).** Same reasoning: panic safety.
* **Error paths generally (`Error::from_source`, `Error::caused_by`).** Cold by
  construction. Not worth `#[cold]` annotations without measurement.
* **`postcard` as the serialisation dependency
  (`crates/cachet/src/serialize/codec.rs`).** A compact, no-std, zero-alloc-
  friendly format — a well-justified ecosystem choice for a cache codec.

---

## Crate: cachet_memory

### Summary

`cachet_memory` is a thin adapter that presents `moka::future::Cache` as a
`cachet_tier::CacheTier`. It is 1,493 source lines, most of which is the
builder (641) and tests. The adapter layer itself is genuinely thin — `get`,
`invalidate`, `clear` and `len` are one-line delegations — and that is the right
design: the heavy lifting (TinyLFU admission, sharded concurrent map, expiry
wheel) belongs to `moka`, a well-established and well-optimised ecosystem
choice, and using it rather than hand-rolling is exactly what
`docs/performance.md` asks for.

There is one clear, free win (a redundant clone on `insert`), one structural
cost inherited from the `CacheTier` trait shape, and a benchmark file whose
design is otherwise a model for the rest of the group — it compares
`cachet_memory` against raw `moka` side by side, which is precisely how you
measure an adapter's overhead.

### Findings

#### F34. `InMemoryCache::insert` clones an owned key

- **Location:** `crates/cachet_memory/src/tier.rs:201-204`, specifically
  `self.inner.insert(key.clone(), entry).await;` at `:202`
- **Issue:** `insert` takes `key: K` **by value** and `moka`'s
  `Cache::insert` also takes `K` by value, yet the adapter clones. `key` is
  never used again — the next statement is `Ok(())`. The clone is pure waste on
  every insert into the primary in-memory tier, which is the most common tier
  configuration in the workspace.
- **Impact:** Medium — one full `K` allocation (a `String` malloc + memcpy for
  the common key type) per insert, on the write path of the default cache
  backend. Small in absolute terms but entirely free to remove.
- **Remediation:** Delete `.clone()`. Zero behaviour change; the borrow checker
  will confirm it.
- **Evidence:** inferred from code reading; verified by reading the full method
  body (lines 201-204) to confirm `key` has no subsequent use, and by confirming
  `moka::future::Cache::insert` takes `K` by value from its use elsewhere in the
  file.
- **Confirming benchmark:** `cargo bench -p cachet_memory --bench overhead`
  already has an `insert` group comparing `moka` against `cachet_memory`
  (`crates/cachet_memory/benches/overhead.rs:108-142`). Removing the clone
  should visibly narrow the gap between the two arms. This is the single
  cheapest empirical confirmation available anywhere in this group.

#### F35. `CacheTier::get`'s `&K` signature forces `moka`'s owned-key lookup

- **Location:** `crates/cachet_memory/src/tier.rs:197-199`
- **Issue:** `Ok(self.inner.get(key).await)` is correct and allocation-free
  *here* — `moka::Cache::get` accepts `&Q where K: Borrow<Q>`, so `moka` itself
  supports the borrowed lookup. The allocation is imposed one layer up, by
  `CacheTier::get(&self, key: &K)` (`crates/cachet_tier/src/tier.rs:46`), which
  forces `cachet::Cache::get` to materialise an owned `K` before it can even
  reach this line (F19). Recorded here because it demonstrates that the backing
  store is *not* the constraint: `moka` already offers the fast path, and the
  `cachet` trait definition is what throws it away.
- **Impact:** High — it is the same allocation as F19, but this finding is what
  makes the fix worth doing: the payoff is real because the bottom of the stack
  can already accept a borrowed key.
- **Remediation:** See cachet_tier F39. `cachet_memory` needs no change once the
  trait is generalised — `self.inner.get(key)` compiles unchanged for a `&Q`.
- **Evidence:** inferred from code reading.
- **Philosophy note:** **Conflicting** for the same reason as F19 — the fix is
  architectural, not surgical.
- **Confirming benchmark:** `--bench overhead`'s `get_hit` group already
  compares `moka` directly against `cachet_memory`; extending it with a
  `cachet::Cache`-on-top arm would show the allocation the trait imposes.

#### F36. The eviction listener runs a dynamic call per registered observer per eviction

- **Location:** `crates/cachet_memory/src/tier.rs:166-178`
- **Issue:** When any listener or observer is registered, the closure installed
  at line 169 runs `crate::notification::from_moka(moka_cause)` (a cheap match,
  `notification.rs:27-40`), then iterates `for observer in &removal_observers`
  making an indirect call per observer, then makes a further indirect call to
  the value listener with `entry.into_value()`. Each observer is a boxed
  closure, so each is an unpredictable indirect branch.
- **Impact:** Low — evictions are, by construction, rarer than gets, and the
  guard at line 168 means zero cost when no listener is registered (the common
  case). Worth recording only because the eviction listener runs on `moka`'s
  maintenance path, which can run inline on a caller's `insert`.
- **Remediation:** None recommended. The guard already handles the common case
  correctly, and collapsing the observer list would trade flexibility for a
  branch that is not on the read path.
- **Evidence:** inferred from code reading.
- **Confirming benchmark:** none exists — `--bench overhead` never triggers an
  eviction (it does not configure `max_capacity` in the hot groups). An
  eviction-pressure scenario is the main coverage gap here.

#### F37. `EntryExpiry` calls `CacheEntry::ttl()` on every create and update

- **Location:** `crates/cachet_memory/src/tier.rs:221-236`
- **Issue:** `moka`'s per-entry `Expiry` trait is invoked on every insert and
  every update, and both implementations here return `value.ttl()`. `ttl()`
  reads the `Option<Duration>` field of `CacheEntry<V>`
  (`crates/cachet_tier/src/entry.rs`), a 16-byte cold field that lives in the
  same allocation as the value. Because `moka` calls `Expiry` on the maintenance
  path *after* the value has been stored, this touches the entry a second time,
  pulling cold metadata into cache when only the TTL is wanted.
- **Impact:** Low — the entry is almost certainly still hot from the insert
  itself. Recorded for completeness and because it interacts with the
  `CacheEntry` layout finding (cachet_tier F42): if the cold metadata were
  compacted, this access would be cheaper still.
- **Remediation:** None. Per-entry expiry is the reason this adapter exists;
  the implementation is minimal.
- **Evidence:** inferred from code reading; layout measured (see cachet_tier
  F42).

#### F38. `cachet_memory` has no Callgrind benchmark and non-conforming group names

- **Location:** `crates/cachet_memory/benches/overhead.rs:24`, `:72`, `:110`
- **Issue:** The Criterion groups are named `"get_hit"`, `"get_miss"` and
  `"insert"` rather than `overhead/get_hit`, `overhead/get_miss`,
  `overhead/insert` as `docs/naming.md` requires (`<file-basename>/<subgroup>`).
  Separately, there is no paired `overhead_cg.rs`, so the adapter's overhead —
  which is by design *small*, a handful of instructions over raw `moka` — is
  measured only by wall clock, where it sits near or below the noise floor. This
  is the canonical case `docs/callgrind-benchmarks.md` was written for: a thin
  wrapper whose whole value proposition is that it costs almost nothing.
- **Impact:** Medium — as a benchmark-validity issue. The bench's design
  (`moka` vs `cachet_memory`, same scenario) is exactly right; it is just using
  the wrong instrument for the size of the effect.
- **Remediation:** Add `crates/cachet_memory/benches/overhead_cg.rs` pairing the
  same three scenarios, and rename the Criterion groups to the `overhead/…`
  form.
- **Evidence:** empirically verified (read of the bench file and
  `docs/naming.md`).

### Benchmark coverage

| File | Kind | Groups |
|------|------|--------|
| `overhead.rs` | Criterion | `get_hit` (moka / cachet_memory), `get_miss` (moka / cachet_memory), `insert` (moka / cachet_memory) |

**No Callgrind benchmark** (F38).

What is good, and should be copied by the rest of the group: every group
benchmarks the *baseline* (`moka`) alongside the wrapper, so the number produced
is an overhead figure rather than an uninterpretable absolute. `cachet`'s
benchmarks do not do this and are much less useful as a result.

Gaps:

1. **No multi-threaded scenario.** `moka` is a concurrent cache; its whole
   design is about scaling across cores, and the adapter is never tested under
   concurrency. This is the largest gap.
2. **No eviction pressure.** No group configures `max_capacity`, so TinyLFU
   admission, the eviction listener (F36) and the maintenance path are all
   unmeasured.
3. **No TTL/expiry scenario.** `EntryExpiry` (F37) is the crate's main piece of
   custom logic on the hot path and is not benchmarked at all.
4. **No custom-hasher scenario.** The crate depends on `foldhash` and supports a
   generic `H: BuildHasher`; the performance difference between the default and
   an alternative hasher is a natural thing to measure and is not measured.
5. **`get_miss` uses a fixed absent key**, so it always takes the same branch
   and the same hash bucket — not representative of a real miss distribution.

### Considered and ruled out

* **Depending on `moka` rather than hand-rolling a concurrent cache.** Exactly
  the ecosystem-default choice `docs/performance.md` asks for. `moka` provides
  sharding, TinyLFU admission and a timer wheel that would take years to
  reproduce. Not a finding.
* **`foldhash` as the default hasher (`Cargo.toml` dependency,
  `builder.rs`).** A fast, non-cryptographic hasher — the right default for a
  cache keyed by application-controlled data, and the generic `H` parameter lets
  a caller substitute a DoS-resistant hasher where keys are attacker-controlled.
  Well judged.
* **`Arc<..., PerProcess>` from `thread_aware` (`tier.rs:57`).** A thread-aware
  `Arc` with per-process semantics; the annotation is about correctness of
  sharing, not a performance cost over `std::sync::Arc`. Not a finding.
* **`clear` implemented as `invalidate_all` (`tier.rs:211-214`).** `moka`'s
  `invalidate_all` is O(1) — it bumps a generation counter and lets entries be
  reclaimed lazily. That is *faster* than a real clear, and the right call.
* **`len` implemented as `entry_count` (`tier.rs:216-218`).** Documented as
  approximate by the `CacheTier` trait, and `moka`'s `entry_count` is a cheap
  approximate read rather than a full traversal. Correct.
* **Absence of `#[inline]` on `InMemoryCacheBuilder`'s thirteen public
  methods (`builder.rs`).** All are in a generic `impl`, so they monomorphise
  into the caller; and all are on the *construction* path, which
  `docs/performance.md` explicitly deprioritises ("first-insert/teardown"). Two
  independent reasons not to flag it.
* **`InMemoryCache::get` returning an owned `CacheEntry<V>` (`tier.rs:197`).**
  `moka` clones the value out of the map on read; that is inherent to a
  concurrent map that cannot hand out references outliving the guard. The
  `CacheTier` trait's `Result<Option<CacheEntry<V>>>` return is therefore not
  imposing a cost `moka` was not already paying. A zero-copy read would require
  `Arc<V>` values throughout, which is an architectural change with its own
  costs.
* **`notification::from_moka` (`notification.rs:27-40`).** A total match over a
  small enum, compiles to a jump table or better. Nothing to do.

---

## Crate: cachet_tier

### Summary

`cachet_tier` is small (1,054 lines across six files) but it is the most
consequential crate in this group, because it defines the `CacheTier` trait that
every other crate here is shaped by. Two decisions in a single 60-line file
(`src/tier.rs`) account for the group's top two findings: the `&K` parameter on
`get`/`invalidate`, which forces an owned-key allocation on every lookup at the
`cachet` layer, and the `dyn(box)` dynosaur erasure, which boxes the returned
future once per tier per operation.

It also ships `MockCache` behind the `test-util` feature, which is a perfectly
good test double but is used as the *storage backend* for `cachet`'s
benchmarks — where it invalidates the results.

`CacheEntry<V>` is defined here, and its layout was measured empirically: it
carries a fixed 32 bytes of cold metadata regardless of `V`.

### Findings

#### F39. `CacheTier::get` and `invalidate` take `&K`, not a borrowed form

- **Location:** `crates/cachet_tier/src/tier.rs:46` (`get`) and `:54`
  (`invalidate`)
- **Issue:** `fn get(&self, key: &K) -> impl Future<...>`. Because the parameter
  is `&K` rather than a `&Q where K: Borrow<Q>`, every caller that holds a
  borrowed key of a different-but-borrowable type must materialise an owned `K`
  first. `cachet::Cache::get` does exactly that at
  `crates/cachet/src/cache.rs:237` and `:246`, and at seven further sites
  (F19, F20). The backing store does *not* require this — `moka::Cache::get`
  already accepts `&Q` (`crates/cachet_memory/src/tier.rs:197`, F35) — so the
  allocation is imposed purely by this trait signature and thrown away
  immediately below it.
- **Impact:** High — this is the root cause of the group's top finding. One heap
  allocation per cache hit for the standard `Cache<String, V>` keyed by `&str`.
- **Remediation:** Generalising to `fn get<Q>(&self, key: &Q) where K: Borrow<Q>`
  is the correct signature but is **not object-safe**, and the trait is erased
  via `dyn(box)` at line 38 — so this cannot be done without also changing F40.
  A narrower alternative: add a provided method
  `fn get_borrowed(&self, key: &Q)` only on the non-erased path, or introduce a
  separate object-safe `get_by_hash`-style entry point. Neither is small.
- **Evidence:** inferred from code reading; signature verified directly, and
  the downstream `to_owned()` sites verified by grep in `cachet/src/cache.rs`.
- **Philosophy note:** **Conflicting.** `docs/performance.md` states that
  allocation is the root of all evil and that nothing should allocate on the hot
  path; it *also* says to prefer surgical interventions over architectural
  rewrites. Those two rules point in opposite directions here. This needs a
  maintainer decision. Reported, not patched.
- **Confirming benchmark:** a Callgrind bench with a no-op tier (none exists —
  `cachet_tier` has no benchmarks at all, F44) comparing a `&str` lookup against
  a `&String` lookup; the malloc/free pair is unambiguous in instruction counts.

#### F40. `dyn(box)` erasure boxes a future per tier per operation

- **Location:** `crates/cachet_tier/src/tier.rs:38` —
  `#[dynosaur::dynosaur(pub(crate) DynCacheTier = dyn(box) CacheTier, bridge(none))]`
- **Issue:** The `CacheTier` trait uses RPITIT
  (`-> impl Future<Output = ...> + Send`), which is allocation-free when used
  generically. The `dynosaur` attribute produces an object-safe `DynCacheTier`
  by **boxing** the returned future. `cachet::Cache` stores its tier as a
  `DynCacheTier`, so every `get`, `insert`, `invalidate`, `clear` and `len`
  allocates a `Box<dyn Future>` — and in a multi-tier stack, once per tier.
  Combined with F39, a two-tier `get` performs at least three heap allocations
  before any work is done.
- **Impact:** High — a second unavoidable allocation on the hottest path, and it
  scales with stack depth. Also costs an indirect call and defeats cross-tier
  inlining.
- **Remediation:** The genuine fix is to stop erasing: make `cachet::Cache`
  generic over `CT: CacheTier<K, V>` so the RPITIT future stays unboxed and the
  whole stack inlines. That is what the trait was already written for. The cost
  is monomorphisation bloat and a `Cache<K, V, CT>` type parameter leaking into
  every consumer signature — a large, source-breaking change. A middle path:
  keep `DynamicCache` for consumers who genuinely need runtime tier selection,
  but make the common single-tier construction monomorphic. `DynamicCache`
  already exists for exactly this purpose (`src/dynamic.rs:26`), which suggests
  the erasure at the `Cache` level was not strictly necessary.
- **Evidence:** inferred from code reading; the `dyn(box)` mode is explicit in
  the attribute at line 38, and `dynosaur`'s `dyn(box)` is documented to box the
  future.
- **Philosophy note:** **Conflicting**, as F39 — the correct fix is
  architectural and source-breaking.
- **Confirming benchmark:** `cargo bench -p cachet --bench dynamic` already
  measures dynamic dispatch and is the closest existing measurement, but it is
  built on `MockCache` (F41) so the boxing cost is swamped. A clean measurement
  needs a no-op tier and a generic-vs-erased A/B.

#### F41. `MockCache` is used as the storage backend for `cachet`'s benchmarks

- **Location:** `crates/cachet_tier/src/testing.rs:224-277`, in particular `get`
  at `:229-236`; used by `crates/cachet/benches/dynamic.rs` and
  `crates/cachet/benches/operations.rs`
- **Issue:** `MockCache::get` does all of the following per call:
  1. `key.clone()` to build a `CacheOp::Get` (line 230) — a heap allocation for
     `String` keys;
  2. `self.should_fail(&op)` — acquires the `fail_when` mutex (line 219-221);
  3. `self.record(op)` — acquires the `operations` mutex and **pushes onto an
     unbounded `Vec<CacheOp<K, V>>`** (line 216);
  4. `self.data.lock()` — a third mutex acquisition — then `.get(key).cloned()`.

  `insert` is worse: it additionally clones the `CacheEntry<V>` (line 240-242).
  The unbounded `operations` `Vec` is the serious part: across a Criterion run
  of hundreds of thousands of iterations it grows to hundreds of thousands of
  cloned keys, reallocating geometrically. Each realloc is a large memcpy
  attributed to whichever iteration triggers it, and the growing live set
  degrades allocator and cache behaviour monotonically through the run. The
  measured per-operation cost therefore *drifts upward* within a sample, which
  is exactly the failure mode Criterion's statistics assume away.
- **Impact:** High **as a benchmark-validity defect**. `MockCache` itself is a
  fine test double and its cost is irrelevant in tests. But it means the
  published numbers for `cachet`'s `operations` and `dynamic` benchmarks are
  dominated by three mutex acquisitions, a key clone and an unbounded `Vec`
  push, not by `cachet`. Every real overhead identified in the `cachet` section
  is smaller than the noise this introduces.
- **Remediation:** Add a bench-only no-op or `HashMap`-backed tier with no
  recording and no locking, and use it as the storage for all `cachet`
  benchmarks. Leave `MockCache` for tests, where recording is the point.
  Optionally add a `MockCache::without_recording()` constructor.
- **Evidence:** inferred from code reading; the three lock acquisitions and the
  unbounded `Vec` push were verified by reading `record`, `should_fail` and the
  `CacheTier` impl in full (lines 215-277), and the bench usage by reading both
  bench files.
- **Confirming benchmark:** re-run `cargo bench -p cachet --bench operations`
  with a no-op tier substituted; the absolute numbers should drop sharply and
  the variance should collapse.

#### F42. `CacheEntry<V>` carries 32 bytes of cold metadata regardless of `V`

- **Location:** `crates/cachet_tier/src/entry.rs:38-43`
- **Issue:** The struct is
  `{ value: V, cached_at: Option<SystemTime>, ttl: Option<Duration> }`.
  Measured on x86-64 Linux with layout-identical replicas compiled by plain
  `rustc`:

  ```
  SystemTime = 16   Option<SystemTime> = 16   Duration = 16   Option<Duration> = 16
  CacheEntry<()>     = 32
  CacheEntry<u32>    = 40
  CacheEntry<String> = 56
  Option<CacheEntry<u32>>    = 40   (niche exploited)
  Option<CacheEntry<String>> = 56   (niche exploited)
  ```

  So a 4-byte value occupies 40 bytes — 36 bytes of overhead, a 10× inflation.
  Both metadata fields are **cold**: `cached_at` is read only by expiry checks
  and `ttl` only by `EntryExpiry` (`cachet_memory/src/tier.rs:221-236`), while
  `value` is read on every hit. Because they are interleaved in one allocation,
  every hit pulls the cold metadata into L1 alongside the value, and the cache
  stores 40 bytes per entry where 8 would do.
- **Impact:** Medium — for small values this materially reduces the number of
  entries that fit in a given amount of L2/L3, which is the whole point of an
  in-memory cache. For large values (the case `crates/bytesbuf/AGENTS.md`
  cares about) it is negligible.
- **Remediation:** The metadata could be compressed to 12 bytes — a `u64` of
  epoch-nanoseconds (good until year 2554) plus a `u32` of TTL seconds, with
  sentinel values for `None` — bringing `CacheEntry<u32>` to 24 bytes or, with
  reordering, 16. **But** this replaces two self-documenting `Option<std::time>`
  fields with hand-packed integers and sentinels.
- **Evidence:** **empirically verified** — measured with a dependency-free
  `rustc -O` program containing layout-identical replicas, printing `size_of`
  and `align_of`. (The program was written to `/tmp`, run, and deleted; nothing
  was added to the repo.)
- **Philosophy note:** **Conflicting.** `docs/performance.md` requires staying
  idiomatic and justifying deviations from ecosystem patterns.
  `Option<SystemTime>` / `Option<Duration>` are the idiomatic representation and
  a hand-packed integer encoding is not. This should only be done with a
  measured demonstration that entry density is a real bottleneck — which no
  existing benchmark provides.
- **Confirming benchmark:** none exists. The measurement needed is a
  working-set sweep: fill a capacity-bounded `InMemoryCache<String, u32>` and
  measure hit throughput as the working set crosses L2 and L3, before and after
  compaction.

#### F43. `DynamicCache` implements `CacheTier`, so nesting it double-boxes

- **Location:** `crates/cachet_tier/src/dynamic.rs:49-68`
- **Issue:** `DynamicCache<K, V>` wraps `Arc<DynCacheTier<'static, K, V>>` — an
  already-erased, already-boxing tier — and then itself implements `CacheTier`.
  Every method is a one-line forward (`self.0.get(key).await`). If a
  `DynamicCache` is then handed to `cachet::Cache`, which erases it *again* via
  `DynCacheTier`, the future is boxed twice per operation: once by the outer
  erasure of `DynamicCache`, once by the inner `DynCacheTier`. Nothing in the
  type system or documentation prevents this, and it is the natural thing to
  write when a caller wants a cloneable tier.
- **Impact:** Medium — doubles the F40 cost for a configuration that looks
  idiomatic. Whether it bites depends on how `cachet::Cache` is constructed;
  the builder makes it easy to hit.
- **Remediation:** Either document the hazard prominently on `DynamicCache`, or
  have the builder detect the `DynamicCache` case and use its inner
  `DynCacheTier` directly rather than re-erasing. The latter is contained and
  surgical.
- **Evidence:** inferred from code reading.
- **Confirming benchmark:** `cargo bench -p cachet --bench dynamic` is the right
  place; it would need a nested-`DynamicCache` arm, which it lacks.

#### F44. `cachet_tier` has no benchmarks at all

- **Location:** `crates/cachet_tier/` — no `benches/` directory
- **Issue:** The crate that defines the trait whose shape imposes the group's
  two largest costs (F39, F40) has no benchmark of any kind, Criterion or
  Callgrind. There is consequently no measurement anywhere in the workspace of
  what `dyn(box)` erasure costs versus a monomorphic `impl CacheTier`, which is
  the single number a maintainer would need to decide F40.
- **Impact:** Medium — a coverage gap that directly blocks the two most valuable
  decisions in this group.
- **Remediation:** Add `crates/cachet_tier/benches/erasure.rs` (Criterion) and
  `erasure_cg.rs` (Callgrind) comparing, over a trivial no-op tier: a direct
  generic call, a `DynCacheTier` call, and a nested `DynamicCache` call. That is
  a small file and it would settle F40 and F43 empirically.
- **Evidence:** empirically verified (directory listing).

#### F45. `CacheEntry`'s ten small public accessors carry no `#[inline]`

- **Location:** `crates/cachet_tier/src/entry.rs:49`, `:61`, `:74`, `:87`,
  `:96`, `:107`, `:112`, `:118`, `:124`, `:133`
- **Issue:** `cached_at()`, `ttl()`, `value()`, `into_value()`, `set_ttl()`,
  `ensure_cached_at()` and the three constructors are all one-or-two-line
  field accesses on a `pub struct`, called across crate boundaries from
  `cachet`, `cachet_memory` and `cachet_service`. None is annotated
  `#[inline]`.
- **Impact:** Low — and this needs an important qualification. `docs/performance.md`
  rule 1 (annotate small public functions that cross a crate boundary) applies
  to *non-generic* functions, because generic functions are monomorphised in the
  consumer crate and are already inlinable. `CacheEntry<V>` is generic, so the
  `impl<V>` block's methods **are** available for inlining without annotation.
  Rule 1 therefore does **not** apply, and rule 2 (measure first) does. Recorded
  at Low only because `ttl()` is called on the `moka` maintenance path
  (`cachet_memory/src/tier.rs:224`, `:234`) where the calling crate is different
  — but monomorphisation still covers it.
- **Impact qualifier:** contrast with `bytesbuf`, where `BytesBuf` and
  `BytesView` are **concrete** types and rule 1 genuinely applies (F9).
- **Remediation:** None recommended without measurement.
- **Evidence:** inferred from code reading; `#[inline]` census done by grep
  (10 public functions, 0 annotations).
- **Confirming benchmark:** the `erasure_cg.rs` proposed in F44 would also
  reveal whether these calls survive as real calls.

### Benchmark coverage

**None.** `crates/cachet_tier/` has no `benches/` directory, no Criterion
benchmarks and no Callgrind benchmarks (F44).

This is the most serious coverage gap in the group in proportion to
consequence: the crate is only 1,054 lines, but its trait signature (F39) and
erasure strategy (F40) determine the allocation behaviour of the entire caching
stack, and neither is measured anywhere.

The minimum viable coverage would be:

1. `benches/erasure.rs` + `benches/erasure_cg.rs` — generic vs `DynCacheTier`
   vs nested `DynamicCache`, over a no-op tier. Settles F40 and F43.
2. A borrowed-vs-owned key lookup pair. Settles F39.
3. `CacheEntry` construction and accessor cost. Settles F45 and gives F42 a
   baseline.

Note also that `cachet`'s existing benchmarks are, in effect, `cachet_tier`
benchmarks measured through a hostile mock (F41), so the numbers currently
attributed to `cachet_tier`'s design are not usable.

### Considered and ruled out

* **`Error`'s `Box<dyn StdError + Send + Sync>` source
  (`crates/cachet_tier/src/error.rs:46-51`, `:74`, `:89`).** Boxing the source
  keeps `Error` small and moves the allocation onto the error path, which is
  cold by construction. This is the standard ecosystem pattern and the right
  trade — the alternative (an enum of concrete sources) would inflate every
  `Result` on the success path. Correct as written.
* **`SizeError::unsupported()` (`error.rs:189`).** Constructs a unit-kind error
  with no allocation, and is the default return of `CacheTier::len`. Cheap.
* **`Error::is_source` / `source_as` (`error.rs:126`, `:148`).** Downcast
  helpers on the error path only.
* **`test-util` gating `parking_lot` (`Cargo.toml:31-32`,
  `:37`).** `MockCache` and its `parking_lot` dependency are correctly behind an
  off-by-default feature, so no production consumer pays for them. The problem
  in F41 is that the *benchmarks* enable it and then use it as storage — the
  feature gating itself is right.
* **`bridge(none)` in the dynosaur attribute (`tier.rs:38`).** Suppresses
  generating a blanket bridge impl; this reduces generated code rather than
  adding cost. Not a finding.
* **The default `len`/`is_empty` provided methods (`tier.rs:58-96`).** `len`
  defaults to `Err(SizeError::unsupported())` and `is_empty` delegates. Neither
  is on a hot path, and the default avoids forcing every tier to track size —
  which would be a real cost. Well judged.
* **`DynamicCache` using `Arc` rather than `Rc` (`dynamic.rs:26`).** Required:
  the trait is `Send + Sync`. Not a choice.
* **`CacheEntry::try_map_value` (`entry.rs:133-139`).** Moves the value through
  the closure and copies the two metadata fields by value; no allocation, no
  clone. Correct.

---

## Crate: cachet_service

### Summary

`cachet_service` (648 lines across four files) bridges `cachet_tier::CacheTier`
to the `layered::Service` abstraction, so a cache tier can be composed with
middleware (retry, timeout, circuit breaking). It offers two paths that do the
same thing: `ServiceAdapter` (a concrete wrapper implementing `CacheTier`) and
`CacheServiceExt` (a blanket extension trait on any suitable `Service`).

The crate is thin and the design is sound, but it converts every operation into
an **owned request value** — `CacheOperation<K, V>` — which means it is the one
place in the group where the owned-key allocation is genuinely unavoidable
rather than accidental. Two of the six clones are nonetheless redundant.

Empirically measured type sizes (x86-64, `rustc` replicas):
`CacheOperation<String, String> = 80`, `CacheResponse<String> = 56`.

### Findings

#### F46. `ServiceAdapter::insert` clones a key it already owns

- **Location:** `crates/cachet_service/src/adapter.rs:75-81`, specifically
  `:76`
- **Issue:** `async fn insert(&self, key: K, entry: CacheEntry<V>)` receives
  `key` **by value**, and `InsertRequest::new(key: K, entry: CacheEntry<V>)`
  (`crates/cachet_service/src/request.rs:52`) also takes it by value — yet the
  call site writes `InsertRequest::new(key.clone(), entry)`. `key` is not used
  again; the following line consumes `request`. The clone is unconditional waste
  on every insert through a service-backed tier, which for a remote cache is the
  write path to the network.
- **Impact:** Medium — one full `K` allocation per insert, trivially avoidable.
  Lower than it looks in relative terms because a service-backed tier usually
  involves I/O that dwarfs a `String` clone; but it is free to remove.
- **Remediation:** Delete `.clone()` at line 76. The borrow checker verifies
  correctness; zero behaviour change.
- **Evidence:** inferred from code reading; the by-value parameter and the
  absence of later use both verified in the method body (lines 75-81), and
  `InsertRequest::new`'s by-value signature verified at `request.rs:52`.
- **Confirming benchmark:** none exists — `cachet_service` has no benchmarks at
  all (F49). A trivial no-op `Service` plus an insert loop would show it.

#### F47. `CacheServiceExt::insert` clones a key it already owns

- **Location:** `crates/cachet_service/src/ext.rs:38-44`, specifically `:39`
- **Issue:** Identical to F46 in the extension-trait path:
  `let req = InsertRequest { key: key.clone(), entry };` where `key: K` is
  already owned by value and never used again. Here it is a struct-literal
  initialisation rather than a constructor call, so the redundancy is even more
  visible.
- **Impact:** Medium — same as F46. Recorded separately because the two code
  paths are independent and fixing one does not fix the other; a caller using
  the extension trait directly never touches `ServiceAdapter`.
- **Remediation:** Delete `.clone()` at line 39, i.e.
  `let req = InsertRequest { key, entry };`.
- **Evidence:** inferred from code reading; verified in the method body.
- **Confirming benchmark:** as F46.

#### F48. The `get` and `invalidate` clones are forced by the owned-request design

- **Location:** `crates/cachet_service/src/adapter.rs:68`, `:84`;
  `crates/cachet_service/src/ext.rs:31`, `:47`
- **Issue:** Unlike F46/F47, these four clones are **not** redundant: `get` and
  `invalidate` receive `key: &K` (dictated by `CacheTier`, cachet_tier F39) and
  must produce an owned `CacheOperation<K, V>` for `Service::execute`. So the
  allocation is structural. It is worth recording because it compounds: a
  `cachet::Cache::get` through a service tier allocates once at
  `cache.rs:237/246` to satisfy `CacheTier::get(&K)` (F19), boxes a future for
  the `dyn(box)` erasure (F40), and then allocates the key *again* here to build
  the request. Three allocations before the service is even called.
- **Impact:** Medium — the third allocation is the one this crate could remove,
  and only by borrowing in the request type.
- **Remediation:** Make the request types borrow: `GetRequest<'a, K> { key: &'a K }`
  or `Cow<'a, K>`. This changes a public type shape and interacts with
  `layered::Service`'s lifetime handling, so it is not a small change; and a
  service that dispatches the request to another thread or task genuinely needs
  ownership. A `Cow` is the honest middle ground: borrow for in-process
  middleware, own only where the request outlives the call. Not recommended
  without measurement.
- **Evidence:** inferred from code reading.
- **Philosophy note:** **Conflicting** if pursued — changing the public request
  types to borrowed forms is architectural, and `Cow<K>` in a public API is
  less idiomatic than an owned key. Reported so the compounding is visible, not
  as a recommended patch.
- **Confirming benchmark:** none exists (F49).

#### F49. `cachet_service` has no benchmarks at all

- **Location:** `crates/cachet_service/` — no `benches/` directory, and no
  `[[bench]]` entries in `Cargo.toml`
- **Issue:** No Criterion and no Callgrind coverage. The crate's entire value
  proposition is that wrapping a tier in the `layered::Service` abstraction is
  cheap enough to be worth the composability, and that claim is untested. In
  particular the cost of constructing an 80-byte `CacheOperation<String, String>`
  and matching on a 56-byte `CacheResponse<String>` per operation, versus a
  direct tier call, is unmeasured.
- **Impact:** Medium — a coverage gap on an abstraction whose overhead is
  precisely the thing a reviewer would want quantified before adopting it.
- **Remediation:** Add `crates/cachet_service/benches/adapter.rs` (Criterion)
  and `adapter_cg.rs` (Callgrind) comparing a direct `CacheTier` call against
  the same call through `ServiceAdapter` over a no-op `Service`, per
  `docs/naming.md`'s pairing convention. Callgrind matters more here than
  Criterion: the expected overhead is a few dozen instructions, well under
  wall-clock noise.
- **Evidence:** empirically verified (directory listing and `Cargo.toml` read).

#### F50. Two parallel implementations of the same conversion

- **Location:** `crates/cachet_service/src/adapter.rs:61-100` and
  `crates/cachet_service/src/ext.rs:24-59`
- **Issue:** `ServiceAdapter`'s `CacheTier` impl and `CacheServiceExt`'s blanket
  impl perform byte-for-byte the same work — build a `CacheOperation`, call
  `execute`, match the `CacheResponse`, map the mismatch to an error. Both are
  generic, so both monomorphise per `(K, V, S)` triple. A consumer that uses
  both paths (which is easy, since `CacheServiceExt` applies to *any* suitable
  `Service`, including the one inside a `ServiceAdapter`) gets two copies of
  the same code, doubling instruction-cache footprint for no functional benefit.
  It is also a maintenance hazard: F46 and F47 are the same bug duplicated,
  which is exactly what code duplication predicts.
- **Impact:** Low — code-size and i-cache pressure only, no per-operation cost.
- **Remediation:** Have `ServiceAdapter`'s `CacheTier` impl delegate to
  `CacheServiceExt` (`self.service.get(key).await` etc.), collapsing the two
  into one. Small, mechanical, behaviour-preserving.
- **Evidence:** inferred from code reading; the two impls were read side by
  side and differ only in the `Error::from_message` strings.
- **Confirming benchmark:** not a throughput finding; `cargo bloat` or an
  object-size comparison is the right instrument.

#### F51. `CacheOperation<K, V>` is sized by its largest variant

- **Location:** `crates/cachet_service/src/request.rs:14-24`
- **Issue:** The enum has four variants: `Get(GetRequest<K>)`,
  `Insert(InsertRequest<K, V>)`, `Invalidate(InvalidateRequest<K>)` and
  `Clear`. `InsertRequest<K, V>` holds both a key and a `CacheEntry<V>` (which
  itself carries 32 bytes of cold metadata, cachet_tier F42), so it is much the
  largest. Measured with layout-identical replicas:
  `CacheOperation<String, String> = 80` bytes, while a bare `GetRequest<String>`
  is 24. Every `get` therefore constructs and moves an 80-byte value to carry
  24 bytes of payload, and `Clear` moves 80 bytes to carry nothing.
- **Impact:** Low — 80 bytes is one cache line plus change and the move is a
  register-or-stack copy, not an allocation. Worth recording because it is on
  the per-operation path and because boxing the large variant
  (`Insert(Box<InsertRequest<K, V>>)`) is the standard remedy and would bring
  the enum to 32 bytes.
- **Impact qualifier:** boxing would move an allocation *onto* the insert path
  to take a copy off the get path — a trade that only pays if gets dominate
  inserts, which for a cache they do, but not by enough to act without
  measurement.
- **Remediation:** None recommended without the benchmark from F49.
- **Evidence:** **empirically verified** — sizes measured with a
  dependency-free `rustc -O` program containing layout-identical replicas
  (`CacheOperation<String, String> = 80`, `CacheResponse<String> = 56`,
  `CacheEntry<String> = 56`). The program was created in `/tmp`, run, and
  deleted.
- **Confirming benchmark:** the `adapter_cg.rs` proposed in F49.

### Benchmark coverage

**None.** `crates/cachet_service/` has no `benches/` directory and no
`[[bench]]` sections in `Cargo.toml` (F49).

Together with `cachet_tier` (also zero) and `bytesbuf_io` (also zero), three of
this group's six crates have no performance measurement whatsoever.

Minimum viable coverage:

1. `benches/adapter.rs` + `benches/adapter_cg.rs` — direct `CacheTier` call vs
   `ServiceAdapter` vs `CacheServiceExt`, over a no-op `Service`. Settles F49,
   F50 and F51, and quantifies the abstraction's cost.
2. An insert-heavy arm, so F46/F47 have a before/after number.
3. A middleware-stack arm (one or two `layered` layers) so the composition cost
   the crate exists to enable is visible.

### Considered and ruled out

* **`CacheServiceExt` as a blanket extension trait
  (`crates/cachet_service/src/ext.rs:24-32`).** Blanket impls on a trait bound
  are a standard, zero-cost Rust pattern; the dispatch is static. The
  duplication (F50) is the issue, not the mechanism.
* **RPITIT (`impl Future<Output = ...> + Send`) throughout `CacheServiceExt`
  (`ext.rs:15-21`).** No boxing — the right choice, and a pleasing contrast
  with `cachet_tier`'s `dyn(box)` (F40). Not a finding; worth citing as
  precedent when arguing F40.
* **`len` returning `Err(SizeError::unsupported())`
  (`adapter.rs:98-100`).** Correct and free: a remote service tier genuinely
  cannot report a size cheaply, and the trait's default is designed for this.
  Attempting to track it would be the performance mistake.
* **The `_ => Err(Error::from_message(...))` arms
  (`adapter.rs:70`, `:78`, `:86`, `:93`; `ext.rs:33`, `:41`, `:49`, `:56`).**
  These are unreachable in correct operation but are exactly the kind of
  defensive runtime check `docs/performance.md` says to preserve. The
  `from_message` allocation is on the error path only. Keep them.
* **Only two dependencies (`cachet_tier`, `layered`) and no feature flags
  (`Cargo.toml:31-33`).** Nothing to trim; no heavy default features; no
  optional-dependency mistakes. A model of a lean crate manifest.
* **`CacheResponse::is_hit` / `is_miss` / `into_entry`
  (`request.rs:90`, `:96`, `:102`).** Small, but in a generic `impl<V>`, so
  monomorphised into the consumer and inlinable without annotation — rule 2, not
  rule 1, as with cachet_tier F45.
* **Absence of `#[inline]` on `request.rs`'s six public constructors
  (`:35`, `:52`, `:67`, `:90`, `:96`, `:102`).** Same reasoning: all generic,
  all monomorphised. Not a rule-1 case.

---

## Appendix: findings index

51 findings — **9 High, 23 Medium, 19 Low**.

| # | Crate | Impact | Title |
|---|-------|--------|-------|
| F1 | bytesbuf | Medium | Global pool holds the pool mutex across multi-block buffer construction |
| F2 | bytesbuf | Medium | `BlockRef` reference-count `Clone`/`Drop` are not `#[inline]` |
| F3 | bytesbuf | Medium | `bytes_compat::view`'s `Buf` impl has no `#[inline]`, unlike its `BufMut` mirror |
| F4 | bytesbuf | Medium | `BytesBuf`'s small accessors are not `#[inline]` |
| F5 | bytesbuf | Medium | `BytesView`'s small accessors are not `#[inline]` |
| F6 | bytesbuf | Low | `BytesBuf::peek` builds a `SmallVec` without a capacity hint |
| F7 | bytesbuf | Low | `BytesView::range_checked` performs a two-pass span scan |
| F8 | bytesbuf | Low | `NeutralBlock` colocates the atomic refcount with payload bytes |
| F9 | bytesbuf | Low | No `#[inline]` on `lib.rs`'s public surface |
| F10 | bytesbuf | Low | `buf_put.rs` annotates less than half its public surface |
| F11 | bytesbuf | Low | Buffer `unsafe advance` relies on caller-upheld invariants |
| F12 | bytesbuf | Medium | Benchmarks cover neither `bytes_compat` conversion nor >8-span shapes |
| F13 | bytesbuf_io | High | `ReadAsFuturesStream::poll_next` heap-allocates a boxed future per stream item |
| F14 | bytesbuf_io | Low | `ReadAsFuturesStream::new` boxes the entire stream struct |
| F15 | bytesbuf_io | Medium | Conditional-read loop calls `BytesBuf::peek()` on every iteration |
| F16 | bytesbuf_io | Low | `ReadInspectDecision::Failed` boxes on a path shared with the hot decision |
| F17 | bytesbuf_io | Low | `Error::caused_by` is invoked on every fallible step |
| F18 | bytesbuf_io | Low | No `#[inline]` anywhere in the crate |
| F19 | cachet | High | `Cache::get` allocates an owned key on every lookup |
| F20 | cachet | High | The same `to_owned()` pattern repeats across eight more `Cache` methods |
| F21 | cachet | High | An unconditional global atomic increment on every cache operation |
| F22 | cachet | Medium | `WithRequestId::poll` writes a thread-local and constructs a drop guard on every poll |
| F23 | cachet | Medium | `record_*` reads the request-ID thread-local before checking whether anyone wants it |
| F24 | cachet | Medium | Two clock reads per tier per operation |
| F25 | cachet | Medium | `TimeToRefresh::try_start_refresh` clones the key before discovering it is already in flight |
| F26 | cachet | Medium | Refresh de-duplication uses one global `Mutex<HashSet<K>>` for all keys |
| F27 | cachet | Low | `do_refresh` clones the key twice more |
| F28 | cachet | Low | `FallbackCache::insert` clones the key when it could move it |
| F29 | cachet | Low | `FallbackCache::get_from_fallback` clones both key and value to promote |
| F30 | cachet | Medium | The main `cachet` benchmark requires a non-default feature that changes what it measures |
| F31 | cachet | High | `cachet`'s benchmarks measure `MockCache`, not `cachet` |
| F32 | cachet | Low | `encode` calls `BytesBuf::peek()` and clones the pool per encode |
| F33 | cachet | Medium | `cachet` has zero Callgrind benchmarks |
| F34 | cachet_memory | Medium | `InMemoryCache::insert` clones an owned key |
| F35 | cachet_memory | High | `CacheTier::get`'s `&K` signature forces `moka`'s owned-key lookup |
| F36 | cachet_memory | Low | The eviction listener runs a dynamic call per registered observer per eviction |
| F37 | cachet_memory | Low | `EntryExpiry` calls `CacheEntry::ttl()` on every create and update |
| F38 | cachet_memory | Medium | `cachet_memory` has no Callgrind benchmark and non-conforming group names |
| F39 | cachet_tier | High | `CacheTier::get` and `invalidate` take `&K`, not a borrowed form |
| F40 | cachet_tier | High | `dyn(box)` erasure boxes a future per tier per operation |
| F41 | cachet_tier | High | `MockCache` is used as the storage backend for `cachet`'s benchmarks |
| F42 | cachet_tier | Medium | `CacheEntry<V>` carries 32 bytes of cold metadata regardless of `V` |
| F43 | cachet_tier | Medium | `DynamicCache` implements `CacheTier`, so nesting it double-boxes |
| F44 | cachet_tier | Medium | `cachet_tier` has no benchmarks at all |
| F45 | cachet_tier | Low | `CacheEntry`'s ten small public accessors carry no `#[inline]` |
| F46 | cachet_service | Medium | `ServiceAdapter::insert` clones a key it already owns |
| F47 | cachet_service | Medium | `CacheServiceExt::insert` clones a key it already owns |
| F48 | cachet_service | Medium | The `get` and `invalidate` clones are forced by the owned-request design |
| F49 | cachet_service | Medium | `cachet_service` has no benchmarks at all |
| F50 | cachet_service | Low | Two parallel implementations of the same conversion |
| F51 | cachet_service | Low | `CacheOperation<K, V>` is sized by its largest variant |

### The five changes worth making first

Ordered by benefit divided by risk. All five are surgical, behaviour-preserving
and individually reviewable in minutes.

1. **F23** — move `current_request_id()` inside `emit_tier_event`'s
   `if let Some(handler)` guard (`crates/cachet/src/telemetry/cache.rs:129-140`).
   Removes a thread-local read per tier per operation, in every configuration,
   and shortens fifteen call sites.
2. **F21** — make `next_request_id()` conditional on telemetry being active
   (`crates/cachet/src/telemetry/cache.rs:27-29`). Removes a contended global
   atomic RMW from every cache operation; this is the finding most likely to
   change how the cache *scales*, not just how fast it is.
3. **F34, F46, F47, F28** — delete four redundant `.clone()` calls on
   already-owned keys (`crates/cachet_memory/src/tier.rs:202`,
   `crates/cachet_service/src/adapter.rs:76`,
   `crates/cachet_service/src/ext.rs:39`, `crates/cachet/src/fallback.rs:144`).
   One token each, provably identical behaviour, four allocations removed from
   write paths.
4. **F13** — hoist the boxed future out of `ReadAsFuturesStream::poll_next`
   (`crates/bytesbuf_io/src/read_futures.rs:75-117`), which also removes the
   `mem::transmute` at `:87-97`. One heap allocation per stream item, on a
   streaming-I/O path, plus a reduction in `unsafe` surface.
5. **F41** — stop using `MockCache` as benchmark storage
   (`crates/cachet_tier/src/testing.rs:224-277`). Not a runtime win, but nothing
   else on this list can be *measured* until it is done.

### The two decisions that need a maintainer, not a patch

Both are flagged **conflicting** against `docs/performance.md`: the house rules
say do not allocate on the hot path, and also say prefer surgical interventions
over architectural rewrites. Here those rules disagree.

- **F39 + F19/F20/F35** — `CacheTier::get(&K)` forces an owned-key allocation on
  every cache hit, defeating `Cache::get`'s own `Borrow` API, even though
  `moka` underneath already accepts a borrowed key. The fix requires a
  non-object-safe generic method, which requires F40 to be resolved first.
- **F40 + F43** — `dyn(box)` erasure (`crates/cachet_tier/src/tier.rs:38`)
  boxes a future per tier per operation, and nesting `DynamicCache` doubles it.
  The fix — making `cachet::Cache` generic over its tier — is source-breaking
  for every consumer.

Together these are, on the evidence available, the largest per-operation costs
in the caching stack. F44's proposed `cachet_tier` erasure benchmark is the
cheapest way to put a number on both before committing to either.
