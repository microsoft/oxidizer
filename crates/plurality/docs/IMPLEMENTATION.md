# Plurality — Implementation

This document describes how the architecture in [`DESIGN.md`](./DESIGN.md) is
realised internally. It covers the parts that are not user-visible: how the two
pool forms share one body of machinery, how slot geometry is computed and kept
consistent, how the layout router works, and how the whole thing is measured
and verified. For the public contract see the crate-level rustdoc; for
forward-looking ideas see [`TODO.md`](./TODO.md).

## Layering

Both pool forms are façades over one implementation. What separates them is a
single question — *is the slot geometry known at compile time?* — and that
question is answered by a type parameter rather than by duplicated code.

```text
   Pool<T, A>                              BlindPool<A>
   (public, typed)                         (public, blind)
        │                                       │
        │                                  layout directory
        │                                       │
        │                                  LayoutPool<A>
        │                                  (crate-private)
        ▼                                       ▼
   PoolInner<TypedGeometry<T>, A>          PoolInner<RuntimeGeometry, A>
        └──────────────────┬────────────────────┘
                           ▼
                       PoolCore
        (free-list head · pool refcount · teardown hook)
```

`PoolCore` is unchanged: three fields, no generics, reached by pointer recovery
from any value pointer. Everything above it is generic over the geometry
provider, and the two providers differ only in whether their answers are
compile-time constants or loaded fields.

## Slot geometry

### One formula, two consumers

A slot is a value followed by a `u32` reference count and a `u32` in-chunk
index, laid out as `#[repr(C)]`. Every offset in the pool follows from the
value's size and alignment alone:

```text
cell_align   = max(align, align_of::<u32>())
refcount_off = round_up(size, align_of::<AtomicU32>())
index_off    = round_up(refcount_off + size_of::<AtomicU32>(), align_of::<u32>())
stride       = round_up(index_off + size_of::<u32>(), cell_align)
slots_off    = round_up(size_of::<ChunkHeader>(), cell_align)
chunk_align  = max(align_of::<ChunkHeader>(), cell_align)
chunk_bytes  = pad_to_align(slots_off + stride * slot_count, chunk_align)
```

These formulas have two independent consumers that must agree exactly:

- the **pool**, when it lays out a chunk, addresses a slot, and initialises
  slot metadata during growth;
- the **handle**, when it walks from a value pointer back to the slot, the
  chunk header, and the pool core.

The handle cannot ask the pool, because finding the pool is the whole point of
the walk. Agreement is therefore structural, not negotiated — both sides
evaluate the same formulas over the same inputs. Housing those formulas in one
place is what makes that guarantee auditable, and it is what makes a
layout-parameterised pool possible at all: the reclamation half of the pool has
never depended on the element type, only on its layout.

Two standard-library guarantees carry the chain from `Layout::new::<T>()` to
the slot's first field, and both are load-bearing: `UnsafeCell<T>` is
`#[repr(transparent)]` over `T`, and `MaybeUninit<T>` has the same size,
alignment and ABI as `T`. Together they mean the slot's value field has exactly
`T`'s layout, so routing on `Layout::new::<T>()` and laying out a slot from
that layout describe the same bytes.

### The geometry provider

```rust
pub(crate) trait SlotGeometry: Copy {
    fn value_size(self) -> usize;
    fn value_align(self) -> usize;
    fn stride(self) -> usize;
    fn refcount_offset(self) -> usize;
    fn index_offset(self) -> usize;
    fn slots_offset(self) -> usize;
    fn chunk_layout(self, slots: usize) -> Option<Layout>;
}
```

`TypedGeometry<T>` is zero-sized. Its methods return `size_of::<T>()`,
`align_of::<T>()` and the formulas above, so every geometry expression on the
typed path folds to the same constants the crate emits today. Storing it in
`PoolInner` costs nothing.

