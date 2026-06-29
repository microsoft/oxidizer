# Multitude Implementation Notes

This document describes the internal architecture of the `multitude`
crate. It complements the public-API rustdoc; for a user-level overview
see the crate-level docs.

## Table of contents

- [`Arena`](#arena)
- [`ChunkProvider`](#chunkprovider)
- [`Chunk`](#chunk)
- [Smart-pointer alignment and masking](#smart-pointer-alignment-and-masking)
- [Per-`Arc` reference counting](#per-arc-reference-counting)
  - [`Rc`: the non-atomic sibling](#rc-the-non-atomic-sibling)
- [Zero-copy freeze of growable buffers](#zero-copy-freeze-of-growable-buffers)
- [`Alloc`](#alloc)
- [Closure-panic safety](#closure-panic-safety)

The crate is built from three collaborating pieces — `Arena`,
`ChunkProvider`, and `Chunk` — wired together by a single,
deliberately constrained chunk layout:

```text
ChunkProvider  ── Arc ──>  (cached Chunks)
      ^                              ^
      | StdArc                       | Weak (back-pointer)
      |                              |
    Arena  ─current────> ChunkMutator<A>  ──+1──> Chunk
           ─retired_local──> RetiredLocalChunks<A> (intrusive list of Chunks)
```

A `Chunk` backs every allocation style. Arena-lifetime allocations
(`Alloc<T>` handles) and escape-capable smart pointers (`Arc`/`Rc`/`Box`)
coexist in the same chunk. All of them run their value's destructor
eagerly when the handle drops; the chunk distinguishes them only by
whether the handle takes a per-handle refcount (`Arc`/`Rc`/`Box`, which can
escape and reclaim early) or relies on the arena keeping the chunk pinned
(`Alloc`, which is lifetime-bound to `&Arena`).

## `Arena`

`Arena<A>` is a thin façade over a `ChunkProvider` and one "current"
`ChunkMutator` slot, plus an intrusive list of retired chunks:

```rust
pub struct Arena<A: Allocator + Clone = Global> {
    current:               CurrentChunk<A>,
    local_shared_count:    Cell<u32>,            // smart-pointer handouts from `current`
    retired_local:         RetiredLocalChunks<A>,
    current_has_reference: Cell<bool>,           // did `current` hand out an `Alloc` (arena-lifetime) handle?
    next_class:            Cell<SizeClass>,
    provider:              StdArc<ChunkProvider<A>>,
    #[cfg(feature = "stats")]
    relocations:           Cell<u64>,
}
```

`Arena` is `Send` but `!Sync` (`CurrentChunk` and the `Cell` /
`RetiredLocalChunks` fields are all `!Sync`). Cross-thread *sharing* is
done by allocating `Arc`-family smart pointers and cloning them across
threads.

**Refill rotates the current chunk; pinning is decided per chunk.** When
the current chunk fills, `refill` reconciles its surplus (see below) and
then *either* retires it onto `retired_local` *or* drops it immediately:

- If the chunk handed out any arena-lifetime allocation (`Alloc<T>`,
  including the `str`/`[T]`/`MaybeUninit` forms and growable-collection
  buffers — tracked by `current_has_reference`), it must stay pinned: such
  handles carry no refcount of their own and their lifetime is bounded by
  `&self`, so dropping the chunk could reclaim memory still aliased by an
  outstanding `Alloc`. Retired chunks thread through an intrusive singly
  linked list (each chunk's `next` header field, no separate `Vec`) and keep
  their `+1` until `Arena::reset` / `Arena::drop`. This is the safety story
  that lets `try_reserve_local*` rebind a ticket's lifetime to `&Arena`.
- Otherwise (a smart-pointer-only chunk) it is dropped right away. Each
  `Arc`/`Rc`/`Box` keeps its hosting chunk alive via the atomic refcount, so
  such a chunk can **reclaim early** once its last handle drops, without
  waiting for reset.

A chunk that mixes `Alloc` handles and smart pointers therefore stays pinned
until reset even after its `Arc`s drop — the deliberate cost of letting
a single current chunk serve both allocation styles.

**Smart-pointer handouts are atomic-free via a pre-credited surplus.**
Bumping the chunk's `AtomicUsize` refcount on every smart-pointer
allocation would be a hot-path atomic. Instead, at install time the arena
pre-credits the chunk's atomic `ref_count` with `LARGE_SHARED_REF_SURPLUS`
(2^30) and tracks per-allocation handouts in the non-atomic
`local_shared_count` (`Cell<u32>`); `Alloc` handouts never touch it. At
retire (`refill`, `Arena::reset`, or `Arena::drop`) the surplus is
reconciled with a single
`fetch_sub(LARGE_SHARED_REF_SURPLUS - local_shared_count)`, leaving the
chunk's atomic count equal to the number of escaped handles. The 2^30
surplus is large enough that concurrent `Arc::drop` on other threads
cannot underflow it. Neither `Arc::clone` nor `Rc::clone` touches this
count — each `Arc`/`Rc` family takes exactly one chunk refcount at
allocation and releases it when its last clone drops (clones bump only the
per-handle strong count; see *Per-`Arc` reference counting*).

**Reset is a pure cursor rewind.** `Arena::reset` (`&mut self`, so no
`Alloc` handle — which borrows `&self` — can be live) reconciles the
current chunk's surplus and returns the current and retired chunks' bytes
to the cache (or leaves chunks alive if escaped `Arc`/`Rc`/`Box` handles still
hold them). It runs **no** destructors: every `Alloc` handle already ran
its value's destructor eagerly when it was dropped (which must have
happened before `reset` could be called). Smart-pointer values allocated
before the reset are likewise not dropped by it; they remain owned by their
handles. After reset the next allocation installs a fresh chunk.

**Size-class ratchet.** Each successful refill bumps `next_class` toward
the largest cacheable class (`NUM_CHUNK_CLASSES - 1 = 7`). This hint flows
into `acquire`, preventing a pathological "always smallest class" pattern.
`ArenaBuilder::with_capacity_*` seeds the ratchet so a warm-up
preallocation is consumed by the first refill.

**Oversized allocations bypass refill.** Requests above the chunk size
classes flow through `alloc_oversized_*`, which allocates a one-shot
chunk sized exactly to the request, fills it via a stack-local
mutator, and never installs it as the active chunk — so subsequent
small allocations keep landing in the original active chunk. An oversized
reference chunk is pinned on `retired_local`; an oversized smart-pointer
chunk is kept alive by its handle and reclaims when the handle drops.


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

The cache is a **single intrusive Treiber-style freelist** (one head,
regardless of size class) plus a monotonic non-decreasing `cache_class`
*floor*. The link lives in the cached chunk's `next` **header field**
(`AtomicPtr<u8>`) — the same slot a pinned chunk uses for the retired
list, reused here since the two phases are mutually exclusive in time.
When the floor advances, any below-floor chunks still on the list are
walked and destroyed in one pass.

The cache is multi-producer / single-consumer: pushes happen from any
thread that drops the last handle on a chunk; pops happen only from the
arena's owning thread (`Arena: !Sync` structurally enforces this). MPSC
eliminates Treiber's classic hazards — no other popper can free the head
between our load and CAS (no UAF), and the head's identity cannot recycle
behind our back (no ABA).

A `byte_budget` knob (default `usize::MAX`) caps total outstanding
chunk bytes via a CAS loop on `bytes_outstanding`.

## `Chunk`

A `Chunk` is a DST with an `[UnsafeCell<u8>]` payload tail. It does not
carry a bump cursor — that lives in the `ChunkMutator` that currently
owns it.

```rust
#[repr(C)]
pub(crate) struct Chunk<A: Allocator + Clone> {
    allocator:        A,
    provider:         Weak<ChunkProvider<A>>,
    capacity:         usize,
    ref_count:        AtomicUsize,
    next:             AtomicPtr<u8>,     // intrusive link: retired list OR cache freelist
    #[cfg(feature = "stats")]
    wasted_at_retire: AtomicU32,
    data: [UnsafeCell<u8>],
}
```

The chunk holds a `Weak<ChunkProvider>` because an escaped `Arc`/`Rc`/`Box`
can outlive the arena, and its own `allocator` clone so it can free its
backing memory itself if the arena (and thus the provider) is gone.

The payload is `[UnsafeCell<u8>]` (not `[u8]`) for two reasons:

- **Interior mutability for shared borrows** — a `&Chunk` must allow
  concurrent payload writes through derived `ChunkMutator` handles.
- **Pointer provenance under Stacked / Tree Borrows.** The chunk is
  passed as a fat `NonNull<Chunk<A>>` (the slice tail metadata carries
  `capacity`); reading the payload via `&raw mut (*chunk).data` keeps
  the derived pointer's provenance spanning the whole payload. A
  sized-header thin pointer would have provenance for only the header.

**Mixed-chunk pinning.** A chunk that serves *both* an `Alloc` handle and a
smart pointer is pinned until reset: it cannot reclaim early once its
`Arc`s drop, because the `Alloc`'s borrow has no refcount and is bounded by
the `&Arena` lifetime, so the chunk must stay alive until reset. The
`current_has_reference` flag preserves early reclamation for
smart-pointer-only chunks, so only genuinely mixed chunks pay this cost.

**Provider back-reference.** When a chunk's refcount hits zero it returns
itself to the cache (or frees its backing if the arena is gone). It
`upgrade()`s its `Weak<ChunkProvider>` to do so — one atomic op on the
chunk-drop cold path, never on the allocation hot path — and frees itself
directly if the upgrade fails.


## Smart-pointer alignment and masking

The crate's compact smart-pointer representation is built on a single
geometric invariant: **every chunk allocation is 64 KiB-aligned**
(`CHUNK_ALIGN = 65 536`). The alignment is enforced at allocation time
via `Layout::from_size_align(total, struct_align())`, not via
`repr(align(…))` on the struct itself — keeping the struct's
structural alignment small means `size_of_val(&*fat_ptr)` matches the
actual allocation even for small classes.

Given that invariant, every user-facing smart pointer (`Arc<T>`,
`Rc<T>`, `Box<T>` for any `T` including DSTs, and the bespoke UTF-16
variants) is a **single 8-byte raw pointer** into the chunk's `data` tail. DST
metadata (slice length, vtable) lives unaligned in the chunk prefix
immediately preceding the value payload, read with
`core::ptr::read_unaligned`. For `T: Sized` the metadata is `()` so
there's no prefix overhead. `Arc<T>` and `Rc<T>` additionally store a
per-handle strong count in the prefix, before the metadata — an `AtomicU32`
for `Arc`, a plain unaligned `u32` for `Rc` (see *Per-`Arc` reference
counting*); `Box` has no such prefix.

To recover the owning chunk's header from a smart-pointer value, each
smart-pointer type **masks the low bits to the 64 KiB boundary**
(`CHUNK_BASE_MASK = !(CHUNK_ALIGN - 1)`) and casts the result to
`Chunk<A>`. There is no runtime type tag in the header — `Box::drop`,
`Arc::drop`, and `Rc::drop` all recover a `*const Chunk` the same way.

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

The accounting works as follows:

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

### `Rc`: the non-atomic sibling

`Rc<T>` reuses **everything** above — the thin pointer, header masking,
metadata prefix, the family's single (atomic) chunk refcount, and the eager
last-drop teardown — with exactly two differences, captured by a `Strong`
policy (`thin_dst::{AtomicStrong, LocalStrong}`) that parameterizes the shared
reservation code:

1. **Non-atomic count.** `Rc`'s per-handle strong count is a plain `u32`
   read/written with `read_unaligned` / `write_unaligned` (never as a `&u32`).
   `clone` is `count += 1` and `drop` is `count -= 1` with no atomic op and no
   fence — sound because `Rc` is `!Send`/`!Sync`, so the count never crosses
   threads. The chunk refcount stays atomic (touched only at alloc and last
   drop), so an `Rc` can still outlive the arena and free its chunk.
2. **Unaligned, no alignment floor.** Because the count is non-atomic it needs
   no natural alignment, so `LocalStrong::block_align` is just `align_of::<T>()`
   (vs. `max(_, 4)` for `Arc`). For sub-4-aligned payloads (`str`, `[u8]`) this
   drops the 4-byte reservation floor, packing a few bytes tighter.

`Rc` places no `Send`/`Sync` bound on `T`, so it can own thread-affine values
that `Arc` cannot. Freezing a `Vec`/`String` into an `Rc` reuses the same
`Arc`-layout freeze prefix (the `AtomicU32` `1` reads back as the `u32` `1` the
`Rc` expects), so only direct `alloc_rc*` calls get the tighter packing.

## Zero-copy freeze of growable buffers

`Vec<T>` / `String` / `Utf16String` can **freeze** into an arena-owned
`Box<[T]>` / `Arc<[T]>` with no allocation and no element copy. To make
this possible, every growable buffer of a freezable `T` reserves the full
`Arc<[T]>` freeze prefix in front of its payload at allocation time:

```text
[strong (AtomicU32, at reservation base)][pad][len (usize, unaligned)][payload …]
```

This is exactly the `Arc<[T]>` layout (a superset of `Box<[T]>`'s, which
reads only the length metadata at `payload − size_of::<usize>()`). The
strong count is written `1` at reservation; the length slot is left
uninitialized and filled at freeze. The buffer takes **no** chunk
refcount while it is a live `Vec` — it pins its chunk through the same
reference machinery as `&mut [T]` (`current_has_reference`) — so the hot
push/grow paths are unchanged.

Freezing then:

1. recovers the hosting chunk from the payload pointer by the 64 KiB mask;
2. takes the family's `+1` — drawn from the pre-credited surplus when the
   buffer still lives in the current chunk, or a plain atomic increment
   when its chunk has already been retired (and its surplus reconciled);
3. writes the final length into the reserved metadata slot; and
4. `mem::forget`s the `Vec` (ownership of the elements and storage passes
   to the smart pointer, which runs `T::drop` itself).

A buffer that has **no** prefix — a ZST or over-aligned `T`, or a
zero-copy `split_off` tail whose base points mid-chunk — falls back to the
original O(*n*) draining freeze (move the elements into a fresh
allocation). `ArenaBuf` carries a one-bit `freeze_prefix` flag recording
which case applies.

## `Alloc`

`Alloc<'a, T>` is the handle returned by every arena-lifetime allocation
(`Arena::alloc -> Alloc<'a, T>`, and the `str` / `[T]` / `MaybeUninit`
forms). It is a thin wrapper around the exclusive `&'a mut T` borrow of the
slot the bump allocator just carved out:

```rust
pub struct Alloc<'a, T: ?Sized> {
    inner: &'a mut T,
}
```

It derefs to `T` (so it is used exactly like the bare reference it
replaced) and runs `T`'s destructor **eagerly** in its own `Drop` via
`ptr::drop_in_place(self.inner)`. The `&'a` field binds it to the arena
borrow, so:

- the arena (and the chunk pinning machinery, `current_has_reference` /
  `retired_local`) keeps the backing storage alive for `'a`, with **no**
  per-allocation refcount; and
- because `Arena::reset` / `Arena::drop` take `&mut self`, no `Alloc` can be
  live across them — every handle has already run its destructor by then.

This is why **there is no deferred-drop machinery**: a chunk never has to
remember which slots still need a destructor run at reset. Each `Alloc`
finalizes its own value when it drops, `reset` is a pure cursor rewind, and
`Box`/`Arc`/`Rc` likewise drop eagerly through their refcount (see *Per-`Arc`
reference counting*). `Alloc::leak(handle)` recovers the bare `&'a mut T`
without running the destructor, for the rare caller that wants a leaked
reference.

`Alloc` only runs the destructor; it does **not** return the slot to the
bump cursor (a bump allocator can only rewind its cursor, not free interior
slots), so the memory is reclaimed in bulk at `reset` / arena drop like any
other arena allocation. `mem::forget`ing an `Alloc` simply skips the
destructor — sound, but the value is never finalized.

## Closure-panic safety

The smart-pointer construction paths take a protective `ChunkRef`
(`+1` guard) before invoking the user closure. On unwinding, the
`ChunkRef`'s `Drop` releases the +1; on success the caller calls
`ChunkRef::forget` to transfer the +1 into the freshly-constructed smart
pointer. For arena-lifetime allocations, a closure that panics before the
value is initialized simply leaves the reserved slot uninitialized and no
`Alloc` is constructed (so no `drop_in_place` runs on it); slice init guards
drop any already-initialized prefix on panic. So a panicking closure leaves
no `T::drop` queued on uninitialized memory and no refcount leaked.

**Refcount overflow.** Both the chunk `inc_ref` paths and `Arc::clone`'s
per-`Arc` `strong` increment check against the wraparound boundary and
abort (`std::process::abort` or a forced double-panic under `no_std`) if
exceeded. The abort helper is `#[cold] #[inline(never)]` so the hot-path
call site stays small. This mirrors `std::sync::Arc`: a wraparound would
race live pointers with a free, and the only sound response is to
terminate.

