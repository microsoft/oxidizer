# The blind pool

This document covers the two pieces that turn the shared pool body into a pool
of any type: the crate-private `LayoutPool`, which serves one runtime layout,
and `BlindPool`, the directory that routes each allocation to one. Back to the
[implementation hub](../IMPLEMENTATION.md). For the user-visible model see
[the blind pool's design](../design/blind-pool.md).

## `LayoutPool`

```rust
pub(crate) struct LayoutPool<A: Allocator> {
    inner: NonNull<PoolInner<A, RuntimeGeometry>>,
}
```

One pointer, like the typed pool. It is constructed from a value `Layout` plus
the chunk-sizing configuration, and it is crate-private: the blind pool is its
only user.

It owns the pool it points at: dropping it releases the pool's reference, so
outstanding handles keep the body alive exactly as they do for a typed pool.

The wide allocation surface lives on `BlindPool`, not here. A layout pool
exposes one allocation primitive — claim a slot — and the blind pool layers the
handle flavors and construction forms over it. Putting the ~40 methods in one
place keeps a single set of doc comments and a single translation of "claimed
slot" into "handle", which is the only logic those methods contain.

That primitive is reached through a **non-owning view**:

```rust
#[derive(Clone, Copy)]
pub(crate) struct LayoutPoolRef<A: Allocator> {
    inner: NonNull<PoolInner<A, RuntimeGeometry>>,
}

impl<A: Allocator> LayoutPool<A> {
    pub(crate) fn as_ref(&self) -> LayoutPoolRef<A>;
}

impl<A: Allocator> LayoutPoolRef<A> {
    pub(crate) fn core(&self) -> &PoolCore;

    /// # Safety
    /// `Layout::new::<T>()` must equal the pool's layout.
    pub(crate) unsafe fn alloc_slot<T>(self) -> Result<NonNull<SlotCell<T>>, AllocError>;
}
```

The view exists so that the router can drop its borrow of the directory before
running any user code. It carries no lifetime and copies freely, which is what
lets the allocation path hold nothing borrowed while a value's constructor —
which may itself allocate into the same blind pool — runs.

`alloc_slot` is `unsafe` with the precondition that the element type's layout
equals the pool's layout. The precondition exists because it is the invariant
that keeps allocation and reclamation in agreement, and because the router
establishes it by construction — it selected this pool *because* the layouts
matched. Re-checking it on every allocation would pay for a fact the caller
already proved. A debug assertion restates the precondition so that violations
surface in testing; it documents intent more than it verifies anything, since
the only caller establishes the precondition structurally.

### Clamping the sizing configuration

The typed pool asserts at construction that its chunk size times its chunk cap
fits the pool's maximum slot count, and treats chunk-layout overflow as a
panic. Both are reasonable when a human supplied both numbers for one known
element type. Neither is reasonable for a layout pool: a blind pool derives the
chunk size per layout, so one user-supplied cap meets many derived sizes, and
the first allocation of an unfortunate layout would panic from inside a call
that promised to return a `Result`.

`LayoutPool::new` therefore **clamps rather than asserts**. The derived chunk
size is clamped so that the chunk layout cannot overflow, and the effective
chunk cap is clamped to what the maximum slot count permits at that chunk size.
The resulting effective values are part of the blind pool's contract and are
exposed through its per-layout queries; see
[the blind pool's design](../design/blind-pool.md).

The clamp is driven by the chunk-layout computation itself rather than by a
reimplementation of the same bounds: `chunk_layout` reports overflow as `None`
— its slot array is sized by checked multiplication, since `Layout::repeat` is
unstable — and the clamp simply halves the slot count until that call succeeds.
The clamp therefore cannot disagree with the layout computation it is
protecting. A one-slot chunk is the floor, and it is representable for every
layout `Layout::new::<T>()` can produce.

## The router

### State

```rust
pub struct BlindPool<A: Allocator + Clone = Global> {
    layouts: UnsafeCell<Vec<Layout>>,
    pools: UnsafeCell<Vec<LayoutPool<A>>>,
    sizing: ChunkSizing,
    max_chunks: Option<u32>,
    max_layouts: Option<usize>,
    allocator: A,
}
```

Each layout pool owns a clone of the allocator, which is why `A: Clone` is a
bound of the pool rather than of individual methods.

The two vectors are parallel: `layouts[i]` is served by `pools[i]`. Keys are
kept in their own contiguous vector so that a lookup scans keys only, without
striding over pool pointers it does not need. `layouts` is the shorter of the
two at every instant (see [Reentrancy](#reentrancy)), so a key is never visible
without its pool.

The blind pool needs no heap indirection of its own. Nothing points at it:
chunk headers point at their layout pool's core, and handles point at values.
The directory is reachable only from the pool object.

### Lookup

Allocation computes `Layout::new::<T>()` — a compile-time constant at each call
site — and scans the key vector for it. Programs present few distinct layouts,
so a linear scan over a contiguous array is the right shape: it is
branch-predictable, prefetch-friendly, and beats a tree or a hash for the sizes
that actually occur. Entries are held in first-seen order.

A hit is the only thing the allocation path pays for. Creating and installing a
layout pool is outlined behind `#[cold]` and takes the layout as a value rather
than a type parameter, so it is emitted once per allocator instead of once per
element type and never occupies registers on the path that finds its pool.

Refinements are available if measurement justifies them: packing each key
into a single word so that more of them fit per cache line, and reordering
entries by use. Neither is adopted speculatively — the benchmark that would
motivate them is in [performance](./performance.md), and the workspace
performance guidance requires a measured win before adding either.

### Interior mutability and the allocator thread

Allocation takes `&self` and may grow the directory, so the vectors sit behind
`UnsafeCell`. The soundness argument is the one the chunk directory uses: the
pool is `!Sync`, so allocation never overlaps with itself, and the directory is
touched only while allocating. Reclamation never reaches it.

The discipline that keeps this sound is that no borrow of either vector is held
across *pool* user code — construction closures and destructors. A lookup
copies the layout pool's inner pointer to a local and releases the borrow
before anything else happens. `Vec::try_reserve` holds `&mut` across a call
into the *global* allocator, which is the same exposure the chunk directory has
in the typed pool's growth path. `Vec::push` runs only into capacity that was
already reserved.

### Reentrancy

Allocation runs code outside this module at more points than is immediately
obvious. All of them must be accounted for:

- the construction closure;
- the destructor of a value rejected by a failed fallible allocation;
- `A::clone()`, when a new layout pool is being built;
- the global allocator, during vector reservation and the metadata
  allocation for a new layout pool;
- the pool's own allocator, `A::allocate` on the growth path and
  `A::deallocate` at teardown.

The last three run *while a new layout pool is being created or grown*, which
is precisely the window in which pool state is inconsistent. Two of them are
handled by ordering, and one by a precondition.

**The pool's allocator must not allocate from, or free into, a plurality
pool.** Growth reads the chunk count, derives the new chunk's base index from
it, calls `A::allocate`, and publishes the incremented count only after the
chunk is installed. An allocator that re-entered the pool during that call
would read the same chunk count and derive the same base index, so two chunks
would claim one range of global slot indices. This matches the global
allocator's precondition recorded among the [design
invariants](../DESIGN.md#design-invariants-at-a-glance), and it is a
precondition rather than a defended-against condition because defending it
would put a publication protocol on the growth path to serve an allocator no
caller has reason to write.

The ordering below is chosen so that no other reentrant call can observe a
broken state or force a failure into an infallible position:

1. Compute the layout and scan the key vector for it. On a hit, copy the found
   entry's inner pointer to a local, release the borrow, and go to step 8.
2. On a miss, check the layout cap. If it is reached, release both borrows
   *before* returning, because returning drops the rejected value and its
   destructor is reentrant code; then report capacity exhaustion.
3. **Construct the layout pool.** Construction clones the allocator first and
   allocates the metadata second, so a panic from `A::clone` cannot strand a
   metadata allocation. It is fallible and touches no directory state, so a
   failure here leaves both vectors exactly as they were. `A::clone()` and the
   global allocator run during this step, and a reentrant allocation that
   reaches them sees a consistent — merely incomplete — directory.
4. `try_reserve` both vectors. Reserving after construction means the
   reservation cannot be consumed by a reentrant miss that happened during
   step 3.
5. **Re-scan, and re-check the cap.** Step 3 released control twice, so the
   directory may have grown since. The freshly built pool is abandoned if an
   entry for this layout exists, or if the layout cap is reached. Both checks
   are necessary. Without the re-scan, one layout could acquire two pools —
   the entry found by lookup would be arbitrary, the duplicate would be dead
   but owned, and per-layout statistics would report only one of them. Without
   the cap re-check, a reentrant miss on a *different* layout passes the same
   step-2 check as the outer call and both push, so the cap overshoots by the
   depth of reentrant misses and stops being a bound at all.
6. On either abandonment, **return without pushing**. The freshly built pool is
   dropped and the reserved capacity is left unused, which is harmless. Order
   matters here: copy the found entry's inner pointer to a local and release
   every borrow *before* dropping the abandoned pool, and, on the cap branch,
   before dropping the rejected value. Both drops are reentrant code — the
   abandoned pool's teardown runs the cloned allocator's destructor and a
   global deallocation, and the rejected value's destructor is arbitrary user
   code.
7. Otherwise push `pools` first, then `layouts`. Lookups scan `layouts`, so
   this ordering keeps `layouts.len() <= pools.len()` at every instant, and a
   key is never visible before the pool that serves it exists. The reverse
   order leaves a window in which a reentrant lookup — triggered by the global
   allocator during the second push — finds a key whose pool is not there and
   indexes out of bounds. Both pushes are into capacity reserved in step 4 and
   nothing between step 4 and here can consume it, so neither reallocates and
   neither can fail.
8. Copy the layout pool's inner pointer to a local, drop all borrows, and
   perform the allocation through that local.

A reentrant allocation may grow and reallocate both vectors during step 8
without disturbing the in-flight operation, because the local points at a
`PoolInner` on the heap that never moves.

`Vec::reserve` aborts the process on failure, so the fallible path uses
`try_reserve` throughout, which also handles the capacity overflow that
`reserve` would panic on. `Vec::push` is only ever called into capacity that
has already been reserved.

The fallible family reports pool failures as `Err`, and it is not panic-free
with respect to code outside this module: a construction closure, a rejected
value's destructor, `A::clone`, `A::drop` and the allocator's own methods may
all panic, and those panics propagate.

### Ownership

The blind pool holds one unit of each layout pool's pool-level reference count,
exactly as a typed pool object holds one unit of its own. Dropping the blind
pool drops the pool vector, releasing one unit from each layout pool; those
with outstanding detachable handles survive until their last handle departs.
The key vector is plain data.

**Entries are never removed.** A layout pool, once created, lives until the
blind pool is dropped, even when every value of that layout has been freed.
This is what makes indices stable, makes the parallel vectors safe to grow
under a shared reference, and — most importantly — makes the `Alloc` borrow
argument hold: an `Alloc` borrows the blind pool, the blind pool owns the
vector, and nothing can retire the layout pool the `Alloc` points into while
that borrow is alive.

The memory consequence is worth stating plainly, because it is larger than the
directory entry it saves. Retiring an empty layout pool would drop its last
pool-level reference and run its teardown, which deallocates **every chunk it
holds** — for a large layout pinned at the minimum slot count, a substantial
amount of memory held for a type the program touched once. Retention is
therefore monotonic per layout: a blind pool's memory only grows until it is
dropped. That is the same policy the typed pool follows, which never returns
chunk memory either; a blind pool makes the policy more visible because it
accumulates layouts a program may use briefly. Bounding this is the job of the
two caps rather than of retirement, and an aggregate byte budget is recorded as
an extension in [`TODO.md`](../TODO.md).

Vector reallocation moves the layout pool *handles* — each one pointer — but
never the heap `PoolInner` they point at. Handles that point into a layout
pool's slots are therefore unaffected by router growth.

Each layout pool needs its own allocator instance, which is why the blind pool
requires a cloneable allocator. Sharing one instance would mean either a
lifetime in the pool's type — which would infect every handle and destroy their
detachability — or an extra reference count on the allocator, paid on a path
that exists only to hand memory to chunks.

### Statistics

Aggregate queries — live count, chunks allocated, and the statistics feature's
counters — sum over the layout pools. That is linear in the number of distinct
layouts, which is small and bounded by the program's type set; the alternative,
maintaining shadow counters in the router, would put writes on the allocation
path to serve an introspection call. The layout count itself is a vector
length.

Per-layout queries compute the layout from the named type and look it up. They
never create a layout pool, so querying an unseen type reports an empty pool.

### Failure

Chunk memory comes from the pool's allocator and fails through the shared
allocator-failure path. Pool metadata — the `PoolInner` allocation and the
directory vectors — comes from the global allocator, and every step of the cold
path that touches it is fallible, so a blind pool reports metadata failure
rather than aborting. The disposition of both failures is described in
[the pool body](./pool-body.md#failure).

The cold path is ordered so that failure leaks nothing and reserves nothing
prematurely: the layout pool is constructed first, so a construction failure
leaves both vectors untouched; reservation follows, so a reentrant allocation
during construction cannot consume it; and the pushes come last, into reserved
capacity, so they cannot fail.