`RuntimeGeometry` is a small `Copy` struct holding the precomputed offsets. It
is built once, when a layout pool is constructed, and stored in `PoolInner`, so
the hot path loads values rather than recomputing them. Its fields sit
alongside `chunk_size`, `shift` and `mask`, which the allocation path already
loads, so the extra reads land on lines that are already warm.

It is built with `Layout::extend` and `pad_to_align` rather than with
hand-rolled arithmetic. `Layout::extend` *is* the `repr(C)` field-placement
algorithm, taken from `core`, so extending the value layout by the counter and
then by the index yields the offsets and the stride by construction. This
matters because a hand-rolled runtime formula would have no ground truth to be
checked against, whereas this one is checked against `core` by definition. The
`const` typed path keeps its own arithmetic because `Layout::extend` is not
usable in `const` context, and the two are cross-checked against each other for
every layout that both paths see.

The free path uses neither. It derives its own geometry from the value's
runtime size and alignment, which is exactly what the erased reclamation path
does today for unsized handles. That asymmetry is deliberate and load-bearing:
reclamation must not depend on reaching the pool before it knows where the pool
is.

### Proving the formulas

The typed geometry is cross-checked against the compiler's own layout of the
slot type: a `const` block asserts that each computed offset equals the
corresponding `offset_of!` on the slot struct, and that the computed stride and
alignment equal the slot struct's `size_of` and `align_of`. Every element type
the crate is instantiated with therefore re-verifies the formula against ground
truth, and a divergence is a build error rather than a corrupted free list.

The check must be *forced* from a path every instantiation reaches — an
associated `const` is only evaluated where it is used, so the geometry
accessors reference it. Placing it behind the accessors rather than behind a
dedicated entry point means no instantiation can route around it. These are
post-monomorphization errors, so the diagnostic is poor and the check cannot be
tested negatively; that is acceptable for an assertion whose only job is to
fail a build that would otherwise ship a corrupted free list.

This check carries more weight after the refactor than before it. Today the two
consumers of the geometry are *independently derived*: growth and slot
addressing use the compiler's layout of the slot struct, while the erased free
path uses the hand-rolled formula, so any divergence between them is caught by
every existing handle test. Once both sides evaluate the same formula, a bug in
it becomes self-consistent and functionally invisible. Two things follow. The
`const` cross-check becomes the primary guard rather than a redundant one,
which is why it asserts stride and alignment and not just offsets. And the
bound owner's drop path is deliberately left on the compiler's layout (see
*Handles* below), preserving one independent consumer.

The runtime geometry is checked differently, because it has no slot struct to
compare against: it is *built* from `Layout::extend`, which is `core`'s own
`repr(C)` algorithm, and the hand-rolled formula is asserted against it in
debug builds. Between them, neither derivation is trusted alone.

The formulas have been validated against the compiler's layout for the full
spread the pool must handle: zero-sized types, sub-word and odd sizes,
word-sized and double-word types, alignments up to a page, `MaybeUninit<T>`
(which shares its layout with `T`, so it routes to the same layout pool), and
the unsized views produced by coercing to a trait object or to a slice. In
every case the geometry derived from a value's runtime size and alignment
equals the geometry the pool was built with — which is the invariant the whole
blind design rests on.

## The shared pool body

`PoolInner<G, A>` replaces the element type with the geometry provider. The
field set is otherwise unchanged; `chunk_layout` remains a precomputed runtime
`Layout` value, now obtained from `G`.

Three code paths lose their element-type dependence:

- **Slot addressing.** Stepping to a slot and stepping back to a chunk header
  become `base + offset * geometry.stride()` and its inverse, replacing pointer
  arithmetic over a typed slot pointer. For the typed geometry the stride is a
  constant and the emitted code is unchanged.
- **Growth.** Instead of writing a whole slot struct, growth writes the two
  metadata words at their computed offsets and leaves the value storage
  uninitialised. This is what the typed path already does semantically; it just
  stops naming a type to do it.
