# Plurality — Architecture

This document describes the architecture of the pool: the model it presents, the
patterns that make it fast and safe, and the invariants that hold it together. It
is intentionally implementation-agnostic — for the concrete API see the
crate-level rustdoc, and for forward-looking ideas see [`TODO.md`](./TODO.md).

## What plurality is

Plurality is a **growable, fixed-slot object pool**. It front-loads memory in
coarse chunks and then serves individual objects out of those chunks, so the
steady-state cost of allocating and freeing an object is a handful of pointer
operations rather than a round trip through the global allocator.

It occupies a deliberate niche between three neighbours:

- Unlike a **bump/arena** allocator, individual objects can be freed
  independently and their space reused, without waiting for the whole region to
  be discarded.
- Unlike **slab/slotmap** containers, callers receive real smart pointers that
  dereference to the value, not integer keys or indices they must carry around
  and re-resolve.
- Unlike the **global allocator**, objects are drawn from a small, contiguous,
  cache-friendly working set, and the fast path takes no global lock.

Two properties are guaranteed for the lifetime of every handle:

- **Address stability** — a value never moves once allocated. Its address stays
  valid until the handle that owns it is dropped.
- **Detachable lifetime** — the owning handles may outlive the pool object
  itself. The backing memory persists until the last handle is gone.

## The handle model

The pool's public surface is a family of **smart-pointer handles**, not a
container you index into. Allocation hands back a handle; dropping the handle
runs the value's destructor and returns its slot to the pool. There are four
flavours, spanning two axes — *owned vs. shared* and *pool-bound vs. detachable*:

| Handle  | Ownership | Lifetime            | Thread mobility            | Relative cost      |
|---------|-----------|---------------------|----------------------------|--------------------|
| Bound owner | unique | tied to the pool | single-threaded            | cheapest           |
| Detached owner | unique | may outlive pool | movable across threads¹ | one pool-level step |
| Shared (atomic) | shared | may outlive pool | shareable across threads¹ | atomic refcount     |
| Shared (local)  | shared | may outlive pool | single-threaded            | plain refcount      |

¹ subject to the usual `Send`/`Sync` bounds on the contained value and allocator.

The design rationale behind the split:

- The **bound owner** trades reach for speed. Because a borrow statically proves
  the pool outlives the handle, it can skip the bookkeeping that keeps pool
  memory alive — making it the cheapest handle — at the price of being neither
  detachable nor thread-mobile.
- The **detached owner** is the general-purpose unique pointer: it may be stored
  `'static` and moved between threads, paying one extra pool-level step on
  allocate and free to keep the pool memory alive behind it.
- The two **shared** handles differ only in their reference-count discipline.
  The atomic one is safe to share across threads; the local one uses cheaper
  non-atomic counting and is confined to a single thread. They are otherwise
  interchangeable.

Unique handles expose mutable access to the value; shared handles are read-only,
except when uniqueness-checked mutable access proves that only one shared owner
remains. All four dereference to the value and support comparison, hashing, and
formatting so they substitute cleanly for the standard smart pointers. Pinning
depends on the ownership form rather than being uniform across all four.

### Rust pinning model

Pool slots are address-stable, but address stability by itself is not enough to
make every handle a sound pinned owner. The owner must also keep the slot
occupied for the full duration of the pinning guarantee, even if that owner is
forgotten.

The **bound owner is therefore not pinnable**. It relies on its borrow of the
pool rather than independently retaining pool storage. Forgetting it ends that
borrow without returning the slot, after which dropping the pool could reclaim
the backing memory. A pinning guarantee cannot depend on the forgotten handle's
destructor running.

The detachable owners provide pinning according to their ownership discipline:

- A unique detached owner may be converted into a pin. It independently keeps
  the pool alive, so forgetting it leaks the slot and its pool claim rather than
  permitting reuse.
