# Slot geometry

This document covers the formulas that place a value, its reference count and
its index inside a slot, the provider abstraction that supplies those numbers
at compile time or at run time, and the checks that keep the derivations
honest. Back to the [implementation hub](../IMPLEMENTATION.md).

## One formula, two consumers

A slot is a value followed by a `u32` reference count and a `u32` in-chunk
index, laid out as `#[repr(C)]`. Every offset in the pool follows from the
value's size and alignment alone:

```text
cell_align   = max(align, align_of::<AtomicU32>(), align_of::<u32>())
refcount_off = round_up(size, align_of::<AtomicU32>())
index_off    = round_up(refcount_off + size_of::<AtomicU32>(), align_of::<u32>())
stride       = round_up(index_off + size_of::<u32>(), cell_align)
slots_off    = round_up(size_of::<ChunkHeader>(), cell_align)
chunk_align  = max(align_of::<ChunkHeader>(), cell_align)
chunk_bytes  = pad_to_align(slots_off + stride * slot_count, chunk_align)
```

A slot's alignment must hold both metadata words as well as the value, and
`AtomicU32` is named beside `u32` because a target may align an atomic more
strictly than the plain integer of the same width.

These formulas have two independent consumers that must agree exactly:

- the **pool**, when it lays out a chunk, addresses a slot, and initializes
  slot metadata during growth;
- the **handle**, when it walks from a value pointer back to the slot, the
  chunk header, and the pool core.

The handle cannot ask the pool, because finding the pool is the whole point of
the walk. Agreement is therefore structural, not negotiated — both sides
evaluate the same formulas over the same inputs. Housing those formulas in one
place is what makes that guarantee auditable, and it is what makes a
layout-parameterised pool possible at all: the reclamation half of the pool
does not depend on the element type, only on its layout.

Two standard-library guarantees carry the chain from `Layout::new::<T>()` to
the slot's first field, and both are load-bearing: `UnsafeCell<T>` is
`#[repr(transparent)]` over `T`, and `MaybeUninit<T>` has the same size,
alignment and ABI as `T`. Together they mean the slot's value field has exactly
`T`'s layout, so deriving a slot from `Layout::new::<T>()` and laying out a
slot for a `T` describe the same bytes.

Notice that `slots_off` does not depend on the slot count. Recovery and slot
addressing are therefore pure arithmetic over the geometry and an index, with
no per-chunk state to consult beyond the chunk's base address. Both directions
of that arithmetic, `slot_at` and `header_of`, are methods of the geometry
abstraction in `src/geometry.rs`, so a safety review of slot addressing has a
single place to look; [the pool body](./pool-body.md) covers how the walk is
used.

## The geometry provider

```rust
pub(crate) unsafe trait SlotGeometry: Copy {
    fn stride(self) -> usize;
    fn refcount_offset(self) -> usize;
    fn index_offset(self) -> usize;
    fn slots_offset(self) -> usize;
    fn chunk_layout(self, slots: usize) -> Option<Layout>;

    unsafe fn slot_at(self, chunk: NonNull<ChunkHeader>, offset: usize) -> NonNull<u8>;
    unsafe fn header_of(self, slot: NonNull<u8>, index: u32) -> NonNull<ChunkHeader>;
}
```

The trait is `unsafe`, and both providers are `unsafe impl`s, because
`PoolInner` trusts one geometry value for the whole slot layout at once: it
sizes a chunk allocation with `chunk_layout`, places and addresses the slots
inside that allocation through `slots_offset` and `stride`, initializes and
reads the slot metadata at `refcount_offset` and `index_offset`, and walks back
from a slot to its chunk header with `header_of`. What an implementer promises
is that those answers describe one and the same layout — a chunk allocation
that admits the slots the stride and the slot-array offset describe, metadata
fields in bounds and aligned within each slot, and `header_of` as the exact
inverse of `slot_at` — and that every copy of the value answers alike for as
long as a pool holds it. A caller cannot check that: the numbers are meaningful
only together, and their consistency is exactly what it is asking the geometry
for.

The first five methods answer with numbers. The last two are the addressing
directions built on them, and they are where the two providers differ. Both
have default bodies that multiply the stride, which is what `RuntimeGeometry`
uses. `TypedGeometry<T>` overrides them with typed pointer arithmetic over
`SlotCell<T>`, so the typed path emits an indexed address rather than a
multiply — the reason the typed pool's instruction count is unchanged by having
a provider at all.

`TypedGeometry<T>` is zero-sized. Its methods return the formulas above over
`size_of::<T>()` and `align_of::<T>()`, so every geometry expression on the
typed path folds to a constant. Storing it in `PoolInner` costs nothing.

