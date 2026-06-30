# Plurality — Architecture & Design

This document describes how the crate is built. For user-facing API docs see the
crate-level rustdoc (`src/lib.rs`); for unimplemented ideas see
[`TODO.md`](./TODO.md).

## Goal

A growable, fixed-slot object pool that hands out **single-pointer-wide** smart
pointers which deref to `&T`. Unlike a bump arena, individual values can be
freed and their slots reused; unlike `slab`/`slotmap`, callers get real smart
pointers (not indices/keys) that can be shared (`Arc`/`Rc`) and, for the owned
flavors, outlive the pool itself. The store never moves a value once allocated,
so the pointers stay valid until dropped.

## Module layout

| File | Responsibility |
|---|---|
| `lib.rs` | Crate root, feature gates, re-exports, crate-level docs. |
| `builder.rs` | `PoolBuilder<T, A>` — chunk size, max chunks, allocator; validates and builds `PoolInner`. |
| `pool.rs` | `Pool<T, A>` + `PoolInner<T, A>`: allocation methods, the MPSC free list, single-thread growth, the chunk directory, refcount/teardown. |
| `chunk.rs` | `ChunkHeader<T, A>`, chunk `Layout` math, slot addressing (`slot_at`) and pointer recovery (`header_of`). |
| `slot.rs` | `SlotCell<T>`, the dual refcount/free-link protocol, refcount-overflow abort. |
| `boxed.rs` | `Box<T>` — unique owner, `Send`, may outlive the pool. |
| `alloced.rs` | `Alloc<'pool, T>` — unique owner that borrows the pool; cheapest handle. |
| `sync.rs` | `Arc<T>` — shared, atomic refcount, `Send + Sync`. |
| `rc.rs` | `Rc<T>` — shared, non-atomic refcount, `!Send`. |
| `common.rs` | Macros emitting the shared handle surface (`as_ptr`, `into_pin`, `Unpin`, `AsRef`/`Borrow`, `Debug`/`Display`, `PartialEq`/`Eq`/`Ord`/`Hash`, `Pointer`, and the mutable `as_mut_ptr`/`AsMut`/`BorrowMut`). |
| `atomic.rs` | Atomic type shim: real `core::sync::atomic` normally, `loom` atomics under `--cfg loom`. |

## Concurrency model

The pool is **single-producer / multi-consumer**:

- `Pool<T, A>` is `Send + !Sync`. Allocation takes `&Pool`, and because `&Pool`
  cannot be shared across threads there is exactly **one allocator thread** at a
  time (the whole `Pool` can be *moved* between threads, but only one thread
  ever holds it).
- The handles it produces are `Send`/`Sync` (the owned/atomic ones), so **frees
  race**: many threads may drop `Box`/`Arc` concurrently.

Hence one thread pushes new chunks and pops free slots (uncontended), while many
threads push freed slots back. The free-list head, the pool refcount, and the
per-slot refcounts are atomic to make frees safe; the chunk **directory** is
touched only by the single allocator thread and needs no synchronization. There
is no `Mutex` anywhere — only `core::sync::atomic`.

## Core data structures

### `PoolInner<T, A>` (`pool.rs`)

The shared, refcounted state behind a `Pool`. It outlives the `Pool` handle when
smart pointers are still alive.

- `free_head: AtomicU32` — head of the embedded global free list (`FREE_END` =
  empty / must grow). Single consumer pops, many producers push.
- `pool_refcount: AtomicUsize` — `1` for the live `Pool` handle plus `1` per
  live *refcounted* allocation (`Box`/`Arc`/`Rc`). `Alloc` handles do **not**
  contribute; their borrow already keeps the inner alive.
- `chunk_size: u32`, `shift: u32` (`log2(chunk_size)`), `mask: u32`
  (`chunk_size - 1`) — chunk size is a power of two so index math is shift/mask.
- `max_chunks: Option<u32>` — optional cap; `None` is unbounded.
- `chunks_allocated: AtomicU32` — count so far (atomic because frees read it for
  introspection).
- `chunk_layout: Layout` — fixed layout of one chunk.
- `directory: UnsafeCell<Vec<NonNull<ChunkHeader<T, A>>>>` — `chunk_index ->`
  chunk base. Written only on the allocator thread; read there on `pop` and
  (once quiescent) at teardown. `!Sync` is the soundness gate.
- `allocator: A` — used for chunk allocations (via `allocator-api2`).

