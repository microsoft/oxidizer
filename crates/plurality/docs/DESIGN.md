# Plurality — Architecture

This document describes the architecture of the pool: the model it presents, the
patterns that make it fast and safe, and the invariants that hold it together. It
is intentionally implementation-agnostic — for the concrete API see the
crate-level rustdoc, for the internals see
[`IMPLEMENTATION.md`](./IMPLEMENTATION.md), and for forward-looking ideas see
[`TODO.md`](./TODO.md).

## What plurality is

Plurality is a **growable, fixed-slot object pool**. It front-loads memory in
coarse chunks and then serves individual objects out of those chunks, so the
steady-state cost of allocating and freeing an object is a handful of pointer
operations rather than a round trip through the global allocator.

It comes in two forms. A **typed pool** fixes its element type at construction
and serves values of that one type. A **blind pool** accepts values of any
type, routing each one to the internal pool that serves its memory layout.
Both hand out the same handles and rest on the same chunk, slot, and free-list
machinery; the blind pool adds a layout directory in front of it.

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
runs the value's destructor and returns its slot to the pool. The same four
handles serve both pool forms. There are four flavours, spanning two axes —
*owned vs. shared* and *pool-bound vs. detachable*:

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

## Heterogeneous pooling — the blind pool

A typed pool fixes its element type at construction. A **blind pool** moves
that decision to the call site: one pool object accepts values of any type, and
the type parameter travels with the allocation instead of with the pool.

```rust
let pool = BlindPool::new();
let widget = pool.alloc_box(Widget::new());    // Box<Widget>
let count  = pool.alloc_arc(0_u64);            // Arc<u64>
let name   = pool.alloc_rc(String::new());     // Rc<String>
```

Everything the typed pool promises about a handle continues to hold: address
stability, detachable lifetime, one-pointer width for sized values, and
coercion to trait objects and slices. A blind pool is what lets a single pool
back a heterogeneous working set — a scheduler holding many distinct future
types, a scene graph of unrelated node types, or any collection stored as
`Box<dyn Trait>` where the concrete types differ.

### The router and the layout pools

A blind pool owns a set of **layout pools**. A layout pool is the same
machinery as a typed pool with the element type replaced by a layout fixed at
construction: same chunks, same slots, same free list, same pair of reference
counts. The blind pool itself holds no slots and no values — it is a directory
that maps a layout to the pool serving it.

```text
   alloc_box(widget)
        │  Layout::new::<Widget>()  →  (size 24, align 8)
        ▼
   ┌───────────────────────────────┐
   │  blind pool: layout directory │
   │   (0, 1)  →  layout pool ●    │
   │   (8, 8)  →  layout pool ●    │
   │  (24, 8)  →  layout pool ●────┼──► chunks · slots · free list
   └───────────────────────────────┘
        │
        ▼
   Box<Widget>  — one pointer, no reference to the directory
```

Types that share a layout share a layout pool: `u64` and `i64` draw from the
same slots, and so do two structs of identical size and alignment. This is
memory sharing, not type confusion — each slot holds exactly one value, and the
handle that owns it carries the concrete type. A consequence worth stating
plainly: capacity reported for one type may be occupied by values of another.

### The router sits on the allocation path only

The architecturally important property of this arrangement is what the *free*
path does not do.

Reclamation is pointer recovery: a handle walks from the value's address to its
slot, to the chunk header, to the type-agnostic pool core, using nothing but
arithmetic derived from the value's own size and alignment. That walk arrives
directly at the layout pool that owns the slot. It never asks which pool the
value came from, so it never consults the directory.

```text
 allocate:  directory lookup ──► layout pool ──► slot
 free:      value pointer ─────────────────────► layout pool   (no lookup)
```

This is why blind handles are byte-for-byte the typed handles — one pointer for
a sized value, with no layout key, no pool reference, and no per-handle
metadata. It is also why freeing costs exactly what it costs in a typed pool,
and why frees remain concurrent and lock-free while the directory stays
confined to the single allocator thread.