- **Teardown.** The teardown hook monomorphises over the geometry provider and
  the allocator rather than over the element type. Under `loom`, where teardown
  must drop the instrumented atomic in each slot, it finds that atomic at the
  geometry's reference-count offset. This removes the last element-type
  dependence from teardown, which matters because teardown may run long after
  the pool object is gone.

The free list, the pool-level reference count, the chunk directory, growth
policy and the concurrency discipline are untouched.

## `LayoutPool`

```rust
pub(crate) struct LayoutPool<A: Allocator = Global> {
    inner: NonNull<PoolInner<RuntimeGeometry, A>>,
}
```

One pointer, like the typed pool. It is constructed from a value `Layout` plus
the chunk-sizing configuration, and it is crate-private: the blind pool is its
only user.

Its allocation surface mirrors the typed pool's, with two differences. The
element type is a method parameter, and every entry point is `unsafe` with the
precondition that the element type's layout equals the pool's layout:

```rust
impl<A: Allocator> LayoutPool<A> {
    pub(crate) fn layout(&self) -> Layout;

    /// # Safety
    /// `Layout::new::<T>()` must equal `self.layout()`.
    pub(crate) unsafe fn try_alloc_box_unchecked<T>(&self, value: T)
        -> Result<Box<T, A>, AllocError>;
    // ... one per handle flavour and construction form
}
```

The precondition exists because it is the invariant that keeps allocation and
reclamation in agreement, and because the router establishes it by construction
— it selected this pool *because* the layouts matched. Re-checking it on every
allocation would pay for a fact the caller already proved. A debug assertion
restates the precondition so that violations surface in testing; it documents
intent more than it verifies anything, since the only caller establishes the
precondition structurally.

Each of these methods is a one-line forward to the same crate-private
primitives the typed pool uses to turn a claimed slot into a handle, so the
surface is wide but has no logic of its own.

### Clamping the sizing configuration

The typed pool asserts at build time that its chunk size times its chunk cap
fits the pool's maximum slot count, and it treats chunk-layout overflow as a
panic. Both are reasonable when a human supplied both numbers for one known
element type. Neither is reasonable here: a blind pool derives the chunk size
per layout, so one user-supplied cap meets many derived sizes, and the first
allocation of an unfortunate layout would panic from inside a call that
promised to return a `Result`.

`LayoutPool::new` therefore **clamps rather than asserts**. The derived chunk
size is clamped so that the chunk layout cannot overflow, and the effective
chunk cap is clamped to what the maximum slot count permits at that chunk size.
A blind pool's effective per-layout cap is the smaller of the configured cap
and this ceiling, which is a documented part of its contract.

The overflow check comes from `core` rather than from hand-rolled arithmetic:
extending `Layout::new::<ChunkHeader>()` by the slot array yields the chunk
layout and the slots offset together, and returns an error on overflow instead
of requiring a separate `checked_mul`. The clamp is then driven by that error
rather than by a reimplementation of the same bounds.

## `BlindPool` — the layout router

### State

```rust
pub struct BlindPool<A: Allocator = Global> {
    layouts: UnsafeCell<Vec<Layout>>,
    pools: UnsafeCell<Vec<LayoutPool<A>>>,
    sizing: ChunkSizing,
    max_layouts: usize,
    allocator: A,
}
```

The two vectors are parallel: `layouts[i]` is served by `pools[i]`. Keys are
kept in their own contiguous vector so that a lookup scans keys only, without
striding over pool pointers it does not need. `layouts` is the shorter of the
two at every instant (see *Reentrancy*), so a key is never visible without its
pool.

Unlike the typed pool, the blind pool needs no heap indirection of its own.
Nothing points at it: chunk headers point at their layout pool's core, and
handles point at values. The directory is reachable only from the pool object.

### Lookup