`Pool<T, A>` itself is just `{ inner: NonNull<PoolInner<T, A>> }`.

### `ChunkHeader<T, A>` (`chunk.rs`)

Each chunk is one allocation: a `#[repr(C)]` header followed (after alignment
padding) by its `[SlotCell<T>; N]` payload.

```text
chunk allocation:  [ ChunkHeader | pad | SlotCell 0 | SlotCell 1 | … | SlotCell N-1 ]
                   ^ base                ^ slots_offset bytes in
```

The header stores `pool: NonNull<PoolInner<T, A>>` (raw back-pointer; chunks
live until pool teardown so there is no cycle), `base_index` (this chunk's first
global index = `chunk_index * chunk_size`), and `chunk_index`.

`slots_offset::<T, A>()` is `size_of::<ChunkHeader>()` rounded up to
`align_of::<SlotCell<T>>()` — independent of `N`, so slot addressing and
recovery are pure arithmetic with no per-pointer masking.

### `SlotCell<T>` (`slot.rs`)

```rust
#[repr(C)]
struct SlotCell<T> {
    value: UnsafeCell<MaybeUninit<T>>,
    refcount: AtomicU32, // dual-role (see below)
    index: u32,          // immutable in-chunk index (0..N)
}
```

`refcount` is **contextual**, not tagged:

- **Occupied:** a refcount `>= 1` (number of `Arc`/`Rc` sharing the slot; `Box`
  keeps it at 1 implicitly).
- **Free:** the next-free *global* index, or `FREE_END` (`u32::MAX`) for
  end-of-chain.

This is safe because free-list traversal only follows links from slots already
on the chain (always read as a link), and clone/drop only touch the field on
slots whose handles are live (always read as a count). The two value ranges
overlap numerically; the protocol disambiguates by context.

`index` is the **in-chunk** index, written once at chunk init. It is what makes
single-pointer recovery possible (below), and it yields the slot's global index
as `header.base_index + index`.

Bounds: `FREE_END = u32::MAX` is reserved, so the highest valid slot index is
`u32::MAX - 1`. Refcounts are capped at `MAX_REFCOUNT = i32::MAX`, mirroring
`std::sync::Arc`; overflow aborts the process via a double-panic Bomb (works in
`no_std`).

### Centralized value access

Reaching the `T` inside `UnsafeCell<MaybeUninit<T>>` is the one genuinely tricky
pointer dance in the crate. Rather than re-derive it in every handle, it lives in
four audited `unsafe` primitives on `SlotCell<T>` — `value_ref`, `value_mut`,
`write_value`, `drop_value` — each with a single documented safety contract.
Every handle and the pool's `occupy` path call these instead of writing raw
`(*(*slot).value.get())…` chains, and the shared "drop the value then return the
slot" sequence is a single `drop_and_free` / `drop_and_free_local` helper in
`pool.rs`. The remaining per-handle `unsafe` is irreducible and minimal: the
refcount RMW (atomic for `Arc`, non-atomic for `Rc`) and the slot-pointer deref.

## Single-pointer handles and pointer recovery

A handle is exactly one pointer — `NonNull<SlotCell<T>>` — plus, for `Alloc`, a
zero-sized lifetime marker. Everything else (the chunk and the owning
`PoolInner`) is recovered from the slot pointer by arithmetic in
`header_of` (`chunk.rs`):

1. Read the slot's stored in-chunk `index`.
2. Step back `index` slots to reach slot 0.
3. Step back `slots_offset` bytes to reach the `ChunkHeader`.
4. The header gives `pool` (the `PoolInner` back-pointer) and `base_index`.

`index` must be the **in-chunk** index, not the global one: recovery has to
locate the header *before* it can read anything stored there, so it can only
rely on data already in the slot. The global index — needed when pushing onto
the free list — is derived *after* recovery as `base_index + index`.

Per-operation cost:

- `Deref` → reads `value` only; no recovery.
- `Arc::clone` → atomic `refcount.fetch_add`; `Rc::clone` → non-atomic `+= 1`.
- `Drop` → decrement refcount; on reaching 0, recover the header to reach
  `free_head` (push the derived global index) and, for `Box`/`Arc`/`Rc`,
  decrement `pool_refcount`. `Alloc` drop pushes the slot but does **not** touch
  `pool_refcount`.

### The four handle flavors