### Exact layouts, no size classes

A blind pool routes a value only to the pool whose layout is *exactly*
`Layout::new::<T>()`. It never rounds sizes up, buckets nearby layouts
together, or imposes size classes.

The rule that actually has to hold is narrower: the allocating pool and the
reclaiming handle must agree on **slot geometry**. The reclaiming handle
recomputes stride and offsets from the value it holds, so if a value were
placed in a pool whose stride came from a different geometry, the handle would
walk to the wrong address. Geometry is a pure function of layout, but not an
injective one — several small layouts share a geometry once the counter and
index are accounted for — so bucketing would be sound within a geometry
equivalence class. Routing on the exact layout is simply the cheapest rule that
is always sufficient, and it needs no table of classes to stay correct.

The rule pays for itself: a blind pool has no internal fragmentation from
rounding, and a value occupies exactly the space a typed pool would give it.
What it costs is one layout pool per distinct layout — bounded in practice by
the set of types a program instantiates.

Values of zero size are ordinary participants. Their slots carry only the
counter and the index, which is what a typed pool already does for a
zero-sized element type.

Over-aligned values are ordinary participants too, and pay the same overhead a
typed pool charges them: a slot is padded to the value's alignment, so a value
whose alignment exceeds its size is stored in a slot larger than itself, and a
chunk reserves alignment-sized padding ahead of its first slot. This is
unchanged from the typed pool, but a blind pool is likelier to meet such types,
so it is worth stating.

### Sizing chunks by bytes

A typed pool sizes chunks in **slots**, because it knows how big a slot is. A
blind pool serves layouts spanning several orders of magnitude, so a uniform
slot count would make chunk sizes just as uneven — a few hundred bytes for a
pool of small integers and many megabytes for a pool of large aggregates.

A blind pool therefore sizes chunks by a **byte target**. Each layout pool
derives its own slot count by dividing the target by its slot stride, clamped
to a sensible range and rounded down to a power of two (chunk sizes must remain
powers of two so that slot addressing stays shift-and-mask arithmetic). Small
values get many slots per chunk, large values get few, and every layout commits
a comparable amount of memory per growth step.

A fixed slot count is still available for callers who want the typed pool's
predictability, and applies uniformly to every layout.

### Bounding growth

The chunk cap is **per layout**. Each layout pool independently refuses to grow
past it and reports capacity exhaustion, exactly as a typed pool does.

A per-layout cap alone does not bound the pool, because the number of distinct
layouts is a property of the whole program's type set — including types from
dependencies — and is not something a caller can enumerate. A blind pool
therefore also accepts a **cap on the number of layouts**. Once reached, a
request for an unseen layout reports capacity exhaustion rather than creating a
pool for it.

The two caps together bound the pool's memory as layouts times chunks times the
size of a chunk. That last factor tracks the byte target only for layouts whose
values are small relative to it. A chunk always holds a minimum number of
slots, so a value large enough to exceed the target on its own gets a chunk
sized by the value rather than by the target; and when a caller fixes the slot
count instead, chunk size is the slot count times the stride. The bound is
therefore in chunks, and converting it to bytes requires knowing the largest
layout the program will present.

An aggregate byte budget shared across layout pools would bound memory in bytes
directly, but it requires cross-pool mutable state that outlives every layout
pool. That is a genuine extension rather than a variation, and is recorded in
[`TODO.md`](./TODO.md).

Because the cap is per layout, the *first* allocation of a previously unseen
layout does not compete for capacity with layouts already in the pool: it fails
only against the layout cap, never against another layout's chunk cap.

### Lifetimes and teardown

A blind pool holds one unit of the pool-level reference count on each of its
layout pools, in the same way a typed pool object holds one unit of its own.
Dropping the blind pool drops the directory, releasing one unit from every
layout pool it created. Any layout pool with outstanding detachable handles
survives and tears down when its last handle departs, on whichever thread that
happens to be.

