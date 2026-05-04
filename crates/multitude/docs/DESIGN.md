# Multitude Implementation Notes

This document describes the internal architecture of the `multitude`
crate: how chunks are allocated and recycled, how the
`ChunkProvider`, `Arena`, and smart pointers compose, and the
invariants that keep the `unsafe` blocks sound. It complements the
public-API rustdoc; for a high-level overview suitable for users,
see the crate-level docs.

It is a *design document*, not an implementation plan: it captures
the invariants and shapes the code commits to today, not phased
TODOs. Specific source locations are cited where they help a
future maintainer; line numbers are not.

## Table of contents

- [Goals](#goals)
- [Containing `unsafe`](#containing-unsafe)
  - [Internal abstractions that absorb the unsafety](#internal-abstractions-that-absorb-the-unsafety)
  - [Invariants that justify each `unsafe`](#invariants-that-justify-each-unsafe)
- [Hot-path performance](#hot-path-performance)
  - [Bump allocation](#bump-allocation-arenaalloc-alloc_rc-alloc_arc-)
  - [Single-branch fit check](#single-branch-fit-check)
  - [Value vs. closure fast paths](#value-vs-closure-fast-paths)
  - [Panicking vs fallible inner methods](#panicking-vs-fallible-inner-methods)
  - [Smart-pointer clone](#smart-pointer-clone)
  - [Smart-pointer drop](#smart-pointer-drop)
  - [What's deliberately kept off the hot path](#whats-deliberately-kept-off-the-hot-path)
  - [Memory layout for cache-friendliness](#memory-layout-for-cache-friendliness)
  - [Lazy pinning](#lazy-pinning-no-overhead-for-smart-pointer-only-arenas)
  - [Reset](#reset)
- [Top-level pieces](#top-level-pieces)
- [`LocalChunk` and `SharedChunk`](#localchunk-and-sharedchunk)
  - [Single-pointer smart pointers (64 KiB chunk alignment)](#single-pointer-smart-pointers-64-kib-chunk-alignment)
  - [Why DSTs?](#why-dsts)
- [`ChunkProvider`](#chunkprovider)
  - [Backing allocator: generic, stored by value](#backing-allocator-generic-stored-by-value)
  - [`Send` / `Sync` story](#send--sync-story)
  - [Size classes, high-water mark, and the `max_normal_alloc` knob](#size-classes-high-water-mark-and-the-max_normal_alloc-knob)
  - [API sketch](#api-sketch)
  - [Cache representation](#cache-representation)
  - [Cache eviction policy](#cache-eviction-policy)
- [`Arena`](#arena)
  - [Stub state: no "is there a current chunk?" branch on the hot path](#stub-state-no-is-there-a-current-chunk-branch-on-the-hot-path)
  - [Reentrancy in closure-based paths](#reentrancy-in-closure-based-paths)
  - [Provider ownership: `Arc` on the arena, `Weak` on the chunk](#provider-ownership-arc-on-the-arena-weak-on-the-chunk)
- [`Drop` support](#drop-support)
  - [Trailing drop list (simple refs, `Rc`, `Arc`, DST `Box`)](#trailing-drop-list-simple-refs-rc-arc-dst-box)
  - [Sized `Box`: drop-in-place via the smart pointer](#sized-box-drop-in-place-via-the-smart-pointer)
  - [Refcount overflow: abort, not `debug_assert`](#refcount-overflow-abort-not-debug_assert)
- [Builder collections: `Vec`, `String`, `Utf16String`](#builder-collections-vec-string-utf16string)
  - [Common shape](#common-shape)
  - [Initial buffer allocation](#initial-buffer-allocation)
  - [Growing](#growing)
  - [Drop semantics during build](#drop-semantics-during-build)
  - [Freezing](#freezing)
  - [Why builders are `Local`-flavor](#why-builders-are-local-flavor)
- [Validation: loom and miri](#validation-loom-and-miri)
- [Implementation notes](#implementation-notes)

## Goals

- Cleanly separate the *bump-allocation primitive* (a chunk) from the
  *high-level allocator* (the `Arena`) and from *chunk lifecycle
  management* (the `ChunkProvider`).

- Support two refcounting flavors (single-threaded `Local`, atomic
  `Shared`) without forcing all chunks through a single union/enum
  representation.

- Keep small allocations cheap by reusing chunks of well-known sizes
  via a per-provider cache, while still allowing the arena to satisfy
  arbitrarily large allocations on demand.

- **Make the steady-state hot path blazingly fast.** A long-lived
  arena that keeps allocating and freeing is the central use case;
  bump allocation, smart-pointer clone, and smart-pointer drop must
  all be near-minimal in instructions and atomics. Slow paths
  (chunk overflow, chunk return, drop-list replay, cache eviction)
  must stay out of the inlined hot path.

- **Minimize and tightly encapsulate `unsafe`.** The vast majority
  of the implementation is safe Rust. `unsafe` is reserved for a
  small set of well-defined operations whose invariants are
  documented and verified locally:

  - taking raw pointers into a chunk's `data` payload (and the
    inverse: recovering the chunk header from a payload pointer
    via the 64 KiB-alignment mask),

  - reading or writing a `T` through such a pointer at a known
    offset,

  - the type-erased `drop_in_place::<T>` shims used by the
    drop-list machinery,

  - the DST allocation primitives the `dst` feature exposes.

  Everything else — refcount manipulation (modulo overflow
  aborts), list bookkeeping, cache management, sentinel
  construction, builder growth policies, freezing — is built out
  of safe primitives on `LocalChunk`/`SharedChunk`/`ChunkProvider`/
  `Arena`.

## Containing `unsafe`

The crate is fundamentally about pointer arithmetic into raw chunk
memory, so `unsafe` will appear — but it is concentrated into a
small number of **internal abstractions** so the rest of the code
stays safe.

The places where `unsafe` is fundamentally needed:

1. **Backing-allocator calls** — `Allocator::allocate` and
   `Allocator::deallocate` on the user-supplied allocator `A` are
   safe in the trait, but the pointer manipulation around them is not.

2. **DST construction** for `LocalChunk` / `SharedChunk` — taking a
   freshly-allocated header-plus-tail buffer and producing a
   `*mut LocalChunk<A>` (or `*mut SharedChunk<A>`) by way of
   `slice_from_raw_parts_mut(ptr, payload_len) as *mut _`.

3. **The 64 KiB mask** that recovers a chunk header from a
   payload pointer (`internal/mask.rs`).

4. **Bumping the cursor and writing a `T` into a chunk's payload at
   a known offset** — raw write through a typed pointer in
   `try_alloc_inner_with` / `try_alloc_inner_value` and their
   slice/string/DST analogues.

5. **`drop_in_place` shims** — the type-erased
   `unsafe fn(*mut u8, usize)` callbacks invoked by drop-list
   replay (`internal/drop_list.rs`).

6. **The shared-cache Treiber CAS loop and the `LocalSlot` cell
   access** in `internal/chunk_provider.rs` — `unsafe` only at
   the point where we read `(*chunk).next` of a chunk we have
   ownership claim on (just after a successful CAS on the
   shared-cache head, or via `with_mut` on the local-cache
   `LocalSlot`).

### Internal abstractions that absorb the unsafety

| Internal type                                                        | What it encapsulates                                                                                                                                                                                                                    |
|----------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `LocalChunk` / `SharedChunk` (in `internal/{local,shared}_chunk.rs`) | Chunk DST plus its header; safe `inc_ref`, `dec_ref`, `replay_drops`, `reconcile_swap_out`, etc.                                                                                                                                        |
| `mask::{local_chunk_of, shared_chunk_of}`                            | The 64 KiB-mask trick. One `unsafe fn` per flavor with a documented contract (the input pointer must come from an arena allocation).                                                                                                    |
| `DropEntry` (in `internal/drop_list.rs`)                             | The trailing drop-list record; safe constructor; `unsafe` only on shim invocation during replay.                                                                                                                                        |
| `LocalSlot<T>` (in `internal/local_slot.rs`)                         | `UnsafeCell<T>` + `unsafe impl Sync` so the provider's local-cache field can live in a `Sync` struct without an actual lock. The single-thread invariant is enforced structurally because every caller is on the arena's owning thread. |
| `AtomicPtr` shared-cache head (in `internal/chunk_provider.rs`)      | The shared-cache list head, a thin pointer to the top `SharedChunk` manipulated by a multi-producer / single-consumer Treiber CAS. The single-consumer property eliminates the multi-consumer Treiber UAF/ABA hazards.                  |

### Invariants that justify each `unsafe`

Each `unsafe` block in the codebase cites one of the following
invariants in its `// SAFETY:` comment, with `debug_assert!`s where
practical:

- **Chunk-header invariant.** Every value pointer returned by
  `Arena::alloc_*` lives within the first 64 KiB of a 64 KiB-aligned
  chunk allocation, so `addr & !(CHUNK_ALIGN - 1)` is the start of
  a valid chunk header. (Enforced by `Arena::alloc_*` rejecting
  alignment `>= MAX_SMART_PTR_ALIGN = 32 KiB` and by oversized
  chunks placing their single allocation within their first 64 KiB
  tile.)

- **Refcount-positive invariant.** Any code path that accesses a
  chunk's interior holds a +1 either via a smart-pointer's mask
  recovery, the arena's `current_*` slot, or the protective hold
  installed during reentrancy-prone allocation paths.

- **Payload-extent invariant.** Every offset has been validated by
  a bump check (`try_bump_fit`) against the chunk's
  `[data_ptr, drop_back)` free region, so the byte range
  `[aligned_addr, aligned_addr + size)` is inside the chunk's
  payload with correct alignment.

- **Single-thread-local invariant.** `LocalSlot::with_mut` is only
  callable from the arena's owning thread, enforced structurally
  because the only callers are arena methods on a `!Send` type, and
  because `LocalChunk: !Send` makes it impossible for another
  thread to produce one to release.

- **Drop-shim invariant.** A `DropEntry`'s `drop_fn` is monomorphic
  in its target `T`, generated at allocation time, so the
  `(value_offset, len)` pair it's called with always corresponds to
  a valid `T` (or `[T]` of `len` elements) at that offset. DST drop
  shims additionally encode the pointee metadata in the entry's
  `len` slot.

## Hot-path performance

The design is shaped around these operations being as cheap as
physically possible:

### Bump allocation (`Arena::alloc`, `alloc_rc`, `alloc_arc`, …)

Steady-state, the inlined hot path is:

```text
1. read  data_ptr     (1 Cell load — chunk's bump cursor as a NonNull<u8>)
2. read  drop_back    (1 Cell load — limit pointer)
3. align data_ptr addr up to T::ALIGN, compute end = aligned + size
4. single-branch fit check (see "Single-branch fit check" below)
5. write back data_ptr := end  (and drop_back := end - entry_size if T: Drop)
6. write the value into the slot
7. for Rc/Box:  bump current_local.smart_pointers_issued  (1 Cell add — non-atomic)
   for Arc:     bump current_shared.smart_pointers_issued (1 Cell add — non-atomic)
   for &T:      set current_local_pinned = true            (1 byte store, idempotent)
```

**Zero atomic operations on the bump hot path** — for *any* of the
allocation flavors. Atomic ops only happen on cross-thread
`Arc::clone` / `Arc::drop` (one Relaxed RMW each) and on the cold
chunk-swap path (one `fetch_sub` per shared swap-out, plain `Cell`
write for local). The bump cursor lives on the arena (a `!Send`
type), so even shared-flavor bump cursors are non-atomic. No
locks. No virtual calls. No allocation. No system calls. The hot
path inlines into every public `alloc_*` call site.

### Single-branch fit check

The fit check used by every fast path is `try_bump_fit` (in
`arena.rs`). It returns a `BumpFit { fits: bool, aligned_ptr,
end_ptr, new_drop_back_ptr }` struct and compiles down to:

```text
aligned     = align_up(data_ptr, align)
end         = aligned + bumped + entry_size
if end > drop_back { miss → slow path }
new_drop_back = drop_back - entry_size
```

On x86-64 it lowers to one `align_up`, one `add`/`lea`, one
`cmp`+`ja`. The arithmetic uses plain `+` rather than saturating
math; callers establish `bumped + entry_size <= isize::MAX` (via
`Layout::array` on the slice paths — which itself enforces
`size_aligned <= isize::MAX` — or
`assert_unchecked(bumped <= MAX_CHUNK_BYTES)` on the value paths)
and `try_bump_fit` re-asserts the bound so LLVM elides the
overflow checks.

The struct return shape (rather than `Option<(NonNull, NonNull,
NonNull)>`) avoids a dead `test rax, rax / je` at the call site:
with the `Option`-of-`NonNull` shape, LLVM folds the `None`
discriminant into one of the `NonNull` niches and emits a niche
check that `assert_unchecked` could not reliably eliminate. A
plain `bool` discriminant decomposes via SROA into a flag register
and the check is elided.

### Value vs. closure fast paths

The crate exposes two flavors of allocation entry point per smart
pointer family:

- **Value-by-value** (`alloc`, `alloc_rc`, `alloc_box`, plus their
  `try_*` siblings) — caller hands in a fully-constructed `T`.

- **Closure-based** (`alloc_with`, `alloc_rc_with`, `alloc_box_with`,
  `alloc_arc`, `alloc_arc_with`, plus `try_*` siblings) — caller
  hands in `F: FnOnce() -> T`. The closure runs *after* the bump
  reservation is made and may reentrantly allocate from the same
  arena.

These route through two different inner helpers:

- `try_alloc_inner_value::<T>` — used by the value-by-value paths.
  Because `core::ptr::write` is infallible and there is no
  user-supplied closure to reenter, this path **skips every piece
  of reentrancy machinery**: no `ProtectiveHold` panic guard,
  no pre-advance of `data_ptr` / `drop_back`, no noop drop-entry
  pre-write, no post-write `chunk_unchanged` recheck, and no
  `commit_alloc_after_eviction` cold tail. The drop entry (if
  any) is written directly with the real shim. The result is one
  bump check, one value write, one cursor advance, and optionally
  one drop-entry write plus drop-count bump.

- `try_alloc_inner_with::<T, F>` — used by the closure-based paths
  and by `alloc_arc` (Arc internally wraps the value into a
  closure to share the closure-path machinery). Carries the full
  reentrancy protocol described in [Reentrancy in closure-based
  paths](#reentrancy-in-closure-based-paths).

The split keeps the hot path tight for the common
already-have-a-`T` case while still supporting closure-based
in-place construction with reentrancy and panic safety.

### Panicking vs fallible inner methods

Public allocation APIs come in two flavors: panicking (`alloc`,
`alloc_box`, ...) and fallible (`try_alloc`, `try_alloc_box`, ...).
Each panicking inner method has a **panic-first sibling**:

| Fallible inner                       | Panic-first sibling                  |
|--------------------------------------|--------------------------------------|
| `try_alloc_inner_value`              | `alloc_inner_value_or_panic`         |
| `try_alloc_inner_with`               | `alloc_inner_with_or_panic`          |
| `try_alloc_inner_arc_with`           | `alloc_inner_arc_with_or_panic`      |
| `try_alloc_str_inner`                | `alloc_str_inner_or_panic`           |
| `try_alloc_slice_local_copy`         | `alloc_slice_local_copy_or_panic`    |
| `try_alloc_slice_local_with`         | `alloc_slice_local_with_or_panic`    |
| `try_alloc_slice_shared_with`        | `alloc_slice_shared_with_or_panic`   |

The siblings have parallel bodies; failure points call
[`panic_alloc`] instead of returning `Err(AllocError)`. Public
panicking APIs call the panic-first sibling; public `try_*` APIs
keep using the fallible inner. The cold tail (`*_slow`,
`*_oversized`, `refill_*`) shares a single fallible implementation
between the two — its `Result` return value is `expect_alloc`-ed
in the panic variant, which is fine because the cold-path call
itself is rare and `#[inline(never)]`.

The split avoids a dead `test rax, rax / je` per iteration at
every call site of a panicking entry point. With
`Result<NonNull<_>, AllocError>` LLVM folds the `Err` discriminant
into `NonNull`'s niche, and the niche check survives the inlining
of `expect_alloc(try_*)`. The panic-first inner method returns a
bare `NonNull<_>` and so has no niche check anywhere on the hot
path. Visible at the bench level: replacing the layered approach
with panic-first siblings saved ~8–10% Callgrind instructions per
panicking allocation on `alloc`, `alloc_box`, `alloc_rc`,
`alloc_arc`, and the `alloc_slice_copy` family.

### Smart-pointer clone

```text
1. mask the pointer to its 64K boundary  (1 AND)
2. load the chunk's refcount field         (1 load)
3. inc refcount                            (Cell add for Rc/Box, atomic
                                            fetch_add Relaxed for Arc)
4. cold abort if the new refcount crosses LARGE * 2 (overflow)
```

No dereference of any wrapper struct, no `Arc::clone`-style
strong+weak pair. One atomic op for `Arc`, one Cell op for `Rc`.
The overflow check is cheap (one compare against a constant) and
calls a `#[cold] #[inline(never)]` `refcount_overflow_abort()` that
terminates the process — see [Refcount overflow](#refcount-overflow-abort-not-debug_assert).

### Smart-pointer drop

```text
1. mask the pointer to its 64K boundary  (1 AND)
2. dec refcount                            (Cell sub / atomic
                                            fetch_sub Release)
3. branch: refcount reached 0 ?            (compare + cold branch)
4. (cold) hand the chunk back to the provider
```

The "refcount reached 0" branch is `#[cold]` and out-of-line. In
the steady state of a working arena, the chunk's refcount almost
always stays nonzero on a drop, so the inlined path is just a
decrement plus a compare.

### What's deliberately kept off the hot path

| Cost                                              | Where it lives                                                                                              |
|---------------------------------------------------|-------------------------------------------------------------------------------------------------------------|
| `Weak<ChunkProvider>::upgrade`                    | only when refcount hits 0                                                                                   |
| Cache list push/pop, list traversal               | only on chunk refill / chunk return                                                                         |
| High-water-mark check                             | only on chunk return                                                                                        |
| Drop-list replay (`drop_in_place`s)               | only when the chunk is returning home                                                                       |
| 64 KiB-aligned system allocation                  | only when no cached chunk fits                                                                              |
| Pinning a chunk into the arena's list             | only on the very first simple-ref alloc per chunk (see "Lazy pinning")                                      |
| Atomic CAS on the shared cache                    | only on `SharedChunk` return / acquire                                                                      |
| Atomic refcount RMW on `alloc_arc`                | **none in steady state** — replaced by a non-atomic `smart_pointers_issued++` plus one `fetch_sub` per chunk swap-out |
| noop drop-entry pre-write / `ProtectiveHold` etc. | only on closure-path allocations (`alloc_*_with`); skipped entirely on value-by-value paths                 |

Every one of these is paid at most once per chunk per its working
lifetime, not per allocation.

### Memory layout for cache-friendliness

- `Arena::current_local` and `Arena::current_shared` are sized so
  the bump cursor (`data_ptr`), the drop-list limit pointer
  (`drop_back`), the chunk pointer, and the per-flavor counters fit
  near each other in the arena's hot cache lines. A bump
  allocation against the current chunk touches one or two cache
  lines on the arena.

- `LocalChunk` puts `provider` (cold, only touched on chunk return)
  and `capacity` (warm, read on overflow checks) at the head of the
  header; `refcount`, `next`, and `drop_count` after them.
  `drop_count` is the smallest field (`u16`) and lives at the end
  to avoid a 6-byte padding hole between two align-8 fields.

- `SharedChunk::refcount` lives at offset 0, ahead of the other
  fields, so the 64-byte alignment `CachePadded` imposes doesn't
  force ~48 bytes of padding before it. The cache padding still
  isolates cross-thread clone/drop traffic from the read-mostly
  `allocator`/`provider`/`capacity` fields that follow. `drop_count`
  is at the end for the same padding-hole reason as `LocalChunk`.

- Chunks are 64 KiB-aligned. The mask used to recover the chunk
  header from any smart pointer (`addr & !(CHUNK_ALIGN - 1)`) is
  one instruction, with no branch.

### Lazy pinning (no overhead for smart-pointer-only arenas)

`Arena` carries a `current_local_pinned: Cell<bool>` flag, initially
false, as a sibling field next to `current_local`:

- **Smart-pointer allocations** (`alloc_rc`, `alloc_arc`,
  `alloc_box`) do **not** touch `current_local_pinned`. They bump
  `current_local.smart_pointers_issued` (or
  `current_shared.smart_pointers_issued` for Arc) instead.

- **Simple-reference allocations** (`alloc`, `alloc_str`,
  `alloc_slice`) set `current_local_pinned = true` after the bump
  succeeds. No list push, no atomic op — just a single byte write
  to a field already in the cache line we already touched.
  Idempotent.

- When the current chunk is **swapped out** (refilled because it's
  full), the swap path checks `current_local_pinned`:

  - if true, the chunk is pushed onto `Arena::pinned_local` and the
    +1 the `current_local` slot was holding is *transferred* to
    that pin entry (no refcount change — the +1 simply changes
    hands);

  - if false, the swap-out reconcile may release the +1 (chunk may
    go to 0 and self-recycle).

The flag lives as a separate field rather than inside `CurrentChunk`
because shared chunks have no equivalent concept; keeping it
outside lets the same generic `CurrentChunk<C>` definition serve
both flavors.

The result: an arena that only ever creates `Rc`/`Arc`/`Box`
values pays nothing for pinning bookkeeping. An arena that hands
out simple references pays one byte-write per simple-ref alloc
plus one list push per chunk that gets retired while still pinned.

There is **no shared pin list**: simple references are only
emitted from `LocalChunk` (the `&T`/`&str`/`&[T]` paths all bump
the local slot), so `SharedChunk` never needs pinning.

### Reset

`Arena::reset(&mut self)` is the steady-state "reuse the arena"
operation. The flow:

1. Replay drop entries on each pinned chunk. Drop-free arenas
   skip this entirely because the chunks' `drop_count` is zero.

2. Walk the pinned list, releasing the +1 each entry holds. Each
   release is a refcount dec + a "did I hit 0?" check; chunks
   that hit 0 self-return through the provider's cache (the
   typical case at reset time, since simple refs by definition
   can't outlive the `&mut`).

3. Reset `current_local` / `current_shared` cursors and drop-back
   limits to "empty" for the still-current chunks, keeping them
   active. No chunk re-acquisition is needed in the common case.

For an arena warmed up at the workload's natural size class, with
no smart pointers outliving the reset, `reset` is essentially:

```text
release pinned-list entries (often 0)
reset bump cursor and drop-back on current_*
```

i.e. dominated by the pinned-list walk. No system allocator call,
no atomic CAS against the shared cache (the `current_*` chunks stay
live), no atomic op against the provider in the steady state.
Repeated build-then-reset cycles run almost entirely out of the
same two chunks.

`reset` is sound because it takes `&mut self`: the borrow checker
proves no simple references survive across the call.

## Top-level pieces

```text
ChunkProvider  ── Arc ──>  (cached LocalChunks / SharedChunks)
      ^                              ^
      |                              |
      Arc                            Arc
      |                              |
    Arena  ──current_local──>  LocalChunk    (DST, [u8] tail)
           ──current_shared─>  SharedChunk   (DST, [u8] tail)
```

- An `Arena<A>` owns `Arc<ChunkProvider<A>>` plus its two "current"
  chunks.

- Each chunk holds a `Weak<ChunkProvider<A>>` (for returning itself
  to the cache while the provider is alive) **and** a separate clone
  of the user-supplied allocator `A` (so the chunk can free its own
  backing allocation even after the provider is gone). When a chunk's
  refcount reaches zero, it tries `Weak::upgrade` first; on success
  it routes through the provider's cache, on failure it self-frees
  through its own allocator clone.

- User-facing smart pointers (`Box<T>`, `Rc<T>`, `Arc<T>`, `RcStr`,
  `ArcStr`, `BoxStr`, and the UTF-16 variants) are **single raw
  pointers directly into a chunk's payload area**. They keep the
  chunk alive only by the +1 they contribute to its refcount. The
  owning chunk's header is recovered on every clone/drop by
  masking the smart pointer's address down to its 64 KiB
  boundary (see "Single-pointer smart pointers" below).

## `LocalChunk` and `SharedChunk`

There are two independent chunk types, with no shared trait between
them. Each is a DST with a `[u8]` tail. Neither carries a bump
cursor — that lives on the `Arena`.

```rust
#[repr(C)] // alignment enforced via `Layout::from_size_align(_, CHUNK_ALIGN)`
struct LocalChunk<A: Allocator + Clone> {
    /// Clone of the user-supplied backing allocator. Cloned from
    /// the provider at chunk-creation time; kept alive at least as
    /// long as the chunk so the chunk can free its own backing
    /// allocation even if the provider has already been dropped.
    allocator: A,
    /// Lets the chunk return itself to the provider's cache while
    /// the provider is still alive. If `upgrade()` fails, the
    /// arena that created this chunk has already been dropped,
    /// the cache is gone, and the chunk frees itself directly.
    provider: Weak<ChunkProvider<A>>,
    capacity: usize,
    refcount: Cell<usize>,
    /// Intrusive list link. Used by the arena's pinned-chunks list
    /// while the chunk is pinned, or by the provider's cache list
    /// after the chunk is returned. `None` when in neither.
    next: Cell<Option<NonNull<Self>>>,
    /// Number of `DropEntry`s on the back-stack. The base offset
    /// of the back-stack is `capacity - drop_count * size_of::<DropEntry>()`.
    /// Placed after `next` so the `u16` sits flush against `data`
    /// (align 1) rather than forcing a 6-byte padding hole.
    drop_count: Cell<u16>,
    data: [u8],
}

#[repr(C)] // alignment enforced via `Layout::from_size_align(_, CHUNK_ALIGN)`
struct SharedChunk<A: Allocator + Clone> {
    /// Refcount is isolated on its own cache line so cross-thread
    /// clone/drop traffic on this atomic doesn't ping-pong the
    /// line that holds the read-mostly fields below. Placed first
    /// so the 64-byte alignment `CachePadded` imposes doesn't
    /// force ~48 bytes of padding ahead of it.
    refcount: CachePadded<AtomicUsize>,
    allocator: A,
    provider: Weak<ChunkProvider<A>>,
    capacity: usize,
    /// Intrusive cache-list link, stored as a thin `AtomicPtr<u8>`
    /// to the next chunk's header (the slice fat pointer cannot
    /// live in an atomic). The fat `*mut SharedChunk` is
    /// reconstructed on demand by reading the target's `capacity`.
    /// Mutated by the Treiber-stack push (any thread) and the
    /// single-consumer pop (owner thread). Push uses
    /// `head.compare_exchange(_, _, AcqRel, Acquire)` with the
    /// chunk's `next` already stored Relaxed; the Release on the
    /// head publishes the chunk fields to any subsequent popper.
    next: AtomicPtr<u8>,
    /// Same role as `LocalChunk::drop_count`. `AtomicU16` because
    /// `Arc::<MaybeUninit<T>>::assume_init` can run on a non-owner
    /// thread and walks the back-stack to retarget the placeholder
    /// shim; owner-side writes are Release, the cross-thread reader
    /// uses Acquire, owner-only RMWs are Relaxed. Placed after
    /// `next` for the same padding-hole reason as `LocalChunk`.
    drop_count: AtomicU16,
    data: [u8],
}
```

Each type has its own inherent `impl` block exposing the safe
primitives the rest of the crate needs (`capacity`, `inc_ref`,
`dec_ref`, `replay_drops`, `reconcile_swap_out`, …). Keeping the
two types independent lets each one own its thread-safety story
(non-atomic vs. atomic) without trait-level genericity.

### Single-pointer smart pointers (64 KiB chunk alignment)

Every chunk allocation — both `LocalChunk` and `SharedChunk` — is
**aligned to a 64 KiB boundary** (`CHUNK_ALIGN = 65 536`). The
alignment is enforced at allocation time via
`Layout::from_size_align(total, chunk_align::<A>())`, not via a
`repr(align(…))` attribute on the struct itself: keeping the
struct's structural alignment small means `size_of_val(&*fat_ptr)`
matches the actual allocation even for small classes (a 512-byte
class-0 chunk really is 512 bytes on the heap).

This 64 KiB alignment is the foundation of the crate's compact
smart-pointer representation: every user-facing smart pointer
(`Box<T>`, `Rc<T>`, `Arc<T>`, `BoxStr`, `RcStr`, `ArcStr`, and the
UTF-16 variants) is a single raw pointer (or, for `T: ?Sized`, a
fat pointer) into the chunk's `data` tail. No separate chunk
handle, no length stored alongside (string lengths live inline in
the chunk data, in front of the bytes).

To find the owning chunk's header from a smart-pointer value, each
smart-pointer type masks the low bits of its raw pointer to its 64
KiB boundary and casts the result to its own statically-known chunk
type. There is **no runtime flavor discriminator** in the chunk
header — `Rc::drop` always casts to `*const LocalChunk`, `Arc::drop`
always casts to `*const SharedChunk`, etc. Those casts live in
`internal/mask.rs`.

Implications:

- The chunk allocation must come from a backing allocator that can
  honour 64 KiB alignment. The default path uses the system
  allocator with an explicit
  `Layout::from_size_align(total, chunk_align::<A>())`. User
  allocators with `align_of::<A>() > CHUNK_ALIGN` raise the
  effective alignment further.

- The maximum supported alignment for any allocation that produces
  a smart pointer is `MAX_SMART_PTR_ALIGN = CHUNK_ALIGN / 2 =
  32 KiB`. `try_alloc_*` returns `AllocError` on requests at or
  above that bound; `alloc_*` panics.

- **Oversized chunks (those whose `header + capacity` exceeds
  `MAX_CHUNK_BYTES = 64 KiB`) hold exactly one
  smart-pointer-producing allocation,** placed at the start of
  the chunk's payload. The chunk is still 64 KiB-aligned and the
  value pointer lives within the chunk's first 64 KiB tile, so
  the same mask recovers the chunk header. After producing its
  single allocation, an oversized chunk is **retired immediately**
  via `retire_oversized_chunk` — subsequent allocations route to
  a normal current chunk. The "one allocation per oversized
  chunk" invariant is structural, not policy.

- Smart-pointer **clone** does: mask → load header → bump refcount
  (atomic for `SharedChunk`, non-atomic for `LocalChunk`).

- Smart-pointer **drop** does: mask → load header → decrement
  refcount; if it dropped to zero, route the chunk back to its
  provider for caching or freeing.

- `Box<T>` is also a single (or fat, for `T: ?Sized`) pointer; its
  `Drop` runs `T::drop` in place via `core::ptr::drop_in_place`,
  then releases its +1 hold on the chunk's refcount. For DST
  boxes (`Box<[T]>`, `Box<dyn Trait>`, custom DSTs from the `dst`
  feature) the fat pointer's metadata drives `drop_in_place`
  directly — no co-allocated runtime drop function is needed.

### Why DSTs?

A DST tail lets a single allocation hold both the chunk header and
its bump arena, with a single `dealloc` at the end of life. No
separate heap allocation for the storage area; one fewer
indirection on every bump.

## `ChunkProvider`

`ChunkProvider` is the factory and cache for chunks. Each `Arena`
owns exactly one `ChunkProvider` (held strongly via
`Arc<ChunkProvider>`); chunks hold back-references via
`Weak<ChunkProvider>`. The provider is **not** shared between
arenas — `Arena` is `!Send`, `!Sync`, and `!Clone`, so the strong
`Arc<ChunkProvider>` is held on a single thread. `SharedChunk`s
are nevertheless `Send + Sync`, so a smart pointer they back may
be dropped on a different thread; the provider itself is `Sync`,
and the shared cache is a lock-free Treiber stack (multi-producer
pushes, owner-thread-only pops; see [Cache representation](#cache-representation)
for the soundness argument).

### Backing allocator: generic, stored by value

`ChunkProvider`, `LocalChunk`, and `SharedChunk` are all generic in
the user-facing allocator type `A: Allocator + Clone`. Each chunk
holds its own clone of `A` so it can free its own backing
allocation even after the provider is gone:

```rust
struct ChunkProvider<A: Allocator + Clone> {
    allocator: A,
    // ... cache fields ...
}
```

`Arena::new_in(a: A)` clones `a` into the provider; the provider
clones it once more into every chunk it allocates. `A` is dropped
only when the last holder is released — both the provider **and**
every surviving chunk.

Lifetime sequence on `Arena::drop`:

1. Arena releases pinned and `current_*` +1s; chunks at zero
   round-trip through the provider's cache.
2. Arena drops `Arc<ChunkProvider<A>>` — last strong, so provider's
   `Drop` runs: walks both cache lists and frees each cached
   chunk through its (still-live) allocator clone.
3. Provider drops its own `A` clone.
4. If there *are* surviving chunks (held by user-facing smart
   pointers), each carries its own `A` clone. Their
   `Weak::upgrade` will return `None`; when their refcount
   eventually reaches zero on whichever thread drops the last
   smart pointer, they self-free their backing allocation through
   their own `A` clone. The very last surviving chunk's drop
   releases the final `A` clone, dropping the underlying allocator
   state at that point.

Storing `A` by value (rather than going through a refcounted
type-erasure wrapper) keeps the chunk header small (especially for
ZST allocators like `Global`) and removes one indirect call from
each chunk allocation/deallocation.

### `Send` / `Sync` story

| Type | `Send` | `Sync` | Notes |
|------|--------|--------|-------|
| `Arena<A>` | no | no | `!Clone`; held on its construction thread |
| `ChunkProvider<A>` | yes | yes | shared cache is a lock-free Treiber stack (multi-producer / single-consumer); local cache is wrapped in a `LocalSlot`; requires `A: Send + Sync` |
| `Arc<ChunkProvider<A>>` / `Weak<ChunkProvider<A>>` | yes | yes | so `SharedChunk`s on remote threads can release |
| `LocalChunk<A>` | no | no | non-atomic refcount; touched only on owning thread |
| `SharedChunk<A>` | yes | yes | atomic refcount; smart pointers may move between threads (requires `A: Send + Sync`) |

Synchronization in the provider:

- **Arena pinned list** (`pinned_local`): touched only by the
  arena's owning thread. Plain `Cell` operations. There is no
  shared pin list: simple references are always emitted from
  `LocalChunk`.

- **Local cache** (`ChunkProvider::local_cache`): touched only by
  the arena's owning thread, because `LocalChunk: !Send` makes it
  impossible for another thread to produce a `LocalChunk` to
  release. The list head lives in a `LocalSlot<Option<NonNull<LocalChunk>>>`
  newtype that wraps an `UnsafeCell` and asserts
  `unsafe impl<T> Sync for LocalSlot<T>` — making
  `ChunkProvider: Sync` without paying for an actual lock.

- **Shared cache** (`ChunkProvider::shared_cache_head`): a
  lock-free Treiber stack stored as `AtomicPtr<u8>` (thin pointer
  to the top `SharedChunk`). The list is linked through each
  chunk's `next: AtomicPtr<u8>`. Pushes (any thread, from
  cross-thread `Arc<T>` drops via `release_shared`) use a CAS
  loop. Pops (only the arena's owning thread, via `acquire_shared`)
  are single-consumer — that property eliminates the multi-consumer
  Treiber UAF (no other thread can free `head` between our load
  and CAS) and the ABA hazard (no other thread can pop `head`
  out and back in). The popper reads `capacity` only **after** the
  CAS takes ownership; if the popped chunk is too small for the
  request, the popper frees its backing and loops to try the new
  head. See [Cache representation](#cache-representation) for the
  detailed argument.

- **High-water marks**: `local_high_water` is touched only by
  the owning thread (acquire/release of `LocalChunk`s is
  intrinsically single-threaded), so it lives in a
  `LocalSlot<u8>` with plain load/store. `shared_high_water` is
  `AtomicU8` because shared-flavor acquires/releases can come
  from any thread; updates use `fetch_max(class, Relaxed)`.

The hot path doesn't touch any of this — bump allocation and
smart-pointer clone/drop never reach the provider's cache. Only
the cold chunk-refill / chunk-return paths perform a CAS on the
shared-cache head or borrow the local-cache `LocalSlot`.

### Size classes, high-water mark, and the `max_normal_alloc` knob

Cacheable chunks come in a fixed set of well-known **total
allocation sizes** — powers of two from `MIN_CHUNK_BYTES = 512 B`
to `MAX_CHUNK_BYTES = 64 KiB`, giving `NUM_CHUNK_CLASSES = 8`
classes. `class_to_bytes(class)` and `min_class_for_bytes(bytes)`
(in `internal/constants.rs`) convert between the two
representations. The user-visible **payload** of a class-`c`
chunk is `class_to_bytes(c) - header_size::<A>()` (a few dozen
bytes less than the total — the chunk header eats into the class).

There are two independent concepts:

1. **`max_normal_alloc`** — a per-arena, builder-configurable
   *routing threshold* on user-payload bytes. Default
   `MAX_NORMAL_ALLOC = 16 KiB`; bounds
   `[MIN_MAX_NORMAL_ALLOC = 4 KiB, max_bump_extent::<A>()]`
   (slightly less than 64 KiB; the upper bound depends on
   `header_size::<A>()`). Allocation requests strictly larger
   than `max_normal_alloc` are routed to the **oversized one-shot
   path**, which produces a chunk sized exactly to the request
   (`header_size + round_payload(user_payload)` bytes — no
   class rounding).

2. **The chunk size class system** — fixed by the implementation.
   Normal chunks always have a total allocation equal to one of
   the eight class sizes (so `≤ MAX_CHUNK_BYTES`).
   `max_normal_alloc` does not cap chunk capacity.

The provider tracks **two size high-water marks** — one for
`Local`, one for `Shared` — recording the largest normal class it
has ever produced for that flavor. The high-water mark only ever
ratchets upward; it is not lowered when chunks return to the
cache. Each starts at `0` (= 512 B total) by default; the matching
`ArenaBuilder::with_capacity_local` / `with_capacity_shared`
knob seeds it to the smallest class whose total covers the
requested preallocation (its `bytes` argument is total
chunk-allocation bytes, not payload).

The high-water mark drives two policy decisions:

1. **Acquire rounds up to the high-water class.** When
   `acquire_local`/`acquire_shared` is asked for `min_payload`,
   the provider takes
   `target_class = max(min_class_for_bytes(min_payload + header_size),
   high_water).min(NUM_CHUNK_CLASSES - 1)`. New chunks produced
   on a fresh-allocation miss ratchet the high-water up by one
   class (capped at 7 = 64 KiB).

2. **Release filters by high-water.** When a chunk returns to the
   provider, it is only kept in the cache if all of:

   - `header_size + capacity ≤ MAX_CHUNK_BYTES` (i.e. the chunk
     isn't oversized — one-shot chunks bypass the cache), and

   - `capacity ≥ class_to_bytes(high_water) - header_size` (its
     payload covers the high-water class).

   Otherwise the chunk's backing storage is freed. Because new
   chunks are always at the high-water class, this effectively
   means lower-class chunks produced earlier in the arena's
   lifetime are discarded as they return — the cache "ages out"
   small chunks naturally. The cache itself is **unbounded**:
   the high-water filter is the sole admission rule. (See
   `release_local` / `release_shared` in
   `internal/chunk_provider.rs` for the implementation.)

### API sketch

```rust
impl ChunkProvider {
    pub(crate) fn acquire_local(self: &Arc<Self>, min_payload: usize)
        -> Result<NonNull<LocalChunk>, AllocError>;
    pub(crate) fn acquire_shared(self: &Arc<Self>, min_payload: usize)
        -> Result<NonNull<SharedChunk>, AllocError>;

    /// Called by a chunk's release path when its refcount has just
    /// reached zero. Replays drop entries, then caches or frees.
    pub(crate) unsafe fn release_local(&self, chunk: NonNull<LocalChunk>);
    pub(crate) unsafe fn release_shared(&self, chunk: NonNull<SharedChunk>);
}
```

`acquire_*(min_payload)`:

1. Compute `class = max(min_class_for(min_payload),
   high_water_class)` for the relevant flavor.

2. If `class` is within the normal range, pop the head of the
   flavor's cache list. For local, the list is consulted via
   `LocalSlot::with_mut`; for shared, `try_pop_shared_at_least`
   does a single-consumer Treiber pop, then checks the popped
   chunk's `capacity`. If the head is too small for the request,
   the popper frees its backing (and releases the corresponding
   budget reservation) and retries on the new head, repeating
   until either a fitting cached chunk is found or the cache is
   empty.

3. If no cached chunk fits, allocate a fresh one at the computed
   class and ratchet the high-water mark.

4. If the requested size exceeds the largest normal class,
   allocate a fresh oversized chunk and tag it as non-cacheable;
   the high-water mark is **not** updated.

5. Set the chunk's refcount to its initial value (`LARGE` for
   both flavors — see "Deferred refcount reconciliation") and
   return.

### Cache representation

The cache is a pair of **intrusive singly-linked lists** of chunks
— one per flavor — both **unbounded**. The high-water filter is
the sole admission policy.

```rust
struct ChunkProvider<A: Allocator + Clone> {
    /// Backing allocator, cloned once per chunk created.
    allocator: A,

    max_normal_alloc: usize,
    byte_budget: Option<usize>,
    total_chunk_bytes: AtomicUsize,

    /// Local cache list head. Touched only by the arena's owning
    /// thread (enforced structurally by `LocalChunk: !Send`).
    /// Unbounded.
    local_cache: LocalSlot<Option<NonNull<LocalChunk>>>,

    /// Shared cache list head. Lock-free Treiber stack: pushes
    /// from any thread (any thread can drop the last `Arc<T>`),
    /// pops only by the arena's owning thread (`acquire_shared`
    /// is reached from owner-thread code only). Stored as a thin
    /// pointer to the top `SharedChunk` (or null when empty); the
    /// list is linked through each chunk's
    /// `next: AtomicPtr<u8>` field. Unbounded.
    shared_cache_head: AtomicPtr<u8>,

    /// Largest normal class ever produced for `Local`. Monotonic.
    local_high_water: LocalSlot<u8>,
    /// Largest normal class ever produced for `Shared`. Monotonic.
    shared_high_water: AtomicU8,

    #[cfg(feature = "stats")] stats: StatsStorage,
}
```

The two cache lists are **intrusive**: each chunk's `next` field
links it into whichever list currently owns it. A chunk is in
**at most one list at any time**:

- the arena's pinned-chunks list (while it has handed out simple
  references for that arena), **or**

- the provider's cache list (after its refcount hit zero and it
  was returned to be cached), **or**

- in neither (e.g., when it's the arena's `current_*` and not
  yet pinned, or in flight between owners).

Reusing one intrusive `next` field across all those contexts
costs a single header word per chunk and avoids any
heap-allocated linked-list nodes. For `LocalChunk` that field is
a `Cell<Option<NonNull<Self>>>` (single-threaded access); for
`SharedChunk` it's an `AtomicPtr<u8>` (cross-thread access via
the lock-free Treiber CAS for push/pop).

#### Lock-free shared cache: why mutex-free is sound here

The shared cache is multi-producer, single-consumer:

- **Push** happens from any thread that drops the last `Arc<T>`
  reference on a `SharedChunk`, which routes through
  `release_shared`. Pushes use a classic Treiber CAS loop:

  ```text
  loop {
      h = head.load(Acquire)
      chunk.next.store(h, Relaxed)
      if head.compare_exchange(h, chunk, AcqRel, Acquire).ok() { return; }
  }
  ```

- **Pop** happens only from the arena's owning thread, via
  `acquire_shared` (reached from the owner-thread allocation
  paths; `Arena` is `!Sync`, which structurally enforces this).
  Single-consumer pop is the special case that makes the
  Treiber-stack hazards of the multi-consumer general case
  disappear:

  - **No field-read-before-ownership UAF.** A multi-consumer
    Treiber pop has to read `head.next` before its CAS, but
    another popper can have already taken `head` and freed it
    by then. With single-consumer pop, no other thread can pop,
    so `head` cannot leave the list (and therefore cannot be
    freed) between our `head.load` and our CAS. The read of
    `head.next` is safe.

  - **No ABA.** Single-consumer ABA would require some other
    popper to remove `head` and re-push it; there is no other
    popper, so the head's identity cannot recycle behind our
    back. The CAS therefore needs no tag.

  Concurrent pushes are still allowed during a pop. A push only
  modifies the head pointer and the *new* chunk's `next` field;
  it does not mutate the existing top node's `next`. The pop's
  CAS either succeeds (if no push raced) or fails with the new
  head (if a push raced) — the popper simply retries.

### Cache eviction policy

When a chunk is released and the high-water filter says it should
be cached, it is pushed to the head of its flavor's list — that's
it. There is no count or byte cap on the cache itself. If a
workload genuinely needs to bound the resident chunk population,
set `ArenaBuilder::byte_budget(...)`; if it knows its target
footprint up front,
`ArenaBuilder::with_capacity_local(bytes)` and / or
`with_capacity_shared(bytes)` preallocate the necessary chunks
into the matching cache and seed the matching high-water mark to
the appropriate class so subsequent releases self-evict the
smaller warm-up chunks.

`acquire_*` pops the head. If the head's capacity is below the
request, the popper frees its backing (releasing the budget
charge) and tries again on the new head, repeating until either
a fitting cached chunk is found or the cache is empty. The
post-warmup steady state is "head always fits" (the high-water
rule keeps cached chunks at or above the current class), so the
loop almost always exits after a single CAS. The free-and-retry
behavior matters mainly during the pre-warmup window when chunks
of multiple classes coexist in the cache and on size-class
promotion (smaller cached chunks are evicted on the first pop
that wants the new class).

If the cache is empty, the caller falls through to a fresh
allocation, which ratchets the high-water mark.

## `Arena`

`Arena` is a thin façade over a `ChunkProvider` and two "current"
chunk slots:

```rust
pub struct Arena<A = Global> {
    provider: Arc<ChunkProvider>,

    // Each "current" slot holds one ref on its chunk, distinct
    // from any ref held by outstanding smart pointers or by the
    // pin list. The arena owns the bump cursor for each current
    // chunk; the chunk itself does not.
    current_local:  CurrentLocalChunk,
    /// Lazy-pinning flag for the chunk currently installed in
    /// `current_local`. Kept as a sibling field rather than inside
    /// `CurrentChunk` so the same generic type can serve both
    /// local and shared flavors (shared chunks have no pin
    /// concept).
    current_local_pinned: Cell<bool>,
    current_shared: CurrentSharedChunk,

    // Intrusive list of pinned local chunks. Same `next` field
    // as the provider's cache list — a chunk is in at most one
    // list at a time. One refcount per entry. There is no shared
    // counterpart.
    pinned_local: Cell<Option<NonNull<LocalChunk>>>,

    allocator: A,
}

/// Shared definition for `current_local` (with `C = LocalChunk<A>`)
/// and `current_shared` (with `C = SharedChunk<A>`).
struct CurrentChunk<C: ChunkKind + ?Sized> {
    /// `None` in stub state — the post-`Arena::new` and
    /// post-`reset` shape that produces a free zero-bump-check
    /// failure on the first allocation.
    chunk: Cell<Option<NonNull<C>>>,

    /// Bump cursor: address of the next free payload byte.
    /// Advances forward on each allocation. `NonNull::dangling()`
    /// (= address `1`) in stub state.
    data_ptr: Cell<NonNull<u8>>,

    /// Drop-list limit pointer: address of the start of the
    /// trailing drop-entry back-stack, equivalently one past
    /// the last free byte. Equal to `data_ptr` (= `dangling`)
    /// in stub state, which makes the very first bump check
    /// fail naturally.
    drop_back: Cell<NonNull<u8>>,

    /// Non-atomic counter of smart-pointer-flavor allocations
    /// (`Rc`/`Box` for `current_local`; `Arc` for `current_shared`)
    /// issued from this chunk since it became current. The
    /// chunk's refcount is held inflated at `LARGE` while it's
    /// current; this counter records how many of those +1s are
    /// "real" so the swap-out reconcile can subtract the unused
    /// inflation in one go.
    smart_pointers_issued: Cell<usize>,
}

type CurrentLocalChunk<A>  = CurrentChunk<LocalChunk<A>>;
type CurrentSharedChunk<A> = CurrentChunk<SharedChunk<A>>;
```

The free region of the current chunk is `[data_ptr, drop_back)`.
A bump-with-drop-entry reservation (1) advances `data_ptr` forward
to `aligned + size_of::<T>()`, and (2) moves `drop_back` back by
`size_of::<DropEntry>()`. The chunk's `drop_count` is the canonical
record of how many entries the back-stack contains; the
`drop_back` pointer is reconstructed from `drop_count` whenever a
chunk leaves and re-enters the `current_*` slot.

### Stub state: no "is there a current chunk?" branch on the hot path

`Arena::new` does **not** ask the provider for an initial chunk.
Instead, `current_local` and `current_shared` are initialized to a
**stub state** in which `chunk = None`, `data_ptr = drop_back =
NonNull::dangling()` (address `1`), and the per-flavor counters
are zero.

The stub is a logical "no chunk yet" marker — there is no
chunk-shaped object backing it, no static, no allocation, no
self-reference. The hot bump path doesn't notice the difference:

- **Bump check:** `try_bump_fit` compares `aligned + bumped`
  against `drop_back - entry_size`. With `data_ptr == drop_back ==
  1` and `bumped >= 1` (`size.max(1)` to handle ZSTs), the
  check fails on the very first call and routes into the
  (already-`#[cold]`) refill path.

- **Value writes:** `data_ptr` is never dereferenced in stub
  state because the bump check fails first.

- **Refcount accounting:** the per-flavor counters
  (`current_local.smart_pointers_issued` /
  `current_shared.smart_pointers_issued`) are bumped only after
  the bump
  check succeeds, which the stub never reaches.

The cold refill path is the only place that distinguishes stub
from real-chunk state, via a single `chunk.is_some()` check:

- if `Some(prev)`, perform the swap-out reconcile on `prev`
  (transferring the +1 to the pin list if `current_local_pinned`,
  otherwise decrementing — in which case the chunk may go to 0
  and recycle);

- if `None`, skip the reconcile (there is no `prev`).

Properties of the stub-state design:

- **No allocation at `Arena::new()`.** The first chunk is
  acquired lazily on the first allocation that needs one.

- **No self-references inside `Arena`.** `chunk` is either `None`
  or a `NonNull` into a heap-allocated chunk — never into the
  `Arena`'s own body — so moving the `Arena` is safe.

- **Hot path is unaffected.** Every hot-path field is inline in
  `CurrentLocalChunk` / `CurrentSharedChunk`; the single
  `is_some` check happens only on the cold refill path.

### Reentrancy in closure-based paths

The closure-based allocation paths (`alloc_*_with`, plus
`alloc_arc` which wraps its value in a closure) have a delicate
problem: if the user closure `f: FnOnce() -> T` reentrantly calls
`Arena::alloc_*` on the same arena, that reentrant call must
**not** see the in-progress allocation as free space, and it must
**not** observe an uninitialized drop-entry slot.

The closure-path inner methods (`try_alloc_inner_with` and the
analogous slice/string/DST helpers) handle both hazards by ensuring
that, while `f` runs, the arena's bookkeeping reflects an
already-completed allocation:

1. **Pre-advance** `current_local.data_ptr` (or `current_shared`)
   to the new end address *before* invoking `f`. A reentrant
   `alloc_*` from inside `f` will see the cursor past our
   reserved range and bump from there, so the two reservations
   never overlap.

2. **Pre-write a noop drop entry** at the reserved drop-back slot
   (with `drop_fn = noop_drop_shim`, `value_offset` set, and
   `len = 1`), and bump the chunk's `drop_count`. If `f` panics
   or a reentrant call evicts the chunk before `f` returns, the
   chunk's drop ledger refers to a *valid* (no-op) entry over
   uninitialized payload — never to an uninitialized entry slot
   or to a slot that runs `T::drop` on uninitialized memory.

3. **Install a `ProtectiveHold`** RAII guard that, on unwind,
   undoes the per-flavor counter bump (or, post-eviction,
   `dec_ref`s the now-unowned chunk).

4. **Run the closure**, then `mem::forget(hold)` on success.

5. **Recheck whether the closure evicted the chunk.** If
   `current_*.chunk` no longer equals our reserved chunk, route
   into `commit_alloc_after_eviction`: that helper reconciles
   the per-flavor counter, transfers the protective hold's +1
   into the chunk's actual refcount, and overwrites the noop
   entry with the real drop shim.

6. **On the no-eviction success path**, overwrite the noop drop
   entry with the real shim (`drop_shim_one::<T>`), leave the
   pre-advanced `data_ptr` and `drop_back` in place, and return.

The value-by-value paths (`try_alloc_inner_value` and friends)
**skip every step of this protocol**. They have no closure to
reenter and `core::ptr::write` is infallible, so they
unconditionally:

1. perform the bump check,
2. bump the per-flavor counter (or set `current_local_pinned`),
3. write the `T`,
4. advance `data_ptr` (and `drop_back` + `drop_count` if there's
   a drop entry),
5. write the real drop shim directly.

This is the tighter hot path the value-only public APIs
(`Arena::try_alloc`, `try_alloc_rc`, `try_alloc_box`, plus their
panicking wrappers) take.

### Provider ownership: `Arc` on the arena, `Weak` on the chunk

`Arena` holds the **strong** `Arc<ChunkProvider>`. Each chunk holds
a **`Weak<ChunkProvider>`** rather than a strong reference. This
ties the provider's (and its cache's) lifetime to the lifetime of
the arena, while still letting individual chunks outlive their
arena via smart pointers.

When a chunk's refcount hits zero and it's about to return itself
to its provider's cache, it `upgrade()`s its `Weak`:

- `Some(provider)` → hand the chunk back to the provider for
  caching (or freeing, if oversized or below the high-water
  class).

- `None` → the arena that owned the provider has been dropped;
  the chunk simply frees its own backing allocation through its
  embedded `A` clone and exits.

The `upgrade()` is one atomic op on the chunk-drop path, not the
allocation hot path, so the cost is negligible.

There are **three** ways a chunk can be kept alive:

1. **Smart pointers** — each holds a +1 on the chunk's refcount
   via the pointer-mask path.

2. **The arena's `current_*` slot** — holds one of the +1s in the
   chunk's `LARGE`-inflated refcount.

3. **Simple references** — these carry no refcount of their own;
   their lifetime is tied to the arena via `'a`. To make them
   safe, the arena keeps their chunk alive by lazy pinning.

#### Deferred refcount reconciliation

`alloc_rc` / `alloc_box` / `alloc_arc` are on the bump hot path.
Bumping the chunk's refcount per allocation — even with a
non-atomic `Cell::set` for `LocalChunk` — costs an extra cache
line touch beyond the arena's `current_*` state, because the
chunk header lives in a different allocation than the arena. On
multicore CPUs the `Arc` case is even worse: each `fetch_add(1)`
is an atomic RMW.

Both flavors use a deferred-reconciliation trick:

- **When a chunk becomes `current_*`** (cold refill path), its
  refcount is set to `LARGE = isize::MAX as usize / 2` (Release
  store for shared, plain `Cell::set` for local). This inflation
  is so large (~4.6 × 10¹⁸ on 64-bit) that no plausible mix of
  allocations and remote drops could drive it to zero while the
  chunk is current.

- **`alloc_rc` / `alloc_box` / `alloc_arc`** increment the matching
  `smart_pointers_issued` counter on the arena's `current_*`
  slot — a `Cell<usize>` in the arena's hot cache line. The
  chunk's refcount is **not** touched.

- **`Rc::clone` / `Arc::clone` / `Box`-after-the-fact paths**
  still use the chunk's refcount (Cell add for local, atomic
  fetch_add for shared).

- **At chunk swap-out** (cold refill path), the arena reconciles
  in **one** operation. Let `N = smart_pointers_issued`.
  For local chunks, `pinned = current_local_pinned ? 1 : 0`; for shared
  `pinned = 0`. Compute `to_subtract = LARGE - N - pinned`, then
  subtract (atomic for shared, plain Cell for local). If the
  result is zero, route to the provider for caching or freeing.
  Implementation: `LocalChunk::reconcile_swap_out` and
  `SharedChunk::reconcile_swap_out`.

- **Math.** With `C` clones and `D` drops of smart pointers
  issued from this chunk during its tenure, the refcount
  immediately before swap-out is `LARGE + C - D`. After
  `sub(LARGE - N - pinned)` it becomes `N + C - D + pinned` —
  exactly the count of outstanding smart pointers plus the
  optional pin reservation.

- **Acquiring a chunk from the cache** sets its refcount to
  `LARGE` before handing it out — restoring the inflation
  invariant for the new arena's tenure.

#### Chunk → provider release

When a chunk's refcount transitions to zero, it calls back into
its `ChunkProvider` (via `Weak::upgrade`). The provider's
`release_local` / `release_shared`:

1. **Replay the drop list** (`replay_drops`) — invokes each
   `(drop_fn)(data + value_offset, len)` in most-recent-first
   order, then sets `drop_count = 0`.

2. **Decide cache eligibility**: chunk is eligible iff
   `header_size + capacity ≤ MAX_CHUNK_BYTES` (not oversized) and
   `capacity ≥ class_to_bytes(high_water) - header_size` (covers
   the high-water class).

3. **Eligible** → push onto the flavor's cache list (via the
   single Treiber CAS for shared); the chunk's bump cursor is
   implicitly reset on the next `acquire_*` (the cursor lives on
   the arena, not the chunk).

4. **Ineligible** → free the chunk's backing allocation via the
   embedded `A` clone.

## `Drop` support

For `T: Drop` we make sure `T::drop` runs at the right moment.
There are two distinct mechanisms.

### Trailing drop list (simple refs, `Rc`, `Arc`, DST `Box`)

For values reached via `Arena::alloc` / `alloc_rc` / `alloc_arc`,
and for unsized `Box` values from the `dst` feature, the value
lives somewhere in the chunk's `data` payload and may be reached
by many readers (simple references) or by clones of a smart
pointer. There's no single "owner" who can run `T::drop`, so the
chunk does it in bulk when it tears down.

Each such allocation reserves both:

- `size_of::<T>()` (aligned) at the front of the free region, and

- one `DropEntry` slot at the back of the free region.

The effective remaining capacity is `back_offset - cursor`, and
overflow is detected when those two meet.

The `DropEntry` (defined in `internal/drop_list.rs`):

```rust
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct DropEntry {
    /// Type-erased shim that performs `drop_in_place::<T>` (when
    /// `len == 1`) or `drop_in_place::<[T]>` (when `len > 1`).
    /// DST allocations encode the pointee metadata in `len`.
    drop_fn: unsafe fn(value: *mut u8, len: usize),
    /// Byte offset within the chunk's `data` payload. `u16` is
    /// sufficient because cached chunks are at most 64 KiB and
    /// oversized chunks place their single allocation at offset 0.
    value_offset: u16,
    /// Number of `T`s starting at `value_offset`. `1` for ordinary
    /// single-value entries; `> 1` for slice entries; reused as
    /// pointee-metadata for DST entries.
    len: u16,
    _pad: [u8; PAD_BYTES],
}
```

The chunk header carries a `drop_count: Cell<u16>`; the
back-stack base offset is derived as
`capacity - drop_count * size_of::<DropEntry>()`. This avoids
storing a separate `drop_back` field on the chunk and lets the
swap-in / swap-out paths reconstruct the limit pointer from
`drop_count` alone.

When `replay_drops` runs (refcount-zero or pin-list release), the
chunk pops entries most-recent-first and invokes each
`(drop_fn)(data + value_offset, len)`. Then the chunk is returned
to the provider (cached or freed).

A few important properties:

- Allocations of `T: !Drop` skip the `DropEntry` reservation
  entirely (`entry_size = 0` in the bump-fit check) — there's no
  back-of-payload cost in the common allocator-of-POD case.

- `Rc<T>::drop` / `Arc<T>::drop` only decrement the chunk
  refcount — they do **not** run `T::drop` themselves. The
  chunk's trailing drop list runs it later, at chunk-return
  time. This is the trade-off that makes `Rc::clone`/`drop` so
  cheap and avoids needing a per-allocation "live" bit. Users
  who need eager destructor semantics use `Box` instead.

- For `alloc_slice_*_rc`/`_arc`, the entry's `len` is set to the
  slice length so the shim runs `drop_in_place::<[T]>`. For
  ordinary single-value `alloc_rc`/`alloc_arc`, `len = 1` and
  `drop_shim_one::<T>` runs.

- DST allocations (under the `dst` feature) also use the trailing
  drop list, with a DST-specific shim
  (`dst_drop_shim_trailing::<T>`) and the pointee metadata
  encoded in the entry's `len` field. The DST path adopts the
  same noop drop-entry pre-write protocol as the closure-based
  value paths so a panic mid-init can never leave an
  uninitialized drop-entry slot for replay to crash on.

### Sized `Box`: drop-in-place via the smart pointer, with a noop entry reserved for `into_rc`

`Box<T>` is the unique-owner flavor: `Box::drop` runs `T::drop`
**immediately** and only afterwards releases the chunk refcount.
`Box<T>` carries a thin `NonNull<T>` (or a fat pointer for the
DST cases); `drop_in_place` dispatches directly through that
pointer without consulting the chunk's drop list.

Even though `Box::drop` doesn't need a drop-list entry, the
**normal-sized** Box alloc path *does* reserve a
`noop_drop_shim` entry up front when `T: Drop`. The reason is
`Box::into_rc`: that conversion transfers the chunk's `+1` to a
new `Rc<T>`, which is dropped through the chunk's drop list, not
through `Box::drop`. Without a pre-installed entry there would be
nowhere for the conversion to retarget the shim to
`drop_shim_one::<T>`. The entry is reserved eagerly because the
conversion has no `&Arena` and therefore cannot update the
arena's `drop_back` mirror itself — it only rewrites the existing
slot's `drop_fn` field. (Without this eager reservation, an
attempt to install the entry post-hoc would read a stale
`drop_back` from a subsequent allocation and collide; see the
regression test `arena_box_into_rc_does_not_corrupt_drop_list`.)

The **oversized** Box path (one-shot chunks for allocations larger
than `MAX_NORMAL_ALLOC`) does *not* reserve an entry: oversized
chunks are never reused after their tenant is dropped, so no
later allocation can collide with a post-hoc entry, and the
oversized `Box::into_rc` path either copies (DST cases) or uses
the chunk's lifetime directly.

| Box flavor                                | Drop strategy                                                                                                           | Per-allocation entry cost     |
|-------------------------------------------|-------------------------------------------------------------------------------------------------------------------------|-------------------------------|
| `Box<T: Sized>`, `T: !Drop`               | nothing                                                                                                                 | 0 bytes                       |
| `Box<T: Sized>`, `T: Drop` (normal-sized) | `drop_in_place::<T>` via the box's thin ptr; chunk holds a `noop_drop_shim` slot for `into_rc` retargeting              | `size_of::<DropEntry>()`      |
| `Box<T: Sized>`, `T: Drop` (oversized)    | `drop_in_place::<T>` via the box's thin ptr; chunk freed at refcount-zero                                               | 0 bytes                       |
| `Box<[T]>`                                | `drop_in_place` via the slice fat pointer; chunk holds a `noop_drop_shim` slot for `into_rc` retargeting when `T: Drop` | 0 or `size_of::<DropEntry>()` |
| `Box<dyn Trait>` / custom DST             | `drop_in_place` via the DST fat pointer                                                                                 | 0 bytes                       |

A `ReleaseGuard` inside `Box::drop` ensures the chunk's `+1` is
released even if `T::drop` panics (see `box.rs`).

When `Box::into_rc` is called, `retarget_box_drop_entry` walks the
chunk's drop-back list looking for the entry whose `value_offset`
matches this Box's value pointer, and rewrites its `drop_fn` to
the real drop shim. If no entry is found (e.g. the Box was
constructed from a path that didn't install one, such as
`alloc_box(MaybeUninit::<T: Drop>::uninit())` followed by
`assume_init().into_rc()`), the helper is a **silent no-op** — it
returns without rewriting anything. The value's destructor will
then not run on chunk teardown. This is a memory leak, not
unsoundness, and mirrors `Rc::assume_init`'s identical
silent-on-miss policy. Callers needing eager-drop on conversion
should use `alloc_uninit_box::<T>()`, which installs the entry up
front.

### Refcount overflow: abort, not `debug_assert`

Both `LocalChunk::inc_ref` and `SharedChunk::inc_ref`, plus the
arena's `bump_smart_pointers_issued` helpers, check for
overflow against `LARGE.saturating_add(LARGE) ==
isize::MAX as usize`. If a refcount or per-flavor counter ever
crosses that threshold, the helper calls
`refcount_overflow_abort()` (in `internal/constants.rs`). With
the `std` feature enabled, that helper calls
`std::process::abort()`. Under `no_std`, it forces a double-panic
(a nested `panic!` inside a `Drop`) so the panic handler aborts.
Either way mirrors `std::sync::Arc`'s behavior: a wraparound
would let live pointers race with a free, and the only sound
response is to terminate the process. The abort helper is
`#[cold]` and `#[inline(never)]` so the call site stays small on
the hot path.

Earlier revisions guarded these only with `debug_assert!`. That
left release builds with no defense at all on the (admittedly
implausible) overflow path; the abort is now an unconditional
release-build check, with `debug_assert!`s retained for the
"refcount must be positive" precondition.

## Builder collections: `Vec`, `String`, `Utf16String`

The arena exposes three growable builders. They are *transient* —
small fixed-size structs that point at a buffer inside the arena
and let the caller append to it incrementally — and they all
support an O(1) **freeze** into an immutable smart pointer once
building is complete.

| Builder              | Element / encoding       | Freezes into                                          |
|----------------------|--------------------------|-------------------------------------------------------|
| `Vec<'a, T, A>`      | arbitrary `T`            | `Rc<[T], A>` / `Arc<[T], A>` / `Box<[T], A>`          |
| `String<'a, A>`      | UTF-8 bytes              | `RcStr<A>` / `ArcStr<A>` / `BoxStr<A>`                |
| `Utf16String<'a, A>` | validated UTF-16 (`u16`) | `RcUtf16Str<A>` / `ArcUtf16Str<A>` / `BoxUtf16Str<A>` |

### Common shape

Each builder is a small struct that carries a borrowing reference
to its arena, a pointer to the start of its element buffer, a
length, and a capacity. Builders are **`Local`-flavor** by
construction — they live inside the arena, are `!Send` /
`!Sync`, and they bump-allocate from the arena's `current_local`
chunk just like simple references.

### Initial buffer allocation

`Arena::alloc_vec()` / `alloc_string()` / `alloc_utf16_string()`
reserve an initial buffer in the current local chunk via the same
forward bump path used for simple references. The first
allocation reserves a small starting capacity (an implementation
knob).

For `String` / `Utf16String`, the buffer is laid out as
`[length prefix][payload]`, with `builder.data` pointing at the
first payload byte. The length prefix is reserved at allocation
time so the eventual frozen smart pointer can find it at a fixed
negative offset from `data`. `Vec` doesn't need a length prefix
because `Rc<[T]>` / `Arc<[T]>` / `Box<[T]>` are slice fat
pointers that already carry the length in their metadata.

### Growing

`push` / `push_str` / `extend_from_slice` first checks the spare
capacity (`cap - len`) and, if it fits, just writes into the
buffer. On overflow, the builder asks the arena to **grow** the
buffer:

1. **Extend in place at the chunk cursor (zero copy).** The
   buffer can only be extended in place if its end coincides with
   `current_local.data_ptr` *and* the chunk hasn't handed out any
   other allocation since this buffer was last grown. If both
   hold, the arena bumps `data_ptr` by the delta and the builder
   updates its `cap` in place. **No copy.**

2. **Reallocate elsewhere (with copy).** Otherwise the arena
   allocates a fresh buffer of the new (larger) capacity, the
   builder memcopies its existing `len` elements into the new
   buffer, updates `data` and `cap`, and abandons the old
   buffer. The old buffer's bytes stay reserved inside their
   owning chunk until the chunk is returned. This wasted-bytes
   cost is what funds the cheap freezes; relocations are
   counted in `ArenaStats::relocations`.

Growth doubles capacity by default (matching `alloc::Vec`), with
the first grow honoring any explicit
`Arena::alloc_*_with_capacity` request.

### Drop semantics during build

A `Vec<T: Drop>` builder dropped without being frozen runs
`T::drop` on each of its `len` live elements eagerly, in its own
`Drop` impl. The buffer's bytes are not freed (they belong to the
chunk), but no `DropEntry` is installed on the chunk for them —
the elements have already been dropped. The chunk reclaims the
storage on its eventual teardown.

`String` / `Utf16String` hold no `Drop` elements so their `Drop`
impls are no-ops apart from forgetting the buffer.

### Freezing

The freeze methods consume the builder and produce an immutable
smart pointer. The `Rc` / `Box` / `RcStr` / `BoxStr` freezes —
those that produce **`Local`-flavor** smart pointers — point into
the same `LocalChunk` the builder was using and are O(1) (no
copy, no allocation):

1. Optionally **shrink in place** if the buffer's tail still sits
   at `current_local.data_ptr`.
2. For `T: Drop` `Vec` `Rc`-freezes, install one slice
   `DropEntry` at the back of the buffer's chunk (with
   `drop_fn = drop_shim_slice::<T>` and `len = builder.len`).
3. For `String` / `Utf16String`, write `len` into the inline
   length prefix.
4. Suppress the builder's `Drop` (`mem::forget`) and return the
   smart pointer.

The `Arc` / `ArcStr` freezes produce `Shared`-flavor smart
pointers from a builder living in a `LocalChunk`. They allocate a
fresh region in the arena's `current_shared` chunk via the same
forward bump path as `alloc_arc` allocations, **memcopy** the
builder's `len` elements/bytes into the new shared region,
install the appropriate drop-list entry / length prefix, and
construct the `Arc`/`ArcStr`. `Arc` freezes are therefore
**O(n)** in the builder's length — the deliberate trade-off for
keeping builders single-threaded. Users who care about avoiding
the copy build directly with `alloc_arc` / `alloc_str_arc` /
etc. when they know the size up front.

### Why builders are `Local`-flavor

A `String` / `Utf16String` / `Vec` builder is a transient,
single-threaded object held by one piece of code while it grows.
There is no value in making it `Send` or atomic; the user pays
nothing for cross-thread bookkeeping during the build phase.

## Validation: loom and miri

- **Loom** (`tests/loom_arc.rs` and the `cfg(loom)` paths in
  `internal/sync.rs`) model-checks the atomic operations on
  `SharedChunk::refcount`, the `shared_cache_head` Treiber CAS,
  the shared high-water mark, and the cross-thread `Arc::clone` /
  `Arc::drop` interleavings against the deferred-reconciliation
  scheme. The provider's `Drop` is included so chunk-stranding
  scenarios on the no-shared-cache paths are covered.

- **Miri** is run in three configurations under CI's nightly
  workflow: default, `-Zmiri-strict-provenance`, and
  `-Zmiri-tree-borrows`. Strict-provenance is the binding
  configuration — the recent provenance refactor (using
  `byte_add` / `byte_sub` on the original chunk pointers rather
  than int-to-ptr casts) was driven by this checker.

## Implementation notes

- **Bookkeeping allocation on `Global`.** A single per-arena
  control allocation — the `Arc<ChunkProvider<A>>` held by the
  arena — currently uses the global allocator regardless of the
  user's `A`. Bulk allocations (chunk storage, builder buffers,
  oversized chunks) do go through `A`. One small one-shot
  allocation per arena lifetime; not on the hot path. Routing it
  through `A` requires a hand-rolled refcounted control block
  (stable `std::sync::Arc` does not support custom allocators),
  so it's deferred. Documented on `Arena::new_in`.

- **`ArenaBuilder` knobs.**

  - `max_normal_alloc(bytes)` → routing threshold above which an
    allocation that can't fit in the current chunk is given a
    one-shot oversized chunk sized exactly to the request.
    Default `MAX_NORMAL_ALLOC = 16 KiB`; bounds
    `[MIN_MAX_NORMAL_ALLOC = 4 KiB, max_bump_extent::<A>()]`
    (slightly under 64 KiB; the upper bound depends on the
    per-chunk-type header size).

  - `with_capacity_local(bytes)` / `with_capacity_shared(bytes)`
    → preallocate enough local / shared chunks up front to
    cover `bytes` bytes of total chunk allocation (header +
    payload, not user-payload alone). Each knob picks the
    smallest size class whose total is ≥ `bytes` (capped at the
    largest class, 64 KiB), allocates as many chunks of that
    class as needed to cover `bytes`, pushes them into the
    matching cache, and seeds the matching high-water mark to
    that class. Each `bytes` must be `0` (no preallocation; the
    default) or `>= MIN_CHUNK_BYTES = 512`.

  - `byte_budget(bytes)` → optional total-byte ceiling on chunks
    the arena has outstanding (live + cached). Enforced at
    allocation time via `total_chunk_bytes`. Default:
    unbounded.

- **`Allocator` impl for `&Arena`.** `allocate(layout)` returns a
  pointer into a chunk; `deallocate(ptr, layout)` masks the
  pointer to find the chunk and decrements the +1 it holds.

- **`ArenaStats` (under the `stats` feature).** Per-flavor split
  for chunk allocation counts:

  - `normal_local_chunks_allocated`, `oversized_local_chunks_allocated`,
  - `normal_shared_chunks_allocated`, `oversized_shared_chunks_allocated`,
  - `total_bytes_allocated` (sum of user `Layout::size`s),
  - `wasted_tail_bytes` (slack at chunk retirement),
  - `relocations` (builder regrowths that copied).

  Counters are stored as `Cell<u64>` in `StatsStorage` and
  updated only on cold paths. The hot path is `cfg`-gated to a
  no-op when the feature is off.

- **Cross-`reset` survival.** `Box<T>::drop` runs `T::drop` and
  decrements the chunk refcount via a `ReleaseGuard`. If a `Box`
  outlives an `Arena::reset`, the chunk stays alive via the
  box's +1 and `reset` doesn't try to replay the box's drop
  entry (boxes don't install one). What `reset` *does* drop is
  any `Rc`/`Arc`/simple-ref allocations recorded in the
  trailing drop list of pinned chunks.