Allocation computes `Layout::new::<T>()` — a compile-time constant at each call
site — and scans the key vector for it. Programs present few distinct layouts,
so a linear scan over a contiguous array is the right shape: it is
branch-predictable, prefetch-friendly, and beats a tree or a hash for the sizes
that actually occur. Entries are held in first-seen order.

Two refinements are available if measurement justifies them: packing each key
into a single word so that more of them fit per cache line, and reordering
entries by use. Neither is adopted speculatively — the benchmark that would
motivate them is described below, and the workspace performance guidance
requires a measured win before adding either.

### Interior mutability and the allocator thread

Allocation takes `&self` and may grow the directory, so the vectors sit behind
`UnsafeCell`. The soundness argument is the one the chunk directory already
uses: the pool is `!Sync`, so allocation never overlaps with itself, and the
directory is touched only while allocating. Reclamation never reaches it.

The discipline that keeps this sound is that no borrow of either vector is held
across *pool* user code — construction closures and destructors. A lookup
copies the layout pool's inner pointer to a local and releases the borrow
before anything else happens. `Vec::try_reserve` and `Vec::push` do hold `&mut`
across a call into the *global* allocator, which is the same exposure the chunk
directory already has in the typed pool's growth path; it is not made worse
here.

### Reentrancy

Allocation runs user code at more points than is immediately obvious. All four
must be accounted for:

- the construction closure;
- the destructor of a value rejected by a failed fallible allocation;
- `A::clone()`, when a new layout pool is being built;
- the global allocator, during vector reservation, pushes, and the metadata
  allocation for a new layout pool.

The last two run *while a new layout pool is being created*, which is precisely
the window in which the directory is inconsistent. The ordering below is chosen
so that no such reentrant call can observe a broken state or force a failure
into an infallible position:

1. Compute the layout and scan the key vector for it.
2. On a miss, check the layout cap and report capacity exhaustion if it is
   reached, then **construct the layout pool**. Construction clones the
   allocator first and allocates the metadata second, so a panic from
   `A::clone` cannot strand a metadata allocation. It is fallible and touches
   no directory state, so a failure here leaves both vectors exactly as they
   were. `A::clone()` and the global allocator run during this step, and a
   reentrant allocation that reaches them sees a consistent — merely
   incomplete — directory.
3. `try_reserve` both vectors. Reserving after construction means the
   reservation cannot be consumed by a reentrant miss that happened during
   step 2.
4. **Re-scan, and re-check the cap.** Step 2 released control twice, so the
   directory may have grown since. Two outcomes abandon the freshly built
   pool: an entry for this layout now exists, or the layout cap is now
   reached. Both are necessary. Without the re-scan, one layout could acquire
   two pools — the entry found by lookup would be arbitrary, the duplicate
   would be dead but owned, and per-layout statistics would report only one of
   them. Without the cap re-check, a reentrant miss on a *different* layout
   passes the same step-2 check as the outer call and both push, so the cap
   overshoots by the depth of reentrant misses and stops being a bound at all.
5. On either abandonment, **return without pushing**. The freshly built pool is
   dropped and the reserved capacity is left unused, which is harmless. Order
   matters here: copy the found entry's inner pointer to a local and release
   every borrow *before* dropping the abandoned pool, because that drop runs
   the layout pool's teardown — the cloned allocator's destructor and a global
   deallocation — and so is another point at which control leaves this code.
6. Otherwise push `pools` first, then `layouts`. Lookups scan `layouts`, so
   this ordering keeps `layouts.len() <= pools.len()` at every instant, and a
   key is never visible before the pool that serves it exists. The reverse
   order leaves a window in which a reentrant lookup — triggered by the global
   allocator during the second push — finds a key whose pool is not yet there
   and indexes out of bounds. Both pushes are into capacity reserved in step 3
   and nothing between step 3 and here can consume it, so neither reallocates
   and neither can fail.
7. Copy the layout pool's inner pointer to a local and drop all borrows.
8. Perform the allocation through that local.