Handles from a blind pool therefore outlive it individually and independently:
two values of different layouts, allocated from the same blind pool, may keep
two separate layout pools alive for different durations.

### Concurrency

The blind pool follows the same single-producer / multi-consumer discipline as
the typed pool, and for the same reason: allocation mutates the directory and
must not overlap with itself, while frees never touch the directory at all. The
pool object may be moved between threads but not shared, and its handles carry
their own thread-mobility rules.

One bound is weaker than the typed pool's. A typed pool is `Send` only when its
element type is, because its type parameter names the values it serves. A blind
pool has no such parameter, and needs none: no pool object ever owns a value.
Every value is owned by a handle, and the pool-level reference count guarantees
that teardown finds no live values. A blind pool is therefore `Send` whenever
its allocator is, and thread mobility for values is governed entirely by the
handles — a handle to a non-`Send` value is itself non-`Send` and stays on its
thread regardless of where the pool goes.

Because each layout pool owns its own clone of the allocator, and because
layout pools tear down independently, two clones of the allocator may be in use
on two threads at once — one tearing down a layout pool whose last handle just
departed, another serving the pool object elsewhere. This is exactly what
`Send` plus `Clone` already licenses for any type, so it imposes no new bound;
it is stated because per-layout cloning makes the situation reachable in a way
a typed pool's single allocator instance never is.

Because reclamation never enters the directory, a destructor running on a
pooled value may freely allocate from, or free into, the same blind pool. There
is no lock to re-enter and no directory borrow held across user code.

### Allocation surface

The blind pool mirrors the typed pool's allocation surface method for method.
Every handle flavour offers the by-value, closure, and uninitialized-then-
initialize forms, each with a panicking and a fallible variant, and the shared
flavours additionally offer their pinned constructors. The only change is where
the type parameter sits:

```rust
typed.alloc_box(value);          // Pool<Widget>  — type fixed by the pool
blind.alloc_box(value);          // BlindPool     — type inferred from the value
blind.alloc_uninit_box::<Widget>();  // named where it cannot be inferred
```

Introspection splits into two tiers, because a blind pool has no single slot
size. Aggregate queries report the whole pool — how many values are live, how
many chunks are held, how many distinct layouts are in play. Per-layout queries
are named for a type and report that type's layout pool: its capacity, its live
count, its derived chunk size. Per-layout queries never create a layout pool,
so asking about a type the pool has not yet seen simply reports an empty pool.

Aggregate queries cost time proportional to the number of layouts, since they
sum over the layout pools. They inherit the typed pool's imprecision under
concurrent frees, and compound it: a sum of independently read counters may
describe a state the pool was never in. They are reporting instruments, not
control-flow inputs.

### How the blind pool differs from a typed pool

| Aspect | Typed pool | Blind pool |
|---|---|---|
| Type parameter | On the pool | On the allocation |
| Handles | Four flavours | The same four, unchanged |
| Chunk sizing | Slot count | Byte target, or a uniform slot count |
| Chunk cap | Per pool | Per layout, plus a cap on layouts |
| Capacity queries | Single tier, constant time | Two tiers; aggregates scale with layouts |
| Allocator | Held once | Cloned per layout, so it must be cloneable |
| `Send` | Requires a `Send` element type | Requires only a `Send` allocator |
| First use of a layout | — | Cold path that allocates pool metadata |

Everything else is common: the handle types and their guarantees, coercion to
unsized values, pinning rules, the uninitialized tiers, the error currency, and
the panic-versus-`Result` split.

Two capabilities are deliberately absent, matching the typed pool. There is no
iteration over pool contents — handles are the only way to reach a value. And
there is no type identity or downcasting: the pool records a layout, not a
type, so a value's concrete type is recovered from its handle or not at all.
Converting a blind pool into a typed pool for one of its layouts is likewise
not offered, because the two use different slot-geometry sources.

