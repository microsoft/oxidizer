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
layout-parameterised pool possible at all: the reclamation half of the pool
does not depend on the element type, only on its layout.

Two standard-library guarantees carry the chain from `Layout::new::<T>()` to
the slot's first field, and both are load-bearing: `UnsafeCell<T>` is
`#[repr(transparent)]` over `T`, and `MaybeUninit<T>` has the same size,
alignment and ABI as `T`. Together they mean the slot's value field has exactly
`T`'s layout, so routing on `Layout::new::<T>()` and laying out a slot from
that layout describe the same bytes.

Notice that `slots_off` does not depend on the slot count. Recovery and slot
addressing are therefore pure arithmetic over the geometry and an index, with
no per-chunk state to consult beyond the chunk's base address. See
[the pool body](./pool-body.md) for how the two directions of that arithmetic
are used.

## The geometry provider

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
typed path folds to a constant. Storing it in `PoolInner` costs nothing.

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
`const` typed path carries its own arithmetic because `Layout::extend` is not
usable in `const` context, and the two are cross-checked against each other for
every layout that both paths see.

The free path uses neither provider. It derives its own geometry from the
value's runtime size and alignment. That asymmetry is deliberate and
load-bearing: reclamation must not depend on reaching the pool before it knows
where the pool is.

## Proving the formulas

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

This check is the primary guard on the formulas, which is why it asserts stride
and alignment and not only offsets. Both halves of the pool evaluate the same
formula, so a bug in it would be self-consistent and functionally invisible: a
wrong stride used to place slots and to find them again produces a pool that
agrees with itself and disagrees with the compiler. Comparing against the
compiler's layout is what turns such a bug into a failed build.

One consumer is deliberately left on an independent derivation for the same
reason. The bound owner reads its slot through the compiler's layout of the
slot struct rather than through a geometry provider (see
[handles](./handles.md)), so a formula bug shared by both providers is
observable as a test failure.

The runtime geometry is checked differently, because it has no slot struct to
compare against: it is *built* from `Layout::extend`, which is `core`'s own
`repr(C)` algorithm, and the hand-rolled formula is asserted against it in
debug builds. Between them, neither derivation is trusted alone.

The formulas are validated against the compiler's layout for the full spread
the pool must handle: zero-sized types, sub-word and odd sizes, word-sized and
double-word types, alignments up to a page, `MaybeUninit<T>` (which shares its
layout with `T`, so it routes to the same layout pool), and the unsized views
produced by coercing to a trait object or to a slice. In every case the
geometry derived from a value's runtime size and alignment equals the geometry
the pool was built with — which is the invariant the whole blind design rests
on, and the reason the design treats geometry agreement rather than type
identity as the routing rule (see
[the blind pool's design](../design/blind-pool.md)).