`RuntimeGeometry` is a small `Copy` struct holding the precomputed offsets. It
is built once, when a layout pool is constructed, and stored in `PoolInner`, so
the hot path loads values rather than recomputing them.

## One derivation, two shapes

Both providers evaluate the same formulas, which is the point: agreement
between a pool addressing a slot and a handle walking back from one is
structural, not negotiated. What differs is *when* they are evaluated.

`TypedGeometry<T>` evaluates them in `const` context over `size_of::<T>()` and
`align_of::<T>()`. `RuntimeGeometry` evaluates them once, at layout pool
construction, over a `Layout` known only at run time, and stores the results.
The erased free path derives a fresh runtime geometry from the value's runtime
size and alignment instead of reading the provider stored in the pool —
reclamation must not depend on reaching the pool before it knows where the pool
is.

The formulas are hand-rolled rather than delegated to `Layout::extend`, which
*is* `core`'s `repr(C)` field-placement algorithm: `extend` is not usable in
`const` context, and its `Result` plumbing costs measurable instructions on the
free path, which runs per deallocation. `extend` is instead used as the
independent oracle the formulas are proven against (see below).

`Layout::repeat` is likewise unstable, so the chunk layout is sized by checked
multiplication and the header offset added by checked addition. Overflow is
therefore reported by `Option` rather than by `core`, and a chunk sizing that
overflows is clamped rather than propagated (see
[the multi pool](./multi-pool.md)).

## Proving the formulas

The typed geometry is cross-checked against the compiler's own layout of the
slot type: a `const` block asserts that each computed offset equals the
corresponding `offset_of!` on the slot struct, and that the computed stride and
alignment equal the slot struct's `size_of` and `align_of`. Every element type
a `TypedGeometry` is instantiated for therefore re-verifies the formula against
ground truth, and a divergence is a build error rather than a corrupted free
list.

The check must be *forced* from a path every instantiation reaches — an
associated `const` is only evaluated where it is used, so the constructor
references it. The constructor is the only way to obtain a value of the type,
so no instantiation can route around the check. These are post-monomorphization
errors, so the diagnostic is poor and the check cannot be tested negatively;
that is acceptable for an assertion whose only job is to fail a build that
would otherwise ship a corrupted free list.

A value allocated from a multi pool meets `RuntimeGeometry` on both of its
ordinary paths: allocation reads the layout pool's stored geometry, and the
detachable handles reclaim through the erased free path. The bound owner is
what pulls the typed provider in — its drop path recovers the chunk header
through `TypedGeometry<T>`, whose constructor forces the check — so the multi
pool's tests deliberately drive their layout spread through it as well as
through the handles.

This check is the primary guard on the formulas, which is why it asserts stride
and alignment and not only offsets. Both halves of the pool evaluate the same
formula, so a bug in it would be self-consistent and functionally invisible: a
wrong stride used to place slots and to find them again produces a pool that
agrees with itself and disagrees with the compiler. Comparing against the
compiler's layout is what turns such a bug into a failed build.

One consumer is deliberately left on an independent derivation for the same
reason. The bound owner holds a `NonNull<SlotCell<T>>` and reaches the value,
the reference count and the index through the compiler's field offsets rather
than through the formulas (see [handles](./handles.md)); only its step back to
the chunk header consults a geometry provider. A formula bug shared by both
providers is therefore observable as a test failure.

Because both providers share the formulas, comparing them against each other
proves only that neither corrupted the other's inputs. The formulas themselves
are therefore also asserted against `Layout::extend` and `pad_to_align` — a
second, `core`-owned implementation of the same `repr(C)` placement rules —
over the same spread of types. `extend` cannot be used to *build* the geometry
without paying for it on the free path, but it costs nothing to use as an
oracle in a test.

The formulas are validated against the compiler's layout for the full spread
the pool must handle: zero-sized types, sub-word and odd sizes, word-sized and
double-word types, alignments up to a page, `MaybeUninit<T>` (which shares its
layout with `T`, so it routes to the same layout pool), and the unsized views
produced by coercing to a trait object or to a slice. In every case the
geometry derived from a value's runtime size and alignment equals the geometry
the pool was built with. That agreement is the allocation and reclamation
safety invariant: a value is allocated and later recovered through matching
stride and metadata offsets. The multi-pool directory keys on slot geometry
rather than on the exact Rust `Layout`: a value's size paired with its
alignment widened to hold the slot metadata, so layouts differing only below
that alignment share one layout pool (see
[the multi pool's design](../design/multi-pool.md)).