A reentrant allocation may grow and reallocate both vectors during step 8
without disturbing the in-flight operation, because the local points at a
`PoolInner` on the heap that never moves.

`Vec::reserve` aborts the process on failure, so the fallible path uses
`try_reserve` throughout, which also handles the capacity overflow that
`reserve` would panic on. `Vec::push` is only ever called into capacity that
has already been reserved.

The fallible family reports pool failures as `Err`, but it is not panic-free
with respect to user code: a construction closure, a rejected value's
destructor, `A::clone`, `A::drop` and the allocator's own methods may all
panic, and those panics propagate.

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
that borrow is alive. Retiring empty layout pools would buy back only the
metadata of a layout the program has already demonstrated it uses, and would
cost the stable-index property that the rest of the design leans on.

Vector reallocation moves the layout pool *handles* — each one pointer — but
never the heap `PoolInner` they point at. Handles that point into a layout
pool's slots are therefore unaffected by router growth.

Each layout pool needs its own allocator instance, which is why the blind pool
requires a cloneable allocator. Sharing one instance would mean either a
lifetime in the pool's type — which would infect every handle and destroy their
detachability — or an extra reference count on the allocator, paid on a path
that exists only to hand memory to chunks.

## Handles

`Box`, `Arc` and `Rc` are unchanged in representation, in their drop paths, and
in their coercion and pinning surfaces. They already reclaim through the erased
path, which is geometry-derived, so they work with either pool form without
knowing which one produced them. This is the property that keeps a blind
handle one pointer wide.

`Alloc` needs one change, and it is smaller than it first appears. Its drop
path stays exactly as it is. That path is monomorphized over `T` and reads the
slot through the compiler's layout of the slot struct — and for a layout pool
built from `Layout::new::<T>()`, those are by definition the same numbers the
layout pool used. A bound owner from a blind pool therefore works unmodified.
Leaving it alone has two further benefits: it preserves `Alloc`'s invariance in
`T` (moving it to the erased path would require storing a value pointer instead
of a slot pointer, silently widening the handle to covariant in `T`), and it
keeps one consumer of the geometry independently derived, so a formula bug
still fails a test rather than hiding behind its own consistency.

What does change is the phantom field. It currently borrows the typed pool by
reference, which both supplies the lifetime and denies `Send` and `Sync`. It
becomes a lifetime marker plus an explicit non-`Send`, non-`Sync` marker, so
that either pool form can return the handle. A bare `PhantomData<&'pool ()>`
would be `Send` and `Sync` and would quietly make `Alloc` thread-mobile; the
marker must deny both explicitly, and the crate's existing auto-trait
assertions are what catch it if it does not.

## Chunk sizing

A blind pool sizes chunks from a byte target:

```text
slots = clamp(target_bytes / stride, MIN_SLOTS, MAX_SLOTS)
slots = largest power of two not exceeding slots
```

Rounding to a power of two is required, not cosmetic: mapping a global slot
index to a chunk and an offset is shift-and-mask arithmetic on the hot
allocation path. Because the lower clamp bound is itself a power of two,
rounding cannot push the result below it.

The lower bound exists so that even a value large enough to dominate the target
still amortises the growth path over several allocations. The upper bound
exists so that a very large target cannot make the first use of a layout commit
an unreasonable amount of memory. The default target is a small multiple of a
page: large enough that small values grow rarely, small enough that a layout
touched once does not cost much.

A caller may instead fix the slot count, which applies uniformly to every
layout and reproduces the typed pool's sizing behaviour.

## Failure

Chunk memory comes from the pool's allocator and fails through the existing
allocator-failure path. Pool metadata — the `PoolInner` allocation and the
directory vectors — comes from the global allocator.