### Failure

The blind pool reports the same two failures as the typed pool, and adds no
third.

Capacity exhaustion covers two cases: the layout pool serving the request
cannot grow further, or the request is for an unseen layout and the pool
already holds its maximum number of layouts.

Allocator failure additionally covers the cold path where a previously unseen
layout is encountered and its pool metadata must be allocated. That path is
fallible end to end, so a failure there is reported rather than aborting. The
error therefore means memory could not be obtained for the pool's own use — a
chunk or the metadata of a new layout pool — rather than naming chunks
specifically.

### Comparison with `infinity_pool::BlindPool`

The reference design for this feature is `infinity_pool`'s blind pool family.
The capability sets are close; the structural difference is where the
type-to-pool mapping is consulted.

| | `infinity_pool` | plurality |
|---|---|---|
| Pool types | Three, by lifetime discipline | One |
| Handle types | Two per pool type | Four, shared with the typed pool |
| Handle width (sized value) | Three to five words | One word |
| Free path | Locked directory lookup per drop | Pointer recovery, no lookup |
| Directory | `BTreeMap` behind a mutex or cell | Contiguous scan, allocator thread only |
| Directory synchronization | Mutex held across value destructors | None on the free path |
| Fallible allocation | Not offered | Full `try_` family |
| Zero-sized values | Rejected at run time | Supported |
| Unsizing to trait objects | Per-trait macro, invoked per crate | Coercion token, no per-trait setup |
| Iteration | Not offered | Not offered |
| Type identity / downcasting | Not offered | Not offered |
| Pinning | Every value pinned | Opt-in pinned constructors |

The single-word handle and the lookup-free drop are direct consequences of the
pointer-recovery architecture the typed pool already rests on. Because a handle
can find its own pool, it does not need to carry a key to it, and the directory
never has to be reachable — or lockable — from a destructor.

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
8. **Slot geometry is a pure function of the value's layout.** The allocating
   pool and the reclaiming handle derive the slot stride and the offsets of the
   counter and index from the same formula over size and alignment, which is
   what lets a handle return its slot without consulting the pool it came from.
9. **A pool serves exactly one slot geometry.** A value may only be placed in a
   pool whose geometry equals the geometry derived from the value's own size
   and alignment, since reclamation recomputes it from the value. Routing on
   the exact layout is the rule that enforces this, and it is stricter than
   strictly necessary because distinct layouts can share a geometry.
10. **The layout directory is allocation-path state.** It is read and grown
    only while allocating, on the single allocator thread, and is never
    reachable from a free, a destructor, or a teardown.
11. **Layout pools are never retired.** A layout pool created by a blind pool
    lives until the blind pool is dropped, which is what makes directory
    indices stable and lets a bound owner borrow the blind pool while pointing
    into a layout pool's slot.
12. **The global allocator does not re-enter the pool.** Pool metadata and the
    layout directory come from the global allocator, and the pool's own state
    is mid-update while those calls are outstanding. A global allocator that
    allocated from a plurality pool would observe that state. This is the
    ordinary assumption any interior-mutable container makes; it is stated
    because a blind pool reaches the global allocator at more points than a
    typed pool does. Pooled values' destructors and construction closures are
    subject to no such restriction — they may re-enter freely.

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
- **Heterogeneous-workload tests** drive many layouts through one blind pool,
  including zero-sized and over-aligned values and values reaching the pool as
  trait objects, confirming that every value is destroyed exactly once and that
  a slot is only ever reused for its own layout.
- **Coverage and mutation testing** guard against untested paths and assertions
  that do not actually constrain behaviour.
- **Instruction-exact and wall-clock benchmarks** run identical operation bodies
  so the hot paths are measured consistently, including cross-crate and
  macro-benchmark comparisons against the system allocator.