- Atomic and local shared owners may be pinned only while freshly constructed,
  before an ordinary alias can escape. Converting an existing shared owner
  would be unsafe because another ordinary alias might later become unique and
  gain mutable access to a `!Unpin` value.
- A pinned shared owner may be unsized while remaining pinned. Unsizing changes
  pointer metadata, not the allocation or the value's address, and never
  exposes an ordinary owner.

Uniqueness-checked mutable access on ordinary shared owners is compatible with
this model precisely because pinned shared construction prevents ordinary
owners from coexisting with the pinned family.

Shared uninitialized owners do not support a pin-then-initialize transition.
The uninitialized wrapper is movable, which would make it possible for an
ordinary alias to escape before the initialized value acquired its pinning
guarantee. A pinned shared value is instead constructed complete and pinned
before it becomes observable.

Closure-based constructors are not emplacement protocols. A closure produces
an ordinary value, which is then moved into its final slot; pinning is
established only after that move. The closure therefore cannot form
self-references to the eventual slot.

### Thin handles and type erasure

A handle to a sized value is exactly **one pointer wide** — the same footprint as
a raw reference. This is a core design constraint: the pool adds no per-handle
metadata that the caller has to carry.

The owning and shared handles can also hold **unsized** values — trait objects
and slices. In that form they carry the usual pointer metadata (a vtable or a
length) exactly like the standard library's smart pointers, while the value
itself stays put in its pool slot. Conversion from a sized handle to an unsized
one is a **compiler-checked coercion**: the caller supplies a token proving the
target unsizing is legal, so erasure cannot be requested for an invalid target.
On drop, an unsized handle reclaims its slot using only the value's runtime size
and alignment — it never needs to know the original concrete type.

## Concurrency model

The pool follows a **single-producer / multi-consumer** discipline, and this
single decision shapes the whole design.

```text
        ┌──────────────────────────────────────────────┐
        │            one allocator thread              │
        │   (holds the pool; grows it; hands out slots)│
        └───────────────┬──────────────────────────────┘
                        │ allocate
                        ▼
                 ┌─────────────┐   free (many threads)
                 │  free list  │ ◄───────────────┬───────────┐
                 └─────────────┘                 │           │
                        ▲                     ┌───┴───┐   ┌───┴───┐
                        │ pop                 │ drop  │   │ drop  │  …
                        │                     └───────┘   └───────┘
```

- **Allocation is single-threaded.** Growing the pool and popping free slots
  happen on exactly one thread at a time. The pool object can be *moved* between
  threads, but only one thread ever holds it, so these operations are
  uncontended and need no locking among themselves.
- **Frees are concurrent.** The owning and shared handles are thread-mobile, so
  many threads may drop handles — and thus return slots — simultaneously.

The consequence is an asymmetric design: one thread pushes new capacity and pops
slots without contention, while any number of threads concurrently return slots.
Only the hand-off point between them needs synchronization, and it is expressed
entirely with atomics — **there is no mutex anywhere in the pool**. State touched
only by the single allocator thread (notably the directory of chunks) needs no
synchronization at all; its confinement to that one thread is itself the
soundness argument.

## Memory layout

Memory is acquired in **chunks** — power-of-two-sized batches of slots. Each
chunk is a single allocation laid out as a small header followed by its array of
slots:

```text
 chunk:  ┌────────┬─────┬────────┬────────┬─────┬──────────┐
         │ header │ pad │ slot 0 │ slot 1 │ ... │ slot N-1 │
         └────────┴─────┴────────┴────────┴─────┴──────────┘
              │                 ▲
              │ back-reference  │ each slot: value + refcount + in-chunk index
              ▼                 (fixed stride, so addressing is pure arithmetic)
        shared pool state
```

Two properties of this layout are load-bearing:

- **Chunks never move and are never individually freed.** They live until the
  entire pool tears down. That means a chunk header can hold a plain
  back-reference to the shared pool state with no risk of a dangling pointer and
  no reference cycle.
