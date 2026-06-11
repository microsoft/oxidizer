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
      |                              |
      Arc                          Weak
      |                              |
    Arena  ─current_local──> ChunkMutator<LocalChunk<A>>  ──+1──> LocalChunk
           ─current_shared─> ChunkMutator<SharedChunk<A>> ──+1──> SharedChunk
           ─retired_local──> Vec<ChunkMutator<LocalChunk<A>>>
```

## `Arena`

`Arena<A>` is a thin façade over a `ChunkProvider` and two "current"
`ChunkMutator` slots, plus a vector of retired local mutators:

```rust
pub struct Arena<A: Allocator + Clone = Global> {
    current_local:  CurrentChunk<LocalChunk<A>>,
    current_shared: CurrentChunk<SharedChunk<A>>,
    retired_local:  RefCell<Vec<ChunkMutator<LocalChunk<A>>>>,
    next_local_class:  Cell<u8>,
    next_shared_class: Cell<u8>,
    provider: StdArc<ChunkProvider<A>>,
}
```

`Arena` is `Send` but `!Sync` (both `CurrentChunk` and `RefCell` are
`!Sync`). Cross-thread *sharing* is done by allocating `Arc`-family
smart pointers and cloning them across threads.

**Local refills retire the displaced mutator.** Simple references
(`Arena::alloc -> &mut T`, `alloc_str`, `alloc_slice_copy`) carry no
refcount of their own; their lifetime is bounded by `&self`. When the
current local chunk fills, the arena cannot drop the displaced mutator
— doing so might let the chunk reach refcount zero and replay drops on
memory still aliased by an outstanding `&mut T`. Instead, `refill_local`
pushes the displaced mutator onto `retired_local`. Each retained
mutator keeps its +1 alive until `Arena::reset` or `Arena::drop`. This
is the safety story that lets `try_reserve_local*` rebind a ticket's
lifetime to `&Arena`.

**Shared refills release immediately.** Shared chunks produce only
`Arc`-family smart pointers — each `Arc` keeps its hosting chunk alive
via the atomic refcount. `refill_shared` drops the displaced mutator
right away; no `retired_shared` vector is needed.

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
`*_cache_class` *floor*. The link lives in the **first bytes of the
cached chunk's payload** — chunks on a free list have refcount zero,
so the payload is reusable. When the floor advances, any below-floor
chunks still on the list are walked and destroyed in one pass.

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
    allocator: A,
    provider:  Weak<ChunkProvider<A>>,
    capacity:  usize,
    ref_count:        Cell<u8>,   // only ever 0 or 1
    drop_entry_count: Cell<u16>,  // capped by chunk capacity
    _padding: [u8; 4],            // explicit; reserve for future fields
    data: [UnsafeCell<u8>],       // length = capacity
}

#[repr(C)]
pub(crate) struct SharedChunk<A: Allocator + Clone> {
    allocator: A,
    provider:  Weak<ChunkProvider<A>>,
    capacity:  usize,
    ref_count:        AtomicUsize,
    drop_entry_count: AtomicU16,
    _padding: [u8; 6],
    data: [UnsafeCell<u8>],
}
```

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

**Provider weak-ref.** When a chunk's refcount hits zero it
`upgrade()`s its `Weak<ChunkProvider>` to return itself to the cache
(or free its backing if the arena is gone). One atomic op on the
chunk-drop cold path; never on the allocation hot path.

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
there's no prefix overhead.

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

## `DropEntry`

`DropEntry` records the deferred destructor work for values whose
`Drop` cannot be run by the smart pointer itself — i.e. arena
references, `Arc<T>`, and DST `Box` (sized `Box<T>` runs `T::drop`
eagerly via `drop_in_place` and so needs no entry).

Each such allocation reserves **both** `size_of::<T>()` at the front
of the free region *and* one `DropEntry` slot at the back. The
effective remaining capacity is `drop_top - bump`; overflow is
detected when those two meet. Allocations of `T: !Drop` skip the
reservation entirely.

```rust
#[repr(C)]
struct DropEntry {
    drop_fn:      AtomicPtr<()>,  // null = uncommitted placeholder
    value_offset: u16,            // offset into the chunk payload
    len:          u16,            // element count (1 for a single value;
                                  // DST metadata, e.g. slice length, for slices)
}
```

`len` is a `u16`; slice/DST allocations whose `needs_drop` count
exceeds `u16::MAX` are rejected up front by their `alloc_*` orchestrator
so the placeholder never overflows.

**Two-phase write.** Allocation paths reserve a *placeholder* (null
`drop_fn`, real `value_offset`/`len`) up front. After the value is
fully initialized, the caller commits the real shim with
`drop_fn.store(real_shim, Release)`. The replay loop loads with
`Acquire`; null entries are skipped — they belong to allocations whose
initialization closure panicked or whose `Uninit` ticket was dropped
without `init`. Storing as `AtomicPtr<()>` (not `AtomicUsize`)
preserves function-pointer provenance under Miri's strict provenance.

The commit is idempotent: concurrent `Arc::<MaybeUninit<T>>::assume_init`
on cloned handles all install the same `T`-determined shim.

**Replay.** When the chunk's last refcount drops, the chunk walks its
drop-entry stack **newest-first** (LIFO, matching Rust drop order) and
invokes `(drop_fn)(data + value_offset, len)` on each committed
entry. A panic in any shim is contained; replay continues so remaining
destructors still run.

**Closure-panic safety.** The smart-pointer construction paths take a
protective `ChunkRef` (`+1` guard) before invoking the user closure.
On unwinding, the `ChunkRef`'s `Drop` releases the +1; on success the
caller calls `ChunkRef::forget` to transfer the +1 into the
freshly-constructed smart pointer. Combined with the two-phase
placeholder, a panicking closure leaves no `T::drop` queued on
uninitialized memory and no refcount leaked.

**Refcount overflow.** Both `inc_ref` paths check against the
wraparound boundary and abort (`std::process::abort` or a forced
double-panic under `no_std`) if exceeded. The abort helper is
`#[cold] #[inline(never)]` so the hot-path call site stays small.
This mirrors `std::sync::Arc`: a wraparound would race live pointers
with a free, and the only sound response is to terminate.