| Handle | Outlives pool? | `Send`/`Sync` | Per-slot refcount | `pool_refcount` traffic |
|---|---|---|---|---|
| `Box<T>` | yes | `Send` (if `T: Send`) | none (unique) | bump on alloc, drop on free |
| `Alloc<'pool, T>` | no (borrows pool) | `!Send` | none (unique) | **none** — borrow proves the pool outlives it |
| `Arc<T>` | yes | `Send + Sync` (if `T: Send + Sync`) | atomic | bump/drop per slot |
| `Rc<T>` | yes | `!Send + !Sync` | non-atomic | bump/drop per slot |

- **`Alloc<'pool, T>`** is a `Box` that borrows the pool. Because the borrow
  statically guarantees the pool outlives every `Alloc`, it skips the
  `pool_refcount` atomic RMW on both alloc and free — the cheapest handle. The
  price: `!Send`, and it cannot be stored `'static`.
- **`Rc<T>`** is `Arc` with non-atomic refcounting. The `refcount` field stays
  `AtomicU32` (it is the cross-thread free-list link while free), but `Rc`
  touches it via `AtomicU32::as_ptr()` with plain `u32` add/sub. This is sound
  because `Rc` is `!Send + !Sync` (single-threaded), an *occupied* slot is never
  on the free list (so no other thread accesses its field atomically), and a
  slot is uniformly `Rc`- or `Arc`-managed. Under `--cfg loom` (where `as_ptr`
  is unavailable) the inc/dec/read fall back to relaxed atomics. Validated with
  Miri's data-race checker.

`Box`/`Alloc` are the unique owner of their slot while alive, so they implement
`Deref + DerefMut`. `Arc`/`Rc` are read-only (`Deref` only); interior mutability
goes inside `T`.

## Index ↔ pointer mapping

A global index `g` maps to its slot on the allocator thread:

```text
chunk_no = g >> shift     // shift = log2(chunk_size)
offset   = g & mask       // mask  = chunk_size - 1
slot     = slot_at(directory[chunk_no], offset)
```

Cheap because `chunk_size` is a power of two. `chunk_size()` exposes the
effective (rounded-up) value. This lookup only ever happens on the allocator
thread; the free/push path reaches `PoolInner` by recovery from the slot, never
through the directory.

## Free list and growth

The free list is an embedded MPSC Treiber stack threaded through free slots'
`refcount` fields. There is **no growth lock**: chunk allocation, directory
append, and the free-list splice all run on the single allocator thread, racing
only the producers' pushes on `free_head`.

**Pop** (allocator thread, single consumer):
1. `g = free_head.load()`. If `g == FREE_END`, grow.
2. Map `g -> slot`; `next = slot.refcount.load()` (read as link).
3. `compare_exchange_weak(free_head, g -> next)`; retry on contention. ABA-free
   because only this thread pops and a free slot is never re-pushed while free.

**Grow** (allocator thread, rare, `#[cold]`): `grow()` returns
`Option<NonNull<SlotCell<T>>>` — the reserved first slot of the new chunk, or
`None` if the pool cannot grow (cap reached, the `u32` ceiling hit, or the
allocator failed). It:
1. Checks `chunks_allocated` against the cap (`max_chunks`, or `FREE_END /
   chunk_size` for unbounded pools).
2. Allocates one chunk via `A`; on failure returns `None`.
3. Initializes the header and every slot (each slot links to the next; slot 0 is
   reserved for the caller and the last slot's link is overwritten by the
   splice, so a uniform `base_index + i + 1` init suffices).
4. Appends the chunk to the directory and bumps `chunks_allocated`.
5. Splices slots `1..N-1` onto `free_head` and returns slot 0.

Returning the reserved slot directly (instead of looping back to `pop`) keeps
the "grow then allocate" path bounded — there is no spin where a lost race could
re-empty the free list.

**Push** (deallocation, many producers):
1. Recover the global index `g = base_index + index`.
2. `h = free_head.load()`; `slot.refcount.store(h)` (now a link);
   `compare_exchange_weak(free_head, h -> g)`; retry on contention.

## Refcount semantics and teardown

Two independent refcounts:

- **Per-slot `refcount`** governs the *value*: how many `Arc`/`Rc` share it.
- **`pool_refcount`** governs the *pool memory* (all chunks + the directory +
  `PoolInner`), so handles can outlive the `Pool` handle.

Transitions:
- `build` → `pool_refcount = 1` (the `Pool` handle).
- Allocate a refcounted handle → set slot `refcount = 1`, bump `pool_refcount`
  (`Alloc` skips the bump).
