# Multitude Implementation Notes

This document describes the internal architecture of the `multitude`
crate. It complements the public-API rustdoc; for a user-level overview
see the crate-level docs.

## Table of contents

- [`Arena`](#arena)
- [`ChunkProvider`](#chunkprovider)
- [`LocalChunk` and `SharedChunk`](#localchunk-and-sharedchunk)
- [Smart-pointer alignment and masking](#smart-pointer-alignment-and-masking)
- [`DropEntry`](#dropentry)

The crate is built from five collaborating pieces — `Arena`,
`ChunkProvider`, `LocalChunk`, `SharedChunk`, and `DropEntry` — wired
together by a single, deliberately constrained chunk layout:

```text
ChunkProvider  ── Arc ──>  (cached LocalChunks / SharedChunks)
      ^                              ^
      | StdArc                       | Weak (SharedChunk) / *const (LocalChunk)
      |                              |
    Arena  ─current_local──> ChunkMutator<LocalChunk<A>>  ──+1──> LocalChunk
           ─current_shared─> ChunkMutator<SharedChunk<A>> ──+1──> SharedChunk
           ─retired_local──> RetiredLocalChunks<A> (intrusive list of LocalChunks)
```

## `Arena`

`Arena<A>` is a thin façade over a `ChunkProvider` and two "current"
`ChunkMutator` slots, plus an intrusive list of retired local chunks:

```rust
pub struct Arena<A: Allocator + Clone = Global> {
    current_local:      CurrentChunk<LocalChunk<A>>,
    current_shared:     CurrentChunk<SharedChunk<A>>,
    local_shared_count: Cell<u32>,            // handouts from current_shared
    retired_local:      RetiredLocalChunks<A>,
    next_local_class:   Cell<SizeClass>,
    next_shared_class:  Cell<SizeClass>,
    provider:           StdArc<ChunkProvider<A>>,
    #[cfg(feature = "stats")]
    relocations:        Cell<u64>,
}
```

`Arena` is `Send` but `!Sync` (`CurrentChunk` and the `Cell` /
`RetiredLocalChunks` fields are all `!Sync`). Cross-thread *sharing* is
done by allocating `Arc`-family smart pointers and cloning them across
threads.

**Local refills retire the displaced chunk.** Simple references
(`Arena::alloc -> &mut T`, `alloc_str`, `alloc_slice_copy`) carry no
refcount of their own; their lifetime is bounded by `&self`. When the
current local chunk fills, the arena cannot drop the displaced mutator
— doing so might let the chunk reach refcount zero and replay drops on
memory still aliased by an outstanding `&mut T`. Instead, `refill_local`
retires the displaced chunk onto `retired_local`, an intrusive singly
linked list threaded through each chunk's `next` header field (no
separate `Vec` allocation). Each retired chunk keeps its +1 alive until
`Arena::reset` or `Arena::drop`. This is the safety story that lets
`try_reserve_local*` rebind a ticket's lifetime to `&Arena`.

**Shared refills release immediately.** Shared chunks produce only
`Arc`-family smart pointers — each `Arc` keeps its hosting chunk alive
via the atomic refcount. `refill_shared` drops the displaced mutator
right away; no `retired_shared` list is needed.

**Shared handouts are atomic-free via a pre-credited surplus.** Bumping
the shared chunk's `AtomicUsize` refcount on every allocation would be a
hot-path atomic. Instead, at install time the arena pre-credits the
chunk's atomic `ref_count` with `LARGE_SHARED_REF_SURPLUS` (2^30) and
tracks per-allocation handouts in the non-atomic `local_shared_count`
(`Cell<u32>`). At retire (refill / reset / arena drop) the surplus is
reconciled with a single
`fetch_sub(LARGE_SHARED_REF_SURPLUS - local_shared_count)`, leaving the
chunk's atomic count equal to the number of escaped handles. The 2^30
surplus is large enough that concurrent `Arc::drop` on other threads
cannot underflow it. `Arc::clone` no longer touches this count at all —
each `Arc` family takes exactly one chunk refcount at allocation and
releases it when its last clone drops (clones bump only the per-`Arc`
strong count; see *Per-`Arc` reference counting*).

**Size-class ratchet.** Each successful refill bumps the matching
`next_*_class` toward the largest cacheable class (`NUM_CHUNK_CLASSES
- 1 = 7`). This hint flows into `acquire_*`, preventing a pathological
"always smallest class" pattern. `ArenaBuilder::with_capacity_*` seeds
the ratchet so a warm-up preallocation is consumed by the first refill.

**Oversized allocations bypass refill.** Requests above the chunk size
classes flow through `alloc_oversized_*`, which allocates a one-shot
chunk sized exactly to the request, fills it via a stack-local
mutator, and never installs it as the active chunk — so subsequent
small allocations keep landing in the original active chunk.

## `ChunkProvider`

`ChunkProvider` is the factory and cache for chunks. Each `Arena` owns
exactly one (strong `Arc`); chunks hold back-references via `Weak`. The
provider is not shared between arenas.

Cacheable chunks come in eight power-of-two **total allocation sizes**:
`MIN_CHUNK_BYTES = 512 B` up to `MAX_CHUNK_BYTES = 64 KiB`. The
builder-configurable `max_normal_alloc` (default 16 KiB) is a
*chunk-acquisition threshold* on user-payload bytes — requests strictly
above it bypass the cache and get a one-shot oversized chunk sized
exactly to the request.

Each cache is a **single intrusive Treiber-style freelist** (one head,
regardless of size class) plus a monotonic non-decreasing
`*_cache_class` *floor*. The link lives in the cached chunk's `next`
**header field** (`Cell<*mut u8>` for local, `AtomicPtr<u8>` for shared)
— the same slot a local chunk uses for the retired list, reused here
since the two phases are mutually exclusive in time. When the floor
advances, any below-floor chunks still on the list are walked and
destroyed in one pass.

- **Local cache** is touched only by the arena's owning thread,
  enforced structurally because `LocalChunk: !Send`. The head lives in
  an `OwnerThreadCell` so the provider stays `Sync` without an actual
  lock.
- **Shared cache** is multi-producer / single-consumer: pushes happen
  from any thread that drops the last `Arc` on a shared chunk; pops
  happen only from the arena's owning thread (`Arena: !Sync`
  structurally enforces this). MPSC eliminates Treiber's classic
  hazards — no other popper can free the head between our load and CAS
  (no UAF), and the head's identity cannot recycle behind our back (no
  ABA).

A `byte_budget` knob (default `usize::MAX`) caps total outstanding
chunk bytes via a CAS loop on `bytes_outstanding`.

## `LocalChunk` and `SharedChunk`

There are two independent chunk types, each a DST with an
`[UnsafeCell<u8>]` payload tail. Neither carries a bump cursor — that
lives in the `ChunkMutator` that currently owns it.

```rust
#[repr(C)]
pub(crate) struct LocalChunk<A: Allocator + Clone> {
    provider: *const ChunkProvider<A>,   // non-owning raw back-pointer
    capacity: usize,
    next: Cell<*mut u8>,                 // intrusive link: retired list OR cache freelist
    ref_count:        Cell<u8>,          // only ever 0 or 1
    drop_entry_count: Cell<u16>,         // capped by chunk capacity
    #[cfg(feature = "stats")]
    wasted_at_retire: Cell<u32>,         // stats-only wasted-tail accounting
    data: [UnsafeCell<u8>],              // length = capacity
}

#[repr(C)]
pub(crate) struct SharedChunk<A: Allocator + Clone> {
    allocator: A,
    provider:  Weak<ChunkProvider<A>>,
    capacity:  usize,
    ref_count: AtomicUsize,
    next:      AtomicPtr<u8>,            // intrusive cache-freelist link
    drop_entry_count: AtomicU16,        // vestigial: shared chunks never register drop entries
    #[cfg(feature = "stats")]
    wasted_at_retire: AtomicU32,
    data: [UnsafeCell<u8>],
}
```

The two chunk types deliberately differ in how they reach their
provider: `LocalChunk` holds a non-owning raw `*const ChunkProvider`
(the provider strictly outlives every local teardown, so no `Weak`
refcount or orphan branch is needed), while `SharedChunk` holds a
`Weak` because an escaped `Arc` can outlive the arena. `LocalChunk`
carries no `allocator` field — the provider supplies the allocator at
teardown time.

The payload is `[UnsafeCell<u8>]` (not `[u8]`) for two reasons:

- **Interior mutability for shared borrows** — a `&Chunk` must allow
  concurrent payload writes through derived `ChunkMutator` handles.
- **Pointer provenance under Stacked / Tree Borrows.** The chunk is
  passed as a fat `NonNull<Chunk<A>>` (the slice tail metadata carries
  `capacity`); reading the payload via `&raw mut (*chunk).data` keeps
  the derived pointer's provenance spanning the whole payload. A
  sized-header thin pointer would have provenance for only the header.

Keeping the two chunk types independent lets each own its
thread-safety story (non-atomic `Cell` vs. atomic) without trait-level
genericity at the smart-pointer surface.

**Provider back-reference.** When a chunk's refcount hits zero it
returns itself to the cache (or frees its backing if the arena is
gone). A `SharedChunk` `upgrade()`s its `Weak<ChunkProvider>` to do so —
one atomic op on the chunk-drop cold path, never on the allocation hot
path — and frees itself directly if the upgrade fails. A `LocalChunk`
dereferences its non-owning raw `*const ChunkProvider` instead: the
provider is guaranteed live for every local teardown, so no `Weak`
upgrade is involved.

## Smart-pointer alignment and masking

The crate's compact smart-pointer representation is built on a single
geometric invariant: **every chunk allocation is 64 KiB-aligned**
(`CHUNK_ALIGN = 65 536`). The alignment is enforced at allocation time
via `Layout::from_size_align(total, struct_align())`, not via
`repr(align(…))` on the struct itself — keeping the struct's
structural alignment small means `size_of_val(&*fat_ptr)` matches the
actual allocation even for small classes.

Given that invariant, every user-facing smart pointer (`Arc<T>`,
`Box<T>` for any `T` including DSTs, and the bespoke UTF-16 variants)
is a **single 8-byte raw pointer** into the chunk's `data` tail. DST
metadata (slice length, vtable) lives unaligned in the chunk prefix
immediately preceding the value payload, read with
`core::ptr::read_unaligned`. For `T: Sized` the metadata is `()` so
there's no prefix overhead. `Arc<T>` additionally stores its
per-`Arc` strong count (an `AtomicU32`) in the prefix, before the
metadata (see *Per-`Arc` reference counting*); `Box` has no such
prefix.

To recover the owning chunk's header from a smart-pointer value, each
smart-pointer type **masks the low bits to the 64 KiB boundary**
(`CHUNK_BASE_MASK = !(CHUNK_ALIGN - 1)`) and casts the result to its
statically-known chunk type. There is no runtime flavor discriminator
in the header — `Box::drop` always recovers a `*const SharedChunk` and
so does `Arc::drop` (both smart-pointer families are backed by shared
chunks).

Two consequences of the masking scheme:

- **Maximum smart-pointer alignment** is `CHUNK_ALIGN / 2 = 32 KiB`.
  `try_alloc_*` returns `AllocError` for higher requests; `alloc_*`
  panics.

- **Oversized chunks** are still 64 KiB-aligned and hold exactly one
  allocation placed at the start of the payload; the value pointer
  lies within the chunk's first 64 KiB tile, so the same mask recovers
  the header.

- **End-of-chunk ZST guard.** Even ZSTs must not return a pointer at
  `chunk_base + CHUNK_ALIGN`, or the mask would walk to the *next*
  chunk. `ChunkMutator::try_alloc` therefore advances the bump cursor
  by `size.max(1)` for every reservation, fast-failing through the
  refill path if a ZST would otherwise land at the one-past-end
  boundary.

## Per-`Arc` reference counting

Each `Arc<T>` carries **its own** strong reference count — an
`AtomicU32` stored in the chunk payload immediately *before* the value
(and before the DST metadata, if any). The layout of an `Arc` value is:

```text
[strong (AtomicU32, at reservation base)][pad][T::Metadata (unaligned)][T payload]
                                                                        ^ value pointer
```

The reservation is aligned to `max(align_of::<T>(), 4)` so the leading
strong slot is 4-byte aligned; the value pointer is `align_of::<T>()`
aligned and the metadata sits immediately before it (recovered with
`read_unaligned`, exactly as for `Box`). The strong count is recovered
from the value pointer by subtracting a fixed prefix
(`thin_dst::strong_prefix_bytes_for`) and is accessed only as an
`AtomicU32` — never through a reference that spans the (possibly
uninitialized) payload, which keeps the scheme sound under Miri.

The accounting is **Option Y**:

- **Allocation** writes `strong = 1` and takes **one** refcount on the
  hosting chunk for the whole `Arc` family (via the pre-credited
  surplus, as for any shared allocation).
- **`Arc::clone`** bumps only the per-`Arc` `strong` with a single
  `Relaxed` increment — it does **not** touch the chunk refcount.
- **`Arc::drop`** does a `Release` decrement of `strong`; on the
  `strong → 0` transition it runs an `Acquire` fence, drops the value
  in place (`drop_in_place::<T>`, which natively handles `?Sized`),
  and releases the family's single chunk refcount (adopted *before*
  the value drop, so a panicking destructor still releases the chunk).

Because the value's destructor runs eagerly on the last `Arc` (rather
than being deferred to chunk teardown), nested arena `Arc`s — e.g.
`Arc<[Arc<T>]>` whose inner and outer handles share a chunk — release
their storage promptly instead of forming a self-pinning cycle.