Metadata allocation becomes fallible internally, but the **typed pool's
behaviour does not change**. A `try_new_inner` returns the metadata allocation
as a `Result`; `PoolBuilder::build()` calls it and routes failure to
`handle_alloc_error`, exactly reproducing today's abort, while `LayoutPool`
consumes the `Result` directly. Turning today's abort into a panic that unwinds
out of `build()` would be a gratuitous contract change, and would need a new
entry in that method's documented panic conditions.

Replacing the boxed `PoolInner` with a raw fallible allocation moves an
obligation that the box was discharging silently. Teardown currently
reconstitutes the box, which runs `PoolInner`'s drop glue: the directory
vector's buffer is freed, the allocator is dropped, and under `loom` the
instrumented atomics in the core are dropped. The raw form must therefore drop
in place before deallocating. Getting this wrong leaks the directory on every
teardown — a leak the existing allocator-tracking tests would not catch,
because they track the pool's allocator, not the global one.

`AllocError`'s allocator-failure variant now covers two sources: a chunk, and
the metadata of a new layout pool. Its documentation and its `Display` text
currently name chunks specifically and must be broadened to say that memory for
the pool's own use could not be obtained. Adding a third variant was considered
and rejected: the caller's recourse is identical in both cases, and a new
variant would be a breaking change to an error type that callers match on.

The cold path is ordered so that failure leaks nothing and reserves nothing
prematurely. The layout pool is constructed first, so a construction failure
leaves both vectors untouched; reservation follows, so a reentrant allocation
during construction cannot consume it; and the pushes come last, into reserved
capacity, so they cannot fail. See *Reentrancy* above for the full ordering and
the reasons each step sits where it does.

## Statistics

Aggregate queries — live count, chunks allocated, and the statistics feature's
counters — sum over the layout pools. That is linear in the number of distinct
layouts, which is small and bounded by the program's type set; the alternative,
maintaining shadow counters in the router, would put writes on the allocation
path to serve an introspection call. The layout count itself is a vector
length.

Per-layout queries compute the layout from the named type and look it up. They
never create a layout pool, so querying an unseen type reports an empty pool.

## Performance

### Cost model

- **Allocate:** the typed pool's cost, plus a scan of the key vector and a load
  of the layout pool's pointer. With one layout present that is a single
  comparison.
- **Free:** identical to the typed pool. The router is not involved, and the
  arithmetic is the same erased path a typed pool's handle already runs.
- **First use of a layout:** cold. One global allocation for the pool metadata,
  two vector pushes, then the ordinary growth path.

### What must not regress

Parameterising the pool body over geometry must leave the typed pool's emitted
code unchanged. `TypedGeometry<T>` is zero-sized and its accessors are
constant-evaluable, so every geometry expression on that path folds to the
constant it replaced. The gate is instruction counts: the typed rows in
[`PERF.md`](./PERF.md) must hold.

The one genuinely new cost on the runtime path is that slot addressing
multiplies by a loaded stride rather than by a constant. The stride shares a
cache line with fields the same code already loads, so the expected cost is one
multiply plus an L1 hit. Where the typed pool's stride is a power of two the
compiler currently emits a shift or folds the scaling into an addressing mode,
so this is a real addition on the runtime path, not merely a relocation of work
the compiler was going to do anyway. It is confined to `LayoutPool`; the typed
path keeps its constant.

## Benchmarks

Blind-pool coverage follows the workspace conventions: identical operation
bodies in the wall-clock and instruction-count harnesses, single-threaded,
measuring elementary operations against a pre-warmed pool with growth and
first-use effects outside the measured region.

The scenarios are:

- **Allocate and free, one layout present** — every handle flavour. This is the
  low case for routing cost and the direct comparison against the typed pool's
  corresponding rows. The gap between the two is the router's price.
- **Allocate and free, many layouts present** — the high case, isolating how
  lookup scales with directory size. The pool is pre-populated with distinct
  layouts and the measured allocation targets one that is not first in the
  scan.