- `Arc`/`Rc` clone → bump slot `refcount` only.
- Drop a handle → `refcount -= 1`; if it hit 0, `drop_in_place(value)`, push the
  slot, then `pool_refcount -= 1` (`Alloc` pushes but skips the decrement).
- Drop the `Pool` handle → `pool_refcount -= 1`.
- When `pool_refcount` hits 0: free every chunk and the directory, then
  `PoolInner`.

Because every live refcounted allocation holds one `pool_refcount`, by the time
it reaches 0 there are **no occupied slots left** — teardown never runs
`T::drop`. Every value is dropped exactly once, on its own handle's drop. So
`Pool::drop` is not synchronous w.r.t. outstanding handles: the backing memory
lives until the last handle drops.

Teardown can run on whichever thread drops the last handle (or the `Pool`),
which may not be the allocator thread. That is sound: `pool_refcount == 0`
implies the `Pool` handle is gone (no more allocation/growth) and no handles
remain, so the directory is quiescent. The `Acquire` on the final
`pool_refcount` decrement establishes happens-before with every prior handle
drop. The directory itself is published separately: `grow()` does
`chunks_allocated.store(Release)` right after each `directory.push`, and
teardown performs a matching `chunks_allocated.load(Acquire)` before walking
the directory. (`pool_refcount` increments are `Relaxed`, so they do not
publish directory growth on their own — `chunks_allocated` is the publish
point.) The teardown thread therefore sees a complete, frozen directory to
walk.

## Allocation API

Each owned/shared flavor (`box`, `alloc`, `arc`, `rc`) offers a triplet plus a
fallible `try_*` sibling:

- `alloc_*(value)` — convenience; RVO usually elides the stack copy.
- `alloc_*_with(f)` — RVO-friendly; the closure body is the construction site.
- `alloc_uninit_*` → `assume_init` — the only stable way to *guarantee* zero
  stack round-trip (mirrors `Box::new_uninit` / `Arc::new_uninit`).

`try_alloc_*` returns `Result<_, AllocError>`, where `AllocError` is the crate's
own error type. On failure the rejected value is dropped and the `_with` closure
is not called — matching the convention of `Box::try_new`. The panicking
`alloc_*` variants panic on a full pool. A pool is "full" for one of two
reasons, which `AllocError` distinguishes via `is_capacity_exhausted()` and
`is_allocator_failure()`: the chunk cap (or the `u32` ceiling) is reached and no
slot is free, or a growth allocation failed. The panic message names the same
cause.

## `no_std`

`alloc` only — no `std`, no `Mutex`. Allocation/growth are single-threaded and
the free list is a lock-free MPSC stack, so only `core::sync::atomic` is needed.
Chunk allocations go through `allocator-api2` so custom allocators compose.

## Verification strategy

The crate is checked by several complementary tools (see `tests/` and
`benches/`):

- **Unit + integration tests** (`tests/integration.rs`, `tests/coverage.rs`) —
  full API, panics, custom/failing/counting allocators, contention stress.
- **Miri** — UB / data-race / leak checking across the integration suite,
  including the non-atomic `Rc` path.
- **Loom** (`tests/loom.rs`, `src/atomic.rs`) — exhaustive interleavings of the
  concurrent free path (two `Arc`s on one slot, cross-thread frees, teardown on
  a worker thread, drop-exactly-once).
- **Bolero** (`tests/bolero.rs`) — property/fuzz coverage.
- **Coverage** — line coverage via `cargo +nightly llvm-cov`; genuinely
  unreachable paths (overflow abort, the `splice_chain` helper) are marked
  `#[cfg_attr(coverage_nightly, coverage(off))]`.
- **Mutation testing** — `cargo mutants --all-features`; equivalent/unreachable
  mutants are skipped inline with `#[cfg_attr(test, mutants::skip)]` next to the
  code they apply to.
- **Benchmarks** (`benches/`) — `gungraun_alloc` (Callgrind, instruction-exact)
  and `criterion_alloc` (wall-clock) run the *same* per-op bodies from
  `benches/shared/ops.rs` for every allocation function; `pool_comparison` is a
  cross-crate comparison; `graph_churn` is a 1M-node macro-benchmark vs mimalloc.
  `scripts/perf_report.rs` runs them and regenerates [`PERF.md`](./PERF.md).