`Arc::<MaybeUninit<T>>::assume_init` is a pure reinterpret: `MaybeUninit<T>`
and `T` share size, alignment, and metadata, so the strong-prefix layout
is identical and the strong count is untouched.

## `DropEntry`

`DropEntry` records the deferred destructor work for **local arena
references only** — `Arena::alloc -> &mut T` and `&mut [T]`, which have
no `Drop` of their own and whose backing chunk runs the destructor at
teardown. **Neither `Box` nor `Arc` registers a drop entry, and shared
chunks never carry one**: `Box::drop` runs `drop_in_place` eagerly on
the (re-fattened) value pointer, and `Arc::drop` does the same on the
last strong reference (see *Per-`Arc` reference counting* above). Drop
entries therefore live exclusively on `LocalChunk`s.

Each such reference allocation reserves **both** `size_of::<T>()` at the
front of the free region *and* one `DropEntry` slot at the back. The
effective remaining capacity is `drop_top - bump`; overflow is detected
when those two meet. Allocations of `T: !Drop` skip the reservation
entirely.

```rust
#[repr(C)]
struct DropEntry {
    drop_fn:      AtomicPtr<()>,  // null = uncommitted placeholder
    value_offset: u16,            // offset into the chunk payload
    len:          u16,            // element count (1 for a single value;
                                  // DST metadata, e.g. slice length, for slices)
    _pad: [u8; PAD_BYTES],        // keep successive back-stack entries aligned
}
```