- **Slot addressing is arithmetic, not lookup.** Because chunk size is a power of
  two and slot stride is fixed, mapping a global slot index to chunk-and-offset
  is shift/mask arithmetic, and stepping from a value's address back to its
  chunk header is fixed-offset arithmetic. No per-object bookkeeping table is
  consulted on the hot path.

### The slot and its dual-purpose counter

Each slot holds three things: storage for the value, a small counter, and its
own immutable index within the chunk. The counter is **contextual** — it means
different things depending on whether the slot is occupied or free:

- **Occupied:** it is the value's reference count (how many shared handles point
  at it).
- **Free:** it is a link — the index of the next free slot in the free list.

These two roles never collide because an occupied slot is only ever read as a
count (by live handles) and a free slot is only ever read as a link (by the free
list). The slot's stored in-chunk index is what makes single-pointer recovery
possible: from a bare value pointer, the pool can find the index, step back to
the chunk header, and from there reach the shared pool state — all without the
handle carrying any extra data.

## Reclamation without back-pointers

Because a sized handle is just a value pointer, freeing it requires
reconstructing everything else from that pointer alone. This **pointer-recovery**
pattern is the architectural heart of the crate:

```text
 value pointer
      │  read the slot's in-chunk index and counter (fixed offsets past the value)
      ▼
 step back to slot 0, then to the chunk header (fixed-stride arithmetic)
      │
      ▼
 chunk header ──► shared pool state (free list, pool-level refcount, teardown hook)
```

A crucial architectural choice makes this safe across type erasure: the shared
pool state that recovery reaches is a **type-agnostic core** — it contains only
what reclamation needs (the free-list head, the pool-level reference count, and a
type-restoring teardown hook). Recovery therefore never has to guess the concrete
value type. The exact original type is restored only by the teardown hook, and
only once the pool is truly finished. This is what lets an erased trait-object
handle return its slot correctly even though its concrete type was forgotten at
the type level.

## The free list

Free slots are threaded together into a **lock-free stack** whose links live
inside the slots themselves (reusing the dual-purpose counter). This is the
concurrency hand-off point:

- **Popping** a slot happens only on the single allocator thread, so there is
  exactly one consumer. This eliminates the classic ABA hazard by construction —
  a free slot is never simultaneously popped by two threads or re-pushed while
  still free.
- **Pushing** a freed slot can happen on any thread. Producers race only on the
  head of the stack, resolved with a compare-and-swap retry loop.

There is **no growth lock**: adding a chunk, extending the directory, and
splicing the new slots onto the free list all run on the sole allocator thread,
racing only against concurrent producer pushes at the head.

Growth is a **cold, rare path**. When the free list is empty, the allocator
reserves one slot from a freshly acquired chunk for the immediate request and
splices the remainder onto the free list in one step. Handing back the reserved
slot directly — rather than looping back to re-pop — keeps the grow-then-allocate
path bounded, with no window where a lost race could re-empty the list.

## Two reference counts, two lifetimes

The pool tracks **two independent reference counts** governing two different
resources:

- A **per-slot count** governs a single value: how many shared handles point at
  it. When it reaches zero, the value's destructor runs and the slot returns to
  the free list.
- A **pool-level count** governs the pool's memory as a whole — every chunk plus
  the shared state. Each detachable handle holds one unit of it, which is exactly
  what allows handles to outlive the pool object.

The interplay yields a clean teardown story:

```text
 build ................. pool-level count = 1  (the pool object holds it)
 allocate detachable ... +1 pool-level     (bound owner does NOT take one)
 share (clone) ......... +1 per-slot only
 drop handle ........... -1 per-slot; at zero: run destructor, return slot,
                          then -1 pool-level (bound owner skips the pool step)
 drop pool object ...... -1 pool-level
 pool-level hits 0 ..... free all chunks and shared state
```

Because every detachable allocation holds a unit of the pool-level count, by the
time that count hits zero there are provably **no occupied slots left**.
Teardown therefore never runs a value's destructor — every value was already
destroyed on its own handle's drop, exactly once. Dropping the pool object is not
synchronous with respect to outstanding handles: it merely relinquishes the pool
object's own claim, and the backing memory survives until the last handle
departs.