- **Allocate, coerce to a trait object, dispatch, and free** — the row that
  lines up with the existing owning fat-pointer comparison, where the reference
  implementation's blind pools already appear. This is where the architectural
  claim becomes a number.
- **Churn against the cross-crate comparison set** — the blind pool alongside
  the typed pool and the surveyed pool crates.

Because the free path is claimed to be identical to the typed pool's, the
allocate-and-free pairs carry that claim: any divergence in instruction counts
between the typed and blind rows must be attributable to the lookup alone.

Allocation tracking asserts that a warmed blind pool performs no system
allocations in steady state, including across a mix of layouts.

## Verification

The blind pool is exercised by the same layered strategy the typed pool uses,
with the additions that heterogeneity makes necessary.

- **Functional tests** cover heterogeneous mixes, two types sharing one layout,
  zero-sized and over-aligned values, coercion to trait objects from a blind
  pool, handles outliving the pool, per-layout capacity exhaustion, the layout
  cap, the sizing clamps at both ends, allocator failure on both the chunk and
  metadata paths, and panic safety in construction closures.

  The layout spread must be exercised **through `Alloc` specifically**, not
  only through the detachable handles. The bound owner is the sole consumer
  that still reads slots through the compiler's layout of the slot type, so it
  is the only runtime check that the compiler's placement and the geometry
  built from `Layout::extend` agree. Covering the spread only through
  `Box`/`Arc`/`Rc` would exercise the geometry against itself.
- **Reentrancy tests** allocate from, and free into, a blind pool from inside a
  pooled value's destructor and from inside a construction closure. An
  allocator instrumented to re-enter the pool from `A::clone` covers the
  layout-pool construction window, which is where the duplicate re-scan and the
  cap re-check live and which no user-written closure can reach. Global
  allocator reentrancy is out of scope by the documented precondition.
- **Undefined-behaviour checking** runs the pointer-recovery arithmetic over a
  spread of layouts, which is where a divergence between the two geometry
  providers would surface.
- **Interleaving exploration** covers teardown of one layout pool on a
  non-allocator thread after the blind pool is gone, and concurrent frees
  across two layout pools.
- **Property and fuzz testing** drives randomised sequences over a set of
  layouts, asserting that every value is destroyed exactly once and that slots
  are reused only within their own layout.
- **Auto-trait assertions** pin the `Send` and `Sync` behaviour of the pool and
  of handles obtained from it, including the deliberate relaxation that the
  blind pool's `Send`-ness depends only on its allocator, and `Alloc`'s
  continued denial of both after its phantom field changes.
- **Leak assertions** on pool teardown under the global allocator, covering the
  directory vector that `PoolInner`'s drop glue is responsible for.
- **Mutation testing** guards the geometry formulas and the routing decision,
  both of which are arithmetic that a weak test suite would not constrain.

## Delivery staging

The work divides into stages that are individually reviewable, with the first
three behaviour-preserving and gated on unchanged instruction counts.

1. **Extract the geometry.** Introduce the geometry trait and the typed
   provider, express the erased reclamation path in terms of the shared
   formulas, and add the compile-time cross-checks against the slot type's
   actual layout. No API change.
2. **Parameterise the pool body.** Replace the element type in the pool's
   internal state with the geometry provider, add the runtime provider built
   from `Layout::extend`, and make growth, slot addressing and teardown
   geometry-driven. The bound owner's drop path is deliberately left on the
   compiler's layout. No API change.
3. **Add `LayoutPool`.** The runtime-geometry façade with its unchecked
   surface, the sizing clamps, and fallible metadata allocation behind a
   `try_new_inner` that the typed builder routes to `handle_alloc_error`.
   Teardown must drop `PoolInner` in place before deallocating it. Still no
   public API change.
4. **Add `BlindPool`.** The router, the builder, the public surface, the
   broadened `AllocError` documentation, and the test suite.
5. **Measure.** Benchmarks, performance-report wiring, and regenerated
   numbers.