`len` is a `u16`; local slice references whose `needs_drop` count
exceeds `u16::MAX` are rejected up front by their `alloc_*` orchestrator
so the placeholder never overflows. (The `Arc<[T]>` family has **no**
such cap, since it drops via `drop_in_place::<[T]>` rather than a
counted entry.)

**Two-phase write.** Allocation paths reserve a *placeholder* (null
`drop_fn`, real `value_offset`/`len`) up front. After the value is
fully initialized, the caller commits the real shim with
`drop_fn.store(real_shim, Release)`. The replay loop loads with
`Acquire`; null entries are skipped — they belong to allocations whose
initialization closure panicked or whose `Uninit` ticket was dropped
without `init`. Storing as `AtomicPtr<()>` (not `AtomicUsize`)
preserves function-pointer provenance under Miri's strict provenance.

**Replay.** When a `LocalChunk`'s refcount drops to zero (at
`Arena::reset` / `Arena::drop`), the chunk walks its drop-entry stack
**newest-first** (LIFO, matching Rust drop order) and invokes
`(drop_fn)(data + value_offset, len)` on each committed entry. Shared
chunks skip this step entirely. A panic in any shim is contained;
replay continues so remaining destructors still run.

**Closure-panic safety.** The smart-pointer construction paths take a
protective `ChunkRef` (`+1` guard) before invoking the user closure.
On unwinding, the `ChunkRef`'s `Drop` releases the +1; on success the
caller calls `ChunkRef::forget` to transfer the +1 into the
freshly-constructed smart pointer. Combined with the two-phase
placeholder (for local references) and eager `drop_in_place` (for
`Box`/`Arc`), a panicking closure leaves no `T::drop` queued on
uninitialized memory and no refcount leaked.

**Refcount overflow.** Both the chunk `inc_ref` paths and `Arc::clone`'s
per-`Arc` `strong` increment check against the wraparound boundary and
abort (`std::process::abort` or a forced double-panic under `no_std`) if
exceeded. The abort helper is `#[cold] #[inline(never)]` so the hot-path
call site stays small. This mirrors `std::sync::Arc`: a wraparound would
race live pointers with a free, and the only sound response is to
terminate.

