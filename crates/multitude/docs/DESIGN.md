# Multitude Implementation Notes

This document describes the internal architecture of the `multitude`
crate. It complements the public-API rustdoc; for a user-level overview
see the crate-level docs.

## Table of contents

- [`Arena`](#arena)
- [`ChunkProvider`](#chunkprovider)
- [`Chunk`](#chunk)
- [Smart-pointer alignment and masking](#smart-pointer-alignment-and-masking)
- [`DropEntry`](#dropentry)

The crate is built from four collaborating pieces — `Arena`,
`ChunkProvider`, `Chunk`, and `DropEntry` — wired together by a single,
deliberately constrained chunk layout:

```text
ChunkProvider  ── Arc ──>  (cached Chunks)
      ^                              ^
      | StdArc                       | Weak (back-pointer)
      |                              |
    Arena  ─current────> ChunkMutator<A>  ──+1──> Chunk
           ─retired_local──> RetiredLocalChunks<A> (intrusive list of Chunks)
```

A `Chunk` backs every allocation style. Arena-lifetime references
(`&mut T`) and escape-capable smart pointers (`Arc`/`Box`) coexist in the
same chunk; the chunk distinguishes them only by *how* each is torn down
(deferred drop entries vs. eager per-handle refcount).

## `Arena`

`Arena<A>` is a thin façade over a `ChunkProvider` and one "current"
`ChunkMutator` slot, plus an intrusive list of retired chunks:

```rust
pub struct Arena<A: Allocator + Clone = Global> {
    current:               CurrentChunk<A>,
    local_shared_count:    Cell<u32>,            // smart-pointer handouts from `current`
    retired_local:         RetiredLocalChunks<A>,
    current_has_reference: Cell<bool>,           // did `current` hand out a `&mut T`?
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

- If the chunk handed out any arena-lifetime reference (`&mut T`,
  `&mut [T]`, `&mut str`, growable-collection buffer — tracked by
  `current_has_reference`) or still carries drop entries, it must stay
  pinned: such references carry no refcount of their own and their
  lifetime is bounded by `&self`, so dropping the chunk could replay
  drops on — or reclaim — memory still aliased by an outstanding `&mut T`.
  Retired chunks thread through an intrusive singly linked list (each
  chunk's `next` header field, no separate `Vec`) and keep their `+1`
  until `Arena::reset` / `Arena::drop`. This is the safety story that lets
  `try_reserve_local*` rebind a ticket's lifetime to `&Arena`.
- Otherwise (a smart-pointer-only chunk) it is dropped right away. Each
  `Arc`/`Box` keeps its hosting chunk alive via the atomic refcount, so
  such a chunk can **reclaim early** once its last handle drops, without
  waiting for reset.

A chunk that mixes references and smart pointers therefore stays pinned
until reset even after its `Arc`s drop — the deliberate cost of letting
a single current chunk serve both allocation styles.

**Smart-pointer handouts are atomic-free via a pre-credited surplus.**
Bumping the chunk's `AtomicUsize` refcount on every smart-pointer
allocation would be a hot-path atomic. Instead, at install time the arena
pre-credits the chunk's atomic `ref_count` with `LARGE_SHARED_REF_SURPLUS`
(2^30) and tracks per-allocation handouts in the non-atomic
`local_shared_count` (`Cell<u32>`); simple references never touch it. At
retire (`refill`, `Arena::reset`, or `Arena::drop`) the surplus is
reconciled with a single
`fetch_sub(LARGE_SHARED_REF_SURPLUS - local_shared_count)`, leaving the
chunk's atomic count equal to the number of escaped handles. The 2^30
surplus is large enough that concurrent `Arc::drop` on other threads
cannot underflow it. `Arc::clone` does not touch this count — each `Arc`
family takes exactly one chunk refcount at allocation and releases it when
its last clone drops (clones bump only the per-`Arc` strong count; see
*Per-`Arc` reference counting*).

**Reset replays reference drops eagerly.** `Arena::reset` (`&mut self`,
so no `&mut T` borrow can be live) reconciles the current chunk's surplus,
then replays every pending drop entry on the current and retired chunks
*before* releasing their `+1`s — so reference destructors run at reset
time even when escaped `Arc`/`Box` handles keep a chunk's refcount above
zero. Smart-pointer values allocated before the reset are **not** dropped
by it; they remain owned by their handles. After reset the next allocation
installs a fresh chunk.

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
    drop_entry_count: AtomicU16,         // reference drops pending replay (capped by capacity)
    #[cfg(feature = "stats")]
    wasted_at_retire: AtomicU32,
    data: [UnsafeCell<u8>],
}
```

The chunk holds a `Weak<ChunkProvider>` because an escaped `Arc`/`Box`
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

**Mixed-chunk pinning.** A chunk that serves *both* a reference and a
smart pointer is pinned until reset: it cannot reclaim early once its
`Arc`s drop, because the `&mut T` borrow has no refcount and its
destructor must run at reset. The `current_has_reference` flag preserves
early reclamation for smart-pointer-only chunks, so only genuinely mixed
chunks pay this cost. The chunk always reserves a drop-entry tail;
smart-pointer allocations simply never write one.

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
(`CHUNK_BASE_MASK = !(CHUNK_ALIGN - 1)`) and casts the result to
`Chunk<A>`. There is no runtime type tag in the header — `Box::drop` and
`Arc::drop` both recover a `*const Chunk` the same way.

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

## `DropEntry`

`DropEntry` records the deferred destructor work for **arena-lifetime
references only** — `Arena::alloc -> &mut T` and `&mut [T]`, which have
no `Drop` of their own and whose backing chunk runs the destructor at
reset / teardown. **Neither `Box` nor `Arc` registers a drop entry**:
`Box::drop` runs `drop_in_place` eagerly on the (re-fattened) value
pointer, and `Arc::drop` does the same on the last strong reference (see
*Per-`Arc` reference counting* above). A chunk's `drop_entry_count` stays
`0` for as long as it only hosts smart pointers.

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

`len` is a `u16`; slice references whose `needs_drop` count exceeds
`u16::MAX` are rejected up front by their `alloc_*` orchestrator so the
placeholder never overflows. (The `Arc<[T]>` family has **no** such cap,
since it drops via `drop_in_place::<[T]>` rather than a counted entry.)

**Two-phase write.** Allocation paths reserve a *placeholder* (null
`drop_fn`, real `value_offset`/`len`) up front. After the value is
fully initialized, the caller commits the real shim with
`drop_fn.store(real_shim, Release)`. The replay loop loads with
`Acquire`; null entries are skipped — they belong to allocations whose
initialization closure panicked or whose `Uninit` ticket was dropped
without `init`. Storing as `AtomicPtr<()>` (not `AtomicUsize`)
preserves function-pointer provenance under Miri's strict provenance.

**Replay.** At `Arena::reset` / `Arena::drop`, each chunk's pending drop
entries are replayed **eagerly** on the owning thread — walking the
drop-entry stack **newest-first** (LIFO, matching Rust drop order) and
invoking `(drop_fn)(data + value_offset, len)` on each committed entry —
*before* the chunk's own `+1` is released, so reference destructors run
even when escaped `Arc`/`Box` handles keep the chunk's refcount above
zero. The count is then cleared so a later refcount-zero teardown does
not run them again. A chunk with no drop entries skips this step
entirely. A panic in any shim is contained; replay continues so remaining
destructors still run.

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

