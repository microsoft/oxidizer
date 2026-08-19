# Plurality — Handles

The handles are the pool's entire public surface: how a value is owned, how
long it lives, how it is reached from a bare pointer, and how it is returned.
Part of the [architecture](../DESIGN.md).

## The handle model

The pool's public surface is a family of **smart-pointer handles**, not a
container you index into. Allocation hands back a handle; dropping the handle
runs the value's destructor and returns its slot to the pool. The same four
handles serve both the typed pool and the [multi pool](./multi-pool.md). There
are four flavors, spanning two axes — *owned vs. shared* and *pool-bound vs.
detachable*:

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

Unique handles expose mutable access when their pinning rules permit it:
`Alloc` always does, while `Box` does so for `Unpin` values. Shared handles are
read-only, except when uniqueness-checked mutable access proves that only one
shared owner remains. All four dereference to the value and support comparison,
hashing, and formatting so they substitute cleanly for the standard smart
pointers. Pinning depends on the ownership form rather than being uniform
across all four. The entry points that produce them are described in
[allocation](./allocation.md), and their representation in
[`implementation/handles.md`](../implementation/handles.md).

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

The slot and chunk structures this walk traverses are described in
[memory](./memory.md).

A crucial architectural choice makes this safe across type erasure: the shared
pool state that recovery reaches is a **type-agnostic core** — it contains only
what reclamation needs (the free-list head, the pool-level reference count, and a
pool-metadata teardown hook). Recovery therefore never has to guess the
concrete value type. Value destruction is driven by the handle's own type or
fat-pointer metadata before reclamation; the teardown hook restores only the
concrete pool metadata type, and only once the pool is truly finished. This is
what lets an erased trait-object handle return its slot correctly even though
its concrete type was forgotten at the type level.

## Two reference counts, two lifetimes

The pool tracks **two independent reference counts** governing two different
resources:

- A **per-slot count** governs a single value: how many shared handles point at
  it. When it reaches zero, the value's destructor runs and the slot returns to
  the free list. It is bounded by the same ceiling the standard library's `Arc`
  uses, and exceeding that ceiling aborts the process.
- A **pool-level count** governs the pool's memory as a whole — every chunk plus
  the shared state. Each detachable allocation holds one unit of it, which is
  exactly what allows handles to outlive the pool object.

The interplay yields a clean teardown story:

```text
 build ................. pool-level count = 1  (the pool object holds it)
 allocate detachable ... +1 pool-level     (bound owner does NOT take one)
 share (clone) ......... +1 per-slot only
 drop shared handle .... -1 per-slot; at zero: run destructor, return slot,
                         then -1 pool-level
 drop unique handle .... run destructor, return slot, then -1 pool-level
 drop bound owner ...... run destructor and return slot without pool-level work
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

Forgetting a handle instead of dropping it is sound, and its slot stays occupied
for as long as the pool's memory exists. A detachable handle also keeps its unit
of the pool-level count, so the pool's memory outlives the forgotten handle
along with the slot.

Teardown may run on whatever thread happens to drop the last handle, which need
not be the allocator thread. This is sound because a zero pool-level count
implies the pool object is gone (no more allocation or growth can occur) and no
handles remain, so all shared structures are quiescent. The atomic release/acquire
discipline on the counts and on the published chunk directory guarantees the
teardown thread observes a complete, frozen set of chunks to reclaim. The
threading rules that make this well defined are described in
[concurrency](./concurrency.md).