Teardown may run on whatever thread happens to drop the last handle, which need
not be the allocator thread. This is sound because a zero pool-level count
implies the pool object is gone (no more allocation or growth can occur) and no
handles remain, so all shared structures are quiescent. The atomic release/acquire
discipline on the counts and on the published chunk directory guarantees the
teardown thread observes a complete, frozen set of chunks to reclaim.

## Allocation surface and failure

Each handle flavour offers the same shape of allocation entry points:

- a **by-value** form for convenience,
- a **closure-based** form that defers value construction until a slot is
  available, and
- an **uninitialized-then-initialize** form, the guaranteed zero-copy path,
  mirroring the standard library's `new_uninit` idioms.

Every form has an infallible variant that panics when the pool cannot satisfy the
request, and a **fallible** sibling that reports the failure instead. A pool
"fails" for one of two architecturally distinct reasons, and the error
distinguishes them:

- **Capacity exhausted** — a configured chunk cap (or the intrinsic index
  ceiling of an unbounded pool) is reached and no slot is free.
- **Allocator failure** — acquiring a new chunk from the underlying allocator
  failed.

On failure the rejected value is dropped and no construction closure is invoked,
matching the standard fallible-allocation convention.

## `no_std` and allocator integration

The pool depends only on `alloc` — no `std`, and no operating-system
synchronization primitives. This is feasible precisely because of the concurrency
model: allocation and growth are single-threaded, and the free list is a
lock-free stack, so only plain atomics are required. Chunk acquisition goes
through the standard allocator abstraction, so custom and instrumented allocators
compose naturally.

## Design invariants at a glance

The safety and correctness of the whole system rest on a short list of
invariants:

1. **Single allocator thread.** At most one thread at a time grows the pool or
   pops slots, and the directory of chunks is confined to that thread. This is a
   "no concurrent allocation" rule, not a thread-affinity rule: the pool may be
   moved to and resumed on a different thread, so long as allocations never
   overlap in time.
2. **Chunks are immortal until teardown.** They never move and are never freed
   individually, so back-references from chunks to pool state can never dangle.
3. **The slot counter is context-typed.** Occupied slots read it as a count,
   free slots as a link; the two never overlap in time.
4. **Recovery is arithmetic and type-agnostic.** A value pointer reconstructs its
   slot, chunk, and pool state by fixed offsets, reaching only a type-erased core.
5. **Two counts, two lifetimes.** The per-slot count owns the value; the
   pool-level count owns the memory. Every detachable handle holds one unit of
   the latter, so teardown finds no live values.
6. **A value is destroyed exactly once**, on its own handle's final drop, never
   during pool teardown.
7. **Pinning follows retained ownership.** Bound owners are not pinnable;
   unique detached owners retain their slots independently, and shared pinning
   is established only during fresh construction before an ordinary alias can
   escape.

## Verification strategy

The architecture is validated by a layered suite of complementary techniques,
each targeting a different failure class:

- **Functional tests** exercise the full handle surface, panic paths, and
  behaviour under custom, failing, and counting allocators, plus contention
  stress.
- **Undefined-behaviour and data-race checking** validates the pointer-recovery
  arithmetic and the non-atomic shared-handle path.
- **Exhaustive interleaving exploration** covers the concurrent free path:
  multiple shared handles on one slot, cross-thread frees, and teardown running
  on a non-allocator thread — confirming each value is destroyed exactly once.
- **Property and fuzz testing** probes pool invariants under randomized
  operation sequences.
- **Coverage and mutation testing** guard against untested paths and assertions
  that do not actually constrain behaviour.
- **Instruction-exact and wall-clock benchmarks** run identical operation bodies
  so the hot paths are measured consistently, including cross-crate and
  macro-benchmark comparisons against the system allocator.
