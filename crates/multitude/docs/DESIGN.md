# Multitude Architecture Overview

This document describes how `multitude` is put together and *why* it
behaves the way it does. It is a conceptual companion to the public-API
rustdoc (which documents *what* each method does) and to
[`PERF.md`](PERF.md) (which reports measured cost). The focus here is on
the runtime model: the moving parts, how they interact, the invariants
that keep them sound, and the edge cases and failure modes that fall out
of the design.

## Table of contents

- [The problem being solved](#the-problem-being-solved)
- [The four allocation styles](#the-four-allocation-styles)
- [Architecture at a glance](#architecture-at-a-glance)
- [Chunk lifecycle](#chunk-lifecycle)
- [Reference counting without hot-path atomics](#reference-counting-without-hot-path-atomics)
- [Thin smart pointers: the alignment/masking trick](#thin-smart-pointers-the-alignmentmasking-trick)
- [Growable collections and zero-copy freeze](#growable-collections-and-zero-copy-freeze)
- [Concurrency model](#concurrency-model)
- [Configuration and tuning](#configuration-and-tuning)
- [Failure modes and edge cases](#failure-modes-and-edge-cases)
- [Safety invariants](#safety-invariants)

## The problem being solved

`multitude` is a bump allocator for **phase-oriented workloads** —
request handlers, parsers, compiler passes — where many allocations are
born together, live together, and die together. The win comes from two
places: allocation is a cursor bump (near free), and reclamation is a
single bulk operation instead of *N* individual frees.

The classic downside of a bump allocator is that it can *only* reclaim in
bulk: no individual object can be freed early, and nothing it hands out
may outlive the arena. `multitude`'s central design goal is to relax both
of those limits without taxing the common fast path:

- Individual chunks can be **reclaimed early** — as soon as the last
  handle referencing them drops — instead of waiting for arena teardown.
- Some handles (the smart pointers) can **outlive the arena** entirely,
  keeping just their backing chunk alive.
- Every allocated value has its **destructor run** at the right moment,
  automatically.

Everything below is machinery in service of those three properties.

## The four allocation styles

The defining architectural choice is that a single arena, and even a
single chunk, simultaneously supports four ways of owning a value. They
all bump-allocate from the same storage, all deref to the value, and all
run `T::drop` **eagerly** when the owning handle drops. They differ only
in ownership, escape capability, and what per-handle bookkeeping they pay:

| Handle | Ownership | Can outlive arena | Per-handle refcount | Cross-thread |
|---|---|---|---|---|
| `Alloc<'a, T>` | unique, `&mut` | no (bound to `&arena`) | none | move only |
| `Box<T>` | unique, `&mut` | yes | none | move only |
| `Rc<T>` | shared (`Clone`) | yes | non-atomic `u32` | no (`!Send`) |
| `Arc<T>` | shared (`Clone`) | yes | atomic `u32` | yes (`T: Send+Sync`) |

The key distinction that drives the whole chunk-lifecycle design is
**whether a handle carries its own refcount**:

- `Arc`/`Rc`/`Box` each take exactly **one** refcount on their hosting
  chunk at allocation. Because the chunk is kept alive by that count, the
  handle may escape the arena, and the chunk may reclaim early once its
  last handle drops.
- `Alloc<T>` carries **no** refcount. Its lifetime is tied to `&Arena` by
  the borrow checker, and the arena is responsible for keeping the
  backing storage alive for that borrow. This is the cheapest style —
  pure bump, no atomic, no prefix — at the cost of not being able to
  escape.

This split is why there is **no deferred-drop list**: because every
handle finalizes its own value when it drops, a chunk never has to
remember which slots still need a destructor at reset. Reset becomes a
pure cursor rewind.

## Architecture at a glance

Three collaborating types do the work, wired together by one deliberately
constrained chunk layout:

```text
                 ┌────────────────────────┐
                 │     ChunkProvider      │   factory + MPSC chunk cache
                 │  (one per Arena, Arc)  │   (Treiber freelist + size floor)
                 └───────────┬────────────┘
                     ▲       │ hands out fresh/cached chunks
              Weak   │       │
        (back-ref)   │       ▼
                 ┌───┴────────────────────┐
                 │        Arena           │   Send when A: Send + Sync; !Sync
                 │  ┌──────────────────┐  │
                 │  │  current chunk   │──┼──► bump-allocated here (hot path)
                 │  │  (ChunkMutator)  │  │
                 │  └──────────────────┘  │
                 │  retired-local list ───┼──► intrusive list of pinned chunks
                 └────────────────────────┘
                            │ allocations produce
        ┌───────────────────┼────────────────────┐
        ▼                   ▼                     ▼
   Alloc<'a,T>         Arc/Rc/Box            Vec/String
   (no refcount,       (one chunk            (grows in place;
    arena-bound)        refcount each)        freezes into Box/Arc)
```

- **`Arena`** is a thin façade: it owns one *current* chunk (via a
  `ChunkMutator` that holds the bump cursor), an intrusive list of
  *retired* pinned chunks, and a strong reference to its provider. It is
  the only public handle to the allocator.
- **`ChunkProvider`** is the factory and cache for chunks. Each arena owns
  exactly one; it is never shared between arenas. Chunks hold a `Weak`
  back-reference to it so an escaped smart pointer can return its chunk to
  the cache (or free it) even after the arena is gone.
- **`Chunk`** is a DST with an `[UnsafeCell<u8>]` payload tail. It holds
  a shared handle to the backing allocator, a refcount, and one intrusive `next`
  link (reused for either the retired list or the cache freelist, since a
  chunk is never on both at once). It does **not** carry the bump cursor —
  that lives transiently in whichever `ChunkMutator` owns it. Keeping the
  allocator behind a shared handle also keeps the header size independent of
  `A`, which is required by first-tile smart-pointer recovery.

The payload is `[UnsafeCell<u8>]` rather than `[u8]` for two reasons:
interior mutability lets multiple derived writers mutate through a shared
`&Chunk`, and carrying the whole payload as a fat pointer preserves
pointer provenance across the entire allocation region under Stacked/Tree
Borrows.

## Chunk lifecycle

A chunk moves through a small state machine. The transition that matters
most — what happens when the current chunk fills or the arena resets —
depends entirely on whether the chunk ever handed out an arena-lifetime
(`Alloc`) handle.

```text
        acquire (fresh or from cache)
                  │
                  ▼
            ┌───────────┐   fills up / reset
            │  CURRENT  │──────────────┐
            │ (mutating)│              │
            └───────────┘              ▼
                              reconcile surplus, then:
                    ┌───────────────────┴───────────────────┐
        handed out an Alloc?                         smart-pointer-only?
                    │ yes                                    │ no
                    ▼                                        ▼
            ┌───────────────┐                        last handle live?
            │  PINNED       │                       ┌────────┴────────┐
            │ (retired list,│                    yes│                 │no
            │  freed at     │                       ▼                 ▼
            │  reset/drop)  │                 stays alive         reclaim now
            └───────┬───────┘                 until handle       (→ cache or free)
                    │ reset / arena drop        drops
                    ▼                             │
              → cache or free  ◄──────────────────┘
```

**Pinned chunks.** If a chunk handed out any `Alloc` handle (including the
`str`/`[T]`/`MaybeUninit` forms and growable-collection buffers), it must
stay alive until reset: those handles hold no refcount and are bounded
only by the `&Arena` borrow, so freeing the chunk could dangle a live
reference. Such chunks are threaded onto an intrusive singly linked
*retired list* and released in bulk at `reset`/`drop`.

**Early-reclaim chunks.** A chunk that handed out *only* smart pointers is
dropped from the current slot immediately on refill. It stays alive purely
through its handles' refcounts, so it reclaims the moment its last
`Arc`/`Rc`/`Box` drops — possibly long before the arena is reset, possibly
after the arena is gone.

**Mixed chunks pay the pin.** A chunk that served *both* an `Alloc` and a
smart pointer is pinned until reset even after its `Arc`s drop. This is the
deliberate, acknowledged cost of letting one current chunk serve both
styles; the arena tracks a single "did this chunk hand out a reference?"
flag so that only genuinely mixed chunks pay it.

**Reset is a cursor rewind.** `Arena::reset` takes `&mut self`, which
statically guarantees no `Alloc` (which borrows `&self`) is live. It runs
**no** destructors — every `Alloc` already ran its own on drop, and
smart-pointer values remain owned by their still-live handles. It simply
reconciles the current chunk's refcount surplus (below) and returns chunk
bytes to the cache (or leaves chunks alive if escaped handles still hold
them).

**Oversized allocations bypass this entirely.** A request larger than the
configured `max_normal_alloc` gets a one-shot chunk sized exactly to it,
filled through a stack-local mutator, and *never installed as current* —
so small allocations keep flowing into the existing active chunk. An
oversized reference chunk is pinned; an oversized smart-pointer chunk
reclaims with its handle.

## Reference counting without hot-path atomics

Naively, every smart-pointer allocation would bump the chunk's atomic
refcount — an atomic on the hot path. `multitude` avoids this with a
**pre-credited surplus** scheme:

1. When a chunk becomes current, its atomic count is pre-credited with a
   large surplus (2³⁰).
2. Each smart-pointer handout increments a **non-atomic** per-arena
   counter instead of the atomic; `Alloc` handouts touch neither.
3. When the chunk is retired (refill, reset, or drop), the surplus is
   reconciled with a **single** atomic subtraction, leaving the chunk's
   count equal to exactly the number of handles that actually escaped.

The 2³⁰ surplus is far larger than any plausible number of concurrent
`Arc::drop`s on other threads, so it cannot underflow before
reconciliation.

There are then **two** independent counts in play:

- **The chunk refcount** (atomic) tracks how many *families* of handles
  keep the chunk alive. The entire `Arc`/`Rc`/`Box` family for one value
  takes exactly **one** chunk refcount at allocation and releases it when
  the last member drops.
- **The per-handle strong count** tracks clones of a single shared value.
  `Arc::clone`/`Rc::clone` bump *only* this count (a relaxed atomic
  increment for `Arc`, a plain non-atomic increment for `Rc`); they never
  touch the chunk count.

On the last-clone drop, the value's destructor runs in place
(`drop_in_place`, which natively handles `?Sized`), and *then* the
family's single chunk refcount is released. The chunk refcount is adopted
*before* the value drop runs, so even a panicking destructor still
releases the chunk.

Because destructors run eagerly on the last clone rather than being
deferred to chunk teardown, nested arena handles — e.g. `Arc<[Arc<T>]>`
whose inner and outer values share a chunk — release promptly instead of
forming a self-pinning cycle.

## Thin smart pointers: the alignment/masking trick

Every escape-capable smart pointer — `Arc<T>`, `Rc<T>`, `Box<T>` for
*any* `T` including DSTs — is a **single 8-byte raw pointer** on 64-bit,
even for `str` and `[T]`. This rests on one geometric invariant:

> **Every chunk allocation is 64 KiB-aligned** (`CHUNK_ALIGN = 65 536`).

Given that, any value pointer can recover its owning chunk's header by
simply **masking off the low 16 bits** — no type tag, no back-pointer
stored per value. `Box::drop`, `Arc::drop`, and `Rc::drop` all recover
`*const Chunk` the same way.

DST metadata (slice length, vtable) and, for `Arc`/`Rc`, the strong count,
live in a small **prefix** in the chunk immediately before the value
payload, read/written unaligned:

```text
Arc/Rc value:  [strong count][pad][T::Metadata (unaligned)][ T payload ]
                                                            ▲ the 8-byte pointer
Box value:                        [T::Metadata (unaligned)][ T payload ]
Sized T:       metadata is (), so there is no prefix overhead
```

Consequences of the masking scheme, each a real edge case:

- **Maximum smart-pointer alignment is 32 KiB** (`CHUNK_ALIGN / 2`). A
  request above that can never be guaranteed to lie inside the first 64 KiB
  tile, so it is rejected — `try_alloc_*` returns `AllocError`, `alloc_*`
  panics.
- **Oversized chunks** are still 64 KiB-aligned and place their single
  value at the payload start, inside the first tile, so the same mask works.
- **End-of-chunk ZST guard.** A zero-sized allocation landing exactly at
  `chunk_base + CHUNK_ALIGN` would mask to the *next* chunk. The bump
  cursor therefore always advances by at least one byte per reservation,
  routing such a case through refill rather than returning a boundary
  pointer.

The alignment is enforced at allocation time via the `Layout`, not via
`repr(align)` on the chunk struct — keeping the struct's structural
alignment small so `size_of_val` matches the real allocation even for the
smallest size classes.

`Rc` reuses *all* of this — thin pointer, header masking, metadata prefix,
the family's single chunk refcount, eager last-drop teardown — with two
differences: its strong count is a plain non-atomic `u32` (sound because
`Rc` is `!Send`/`!Sync`), and because that count needs no natural
alignment it drops the 4-byte reservation floor, packing sub-4-aligned
payloads (`str`, `[u8]`) a few bytes tighter. Because `Rc` imposes no
`Send`/`Sync` bound on `T`, it can own thread-affine values (e.g.
`Rc<RefCell<T>>`) that `Arc` cannot.

## Growable collections and zero-copy freeze

`Vec<T>`, `String`, and `Utf16String` are **transient builders**:
small (~32-byte) mutable handles over an arena buffer, meant to be built
up briefly and then *frozen* into an immutable smart pointer.

While live, a growable buffer pins its chunk through the same
reference machinery as `Alloc` (no chunk refcount), so the hot push/grow
path is a plain bump with no atomics. When it can't grow in place it
**relocates** to a larger buffer (counted by `ArenaStats::relocations`);
the abandoned buffer is dead space reclaimed at reset.

The headline feature is **zero-copy freeze**. Every freezable buffer
reserves the full `Arc<[T]>` freeze prefix (`[strong][len]`) in front of
its payload at allocation time — which is exactly the `Arc<[T]>` layout
(and a superset of `Box<[T]>`'s). Freezing into `Arc<[T]>`/`Box<[T]>`
then:

1. recovers the hosting chunk by the 64 KiB mask;
2. adopts the family's chunk refcount (from the pre-credited surplus if
   the buffer is still in the current chunk, otherwise a plain atomic bump);
3. writes the final length into the reserved slot; and
4. `mem::forget`s the builder, transferring element ownership to the smart
   pointer.

No allocation, no element copy. The freeze also returns unused tail
capacity to the arena when it can.

**When zero-copy doesn't apply**, freeze falls back to an O(*n*) copy:

- `Box<str>`/`Arc<str>` from a `String` always copy — the byte layout must
  be compacted to keep the result a single `Send`-safe pointer.
- ZSTs and over-aligned `T` (≥ 32 KiB) can't host the prefix, so their
  buffers never reserve it and freeze by copying.
- A zero-copy `split_off` tail whose base points mid-chunk has no prefix
  and copies.
- `Vec::leak` → `&mut [T]` is O(1) and allocation-free for `T: !Drop`
  (reinterpret in place), but the result does **not** outlive the arena.

A one-bit `freeze_prefix` flag on each buffer records which path applies.

## Concurrency model

`Arena<A>` is **`Send` when `A: Send + Sync`, but always `!Sync`**. The
whole arena — with its in-flight `Alloc` handles and smart pointers — can
move between threads when its backing allocator permits it, but it cannot
be *shared*; cross-thread sharing is done by allocating an `Arc` and cloning
it across threads. The `Sync` bound on `A` is required because chunks share
one allocator instance and may be reclaimed concurrently on other threads.
`Arena`'s own `!Sync` is structural (the current chunk, the per-arena cells,
and the retired list are all `!Sync`), and it is load bearing:

The chunk cache is a single intrusive **Treiber-style freelist**, but the
`!Sync` arena makes it **multi-producer / single-consumer**. *Pushes*
(returning a chunk to the cache) happen from any thread that drops the last
handle on a chunk; *pops* happen only from the arena's owning thread. MPSC
sidesteps Treiber's classic hazards for free: no rival consumer can free
the head between our load and CAS (no use-after-free), and the head's
identity can't recycle behind our back (no ABA). A monotonic size-class
*floor* rides alongside the freelist; when it advances, below-floor chunks
still on the list are walked and destroyed in one pass.

`Rc` opts out of all of this: being `!Send`/`!Sync`, its non-atomic count
never crosses a thread boundary, which is exactly what makes the
non-atomic count sound.

## Configuration and tuning

`ArenaBuilder` exposes the tuning knobs; all have defaults that reproduce
`Arena::new()` exactly.

- **`max_normal_alloc`** (default 16 KiB, range `[4096, chunk-max]`) — the
  payload-size threshold above which a request bypasses the cache and gets
  a one-shot oversized chunk. Out-of-range values panic at build with the
  resolved bounds.
- **`byte_budget`** (default unbounded) — a cap on total outstanding chunk
  bytes (live + cached), enforced by a CAS loop; hitting it surfaces as an
  allocator failure.
- **`with_capacity`** — preallocates chunk bytes up front and seeds the
  size-class ratchet, so a warm-up preallocation is consumed by the first
  refill rather than being re-grown from the smallest class.

Two adaptive behaviors run without configuration:

- **Size-class ratchet.** Cacheable chunks come in eight power-of-two
  total sizes (512 B … 64 KiB). Each successful refill bumps a per-arena
  "next class" hint toward the largest class, so a long-lived arena stops
  paying the "always smallest class" refill tax.
- **Chunk cache floor.** The provider only caches up to the current floor
  class and destroys stragglers below it, bounding cache footprint as the
  working set's typical chunk size grows.

With the `stats` feature, `Arena::stats` returns a zero-cost snapshot of
lifetime counters (chunks allocated, relocations) and live gauges (bytes
held, wasted tail bytes) for observability.

## Failure modes and edge cases

**Allocation failure** is reported by `AllocError`, which distinguishes
three mutually exclusive kinds so callers can react appropriately:

| Kind | Meaning | Retryable? |
|---|---|---|
| allocator failure | backing allocator returned null, or `byte_budget` exhausted | maybe (free memory / raise budget) |
| alignment too large | requested alignment > 32 KiB smart-pointer cap | never — request is inherently unsatisfiable |
| capacity overflow | layout arithmetic wrapped `usize` or exceeded `isize::MAX` | never |

Every allocation comes in two flavors: `try_alloc_*` returns
`Result<_, AllocError>`; `alloc_*` panics on the same conditions. Choose
`try_*` on paths that must degrade gracefully.

**Refcount overflow aborts.** If a chunk refcount or an `Arc::clone`
strong count would wrap to zero, the process aborts (`std::process::abort`,
or a forced double-panic under `no_std`). This mirrors `std::sync::Arc`: a
wraparound would race live pointers against a free, and termination is the
only sound response. The abort helper is `#[cold]`/`#[inline(never)]` so
the hot path stays small.

**Panic safety.** Smart-pointer construction takes a protective `+1` guard
on the chunk *before* invoking the user's initialization closure; on
unwind the guard's `Drop` releases the `+1`, and on success ownership of
the `+1` transfers into the finished pointer. For arena-lifetime
allocations, a closure that panics before initializing leaves the reserved
slot untouched and constructs no `Alloc`, so no destructor runs on
uninitialized memory; slice initializers additionally drop any
already-initialized prefix on unwind. The net guarantee: a panicking
closure leaks no refcount and queues no `drop` on uninitialized memory.

**`mem::forget` is always sound**, but skips finalization — forgetting an
`Alloc` never runs its value's destructor; forgetting a smart pointer
leaks its refcount (and thus pins its chunk).

**Escaped handles outliving the arena** is a supported, not exceptional,
case: a chunk holds a `Weak` back-reference to its provider and a shared
allocator handle, so when its last handle drops after the arena is gone it
either returns itself to a still-living cache or frees its own backing
memory directly.

## Safety invariants

The design rests on a handful of invariants; violating any one would be
unsound, so they are maintained centrally rather than at each call site:

- **64 KiB chunk alignment** — the sole basis for header recovery by
  masking. Every chunk allocation, normal or oversized, honors it.
- **Smart-pointer alignment ≤ 32 KiB** — guarantees every value pointer
  lies strictly inside its chunk's first tile, so the mask never walks to
  a neighbor. Enforced at allocation.
- **Non-zero cursor advance** — no reservation returns the one-past-end
  boundary pointer, protecting the ZST edge case.
- **Pin-if-referenced** — any chunk that handed out a refcount-free
  `Alloc` stays alive until `&mut self` reset, so an `Alloc`'s borrow can
  never dangle.
- **Refcount before value drop** — a family's chunk refcount is adopted
  before the value's destructor runs, so a panicking destructor still
  releases the chunk.
- **Prefix counts accessed only as raw reads/writes**, never through a
  reference spanning possibly-uninitialized payload — which keeps the
  scheme sound under Miri.
