# Plurality — Architecture

## What plurality is

Plurality is a **growable, fixed-slot object pool**. It front-loads memory in
coarse chunks and then serves individual objects out of those chunks, so the
steady-state cost of allocating and freeing an object is a handful of pointer
operations rather than a round trip through the global allocator.

It comes in two forms. A **typed pool** fixes its element type at construction
and serves values of that one type. A **multi pool** accepts values of any
type, routing each one to the internal pool that serves its memory layout.
Both hand out the same handles and rest on the same chunk, slot, and free-list
machinery; the multi pool adds a layout directory in front of it.

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

## Areas of the design

The design is documented one area at a time. Each area document is
self-contained; this page carries the shape of the whole and the invariants
that bind the areas together.

**[Handles](./design/handles.md)** — the smart-pointer family that is the
pool's entire public surface: the four flavors and the two axes they span, the
pinning rules that follow from ownership, one-pointer width and coercion to
trait objects and slices, the pointer-recovery walk that lets a bare value
pointer find its way home, and the two reference counts that give a value and
the pool's memory independent lifetimes.

**[Concurrency](./design/concurrency.md)** — the single-producer /
multi-consumer discipline: allocation on one thread at a time, frees from any
number of threads, no mutex anywhere, and the division of labour that leaves
the choice of synchronization primitive to the caller.

**[Memory](./design/memory.md)** — how memory is held and addressed: chunks as
the unit of acquisition, the slot and its context-typed counter, arithmetic
rather than lookup for every address computation, and the lock-free free list
that connects the allocating thread to the freeing ones.

**[Allocation](./design/allocation.md)** — the shape of the allocation entry
points, the split between panicking and fallible variants, the two failure
causes the pool distinguishes, and the `no_std` and custom-allocator story.

**[Multi pool](./design/multi-pool.md)** — pooling values of any type in one
pool object: the geometry directory that sits on the allocation path only,
exact-size routing, byte-target chunk sizing, the caps that bound growth, and
how the multi pool differs from a typed one.

The implementation of this design is documented in
[`IMPLEMENTATION.md`](./IMPLEMENTATION.md), measured performance in
[`PERF.md`](./PERF.md), and settled-but-unbuilt extensions in
[`TODO.md`](./TODO.md).

## Design invariants at a glance

The safety and correctness of the whole system rest on a short list of
invariants:

1. **Single allocator thread.** At most one thread at a time grows the pool or
   pops slots, and the directory of chunks is confined to that thread. This is a
   "no concurrent allocation" rule, not a thread-affinity rule: the pool may be
   moved to and resumed on a different thread, so long as allocations never
   overlap in time. It is enforced statically by the pool being `!Sync`, not by
   caller discipline.
2. **Chunks are immortal until teardown.** They never move and are never freed
   individually, so back-references from chunks to pool state can never dangle.
3. **The slot counter is context-typed.** Occupied shared slots read it as a
   count, unique-owner slots leave it unused, and free slots read it as a link;
   the count and link roles never overlap in time.
4. **Recovery is arithmetic and type-agnostic.** A value pointer reconstructs its
   slot, chunk, and pool state by fixed offsets, reaching only a type-erased core.
5. **Two counts, two lifetimes.** For shared handles, the per-slot count owns
   the value; the pool-level count owns the memory. Every detachable
   allocation holds one unit of the latter, so teardown finds no live values.
6. **A value is destroyed exactly once**, on its own handle's final drop, never
   during pool teardown.
7. **The pool object neither yields nor drops pooled values.** Nothing
   reachable from a shared or exclusive reference to a pool reads, drops, or
   hands back a value the pool holds: every method returns a counter or a
   handle, and teardown reclaims chunk memory without touching value storage.
   This is what licenses a pool being `Send` on the strength of its allocator
   alone, with no bound on the values it serves, and it is a standing
   constraint on the pool's API. A method that yields or drops a pooled value
   through the pool object — iteration, draining, clearing, retaining, cloning
   the pool — is outside the design, as is making the pool `Sync`. A value the
   caller offers and the pool rejects is not a pooled value: it never entered a
   slot and is dropped on the caller's own thread. Thread mobility for pooled
   values belongs to the handles, which carry their own bounds.
8. **Pinning follows retained ownership.** Bound owners are not pinnable;
   unique detached owners retain their slots independently, and shared pinning
   is established only during fresh construction before an ordinary alias can
   escape.
9. **Slot geometry is a pure function of the value's layout.** The allocating
   pool and the reclaiming handle derive the slot stride and the offsets of the
   counter and index from the same formula over size and alignment, which is
   what lets a handle return its slot without consulting the pool it came from.
10. **A pool serves exactly one slot geometry.** A value may only be placed in a
    pool whose geometry equals the geometry derived from the value's own size
    and alignment, since reclamation recomputes it from the value. Routing on
    the derived geometry is the rule that enforces this, and it is exactly as
    strict as necessary: two layouts share a pool precisely when they describe
    the same slot.
11. **The layout directory is pool-object state.** It is read and grown through
    `&self` operations on the pool object — allocation and introspection — and
    is never reachable from a free, a destructor, or a teardown. `!Sync`
    excludes another thread; same-thread allocator reentry is admitted and is
    handled by the ordering in
    [reentrancy](./implementation/reentrancy.md), not excluded.
12. **Layout pools are never retired.** A layout pool created by a multi pool
    lives until the multi pool is dropped, which is what makes directory
    indices stable and lets a bound owner borrow the multi pool while pointing
    into a layout pool's slot.
13. **Reentrancy is safe and supported.** The pools place no obligation on
    the allocator. `Allocator::allocate` and `Allocator::deallocate` may
    allocate from, and free into, the pool they serve; the cold growth paths
    order allocation and publication so reentry observes consistent state.
    `Clone::clone` on a multi pool's allocator may also re-enter while a new
    layout pool is installed and is covered by the same ordering. Pooled
    values' destructors and the closures passed to `_with` constructors run
    with no pool state in flight, so they may allocate from and free into the
    pool freely. An allocator that re-enters unconditionally recurses until the
    stack is exhausted, and the pool does not bound that depth. See
    [allocator reentrancy](./implementation/reentrancy.md).

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
- **Heterogeneous-workload tests** drive many layouts through one multi pool,
  including zero-sized and over-aligned values and values reaching the pool as
  trait objects, confirming that every value is destroyed exactly once and that
  a slot is only ever reused for its own layout.
- **Coverage and mutation testing** guard against untested paths and assertions
  that do not actually constrain behaviour.
- **Instruction-exact and wall-clock benchmarks** run identical operation bodies
  so the hot paths are measured consistently, including cross-crate and
  macro-benchmark comparisons against the system allocator.

The test organisation and tooling that realise this are described in
[`implementation/verification.md`](./implementation/verification.md).
