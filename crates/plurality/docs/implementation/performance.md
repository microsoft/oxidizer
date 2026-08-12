# Performance

This document covers the cost model of the pool forms, the properties that must
not regress, and the benchmark decomposition that attributes each cost to its
source. Back to the [implementation hub](../IMPLEMENTATION.md). Measured
numbers live in [`PERF.md`](../PERF.md).

## Cost model

- **Allocate, typed pool:** pop the free list, write the value, and — for the
  shared handles — initialize the slot counter and increment the pool
  reference count.
- **Allocate, blind pool:** the above, plus a scan of the key vector, a load of
  the layout pool's pointer, and a slot address computed from a loaded stride
  rather than a constant. With one layout present the scan is a single
  comparison; each further layout adds a few instructions, so the structure
  suits the handful of distinct layouts a program actually presents.
- **Free:** the same erased reclamation path in both forms. The router is not
  involved, and the arithmetic is what a typed pool's handle runs.
- **First use of a layout:** cold, and outlined so that an allocation whose
  layout is already known costs nothing beyond the scan. A miss clones the
  allocator, allocates the layout-pool metadata, reserves directory capacity
  when needed, pushes the two directory entries, then enters the ordinary
  growth path.
- **Growth:** cold and never inlined, so it costs one allocation plus the
  initialization of a whole chunk, amortised over the chunk's slots.

## What must not regress

Parameterising the pool body over geometry must leave the typed pool's emitted
code identical to what a body written against the element type produces.
`TypedGeometry<T>` is zero-sized and its accessors are constant-evaluable, so
every geometry expression on that path folds to a constant. The gate is
instruction counts: the typed rows in [`PERF.md`](../PERF.md) must hold.

The one genuinely new cost on the runtime path is that slot addressing
multiplies by a loaded stride from the pool body.
Where a typed pool's stride is a power of two the compiler emits a shift or
folds the scaling into an addressing mode, so this is a real addition rather
than a relocation of work the compiler was going to do anyway. It is confined
to `LayoutPool`; the typed path keeps its constant.

## Benchmarks

Blind-pool coverage follows the workspace conventions: identical operation
bodies in the wall-clock and instruction-count harnesses, single-threaded,
measuring elementary operations against a pre-warmed pool with growth and
first-use effects outside the measured region.

### Attributing the routing cost

The blind path adds two costs over the typed path — a runtime stride on the
addressing path and a directory scan — and a benchmark comparing only the two
ends reports their sum as one number. The scan is separated out by varying the
directory size instead: the same operation runs against a pool holding one
layout and against a pool holding sixteen, with the measured layout registered
last so the scan runs its full length. The slope between them is the per-entry
scan cost; the intercept is everything else the blind path adds.

That pair of layout counts is the whole parameterisation. One low value and one
high value make the per-entry cost legible, and further values would add rows
without adding information.

The intercept still folds the runtime stride together with the fixed part of
the lookup — loading the two directory vectors and following the layout pool's
pointer. Splitting those would need a benchmark rung between the two, against
`LayoutPool` directly, which is crate-private and would have to be exposed to
reach it. That is public-surface debt for a diagnostic, so it is recorded as
available in [`TODO.md`](../TODO.md) rather than built: the shipped pair
already bounds the cost that scales, which is the one that governs whether the
linear scan remains the right structure.

Reclamation is the same code on both paths and contributes equally to every
row, so it cancels in the differences. That also gives the design's claim that
reclamation costs what it costs in a typed pool a way to fail — a divergence
would show up as a delta larger than the modeled addition explains.

### Scenarios

- **Allocate and free, one layout present** — the low case for routing cost and
  the direct comparison against the typed pool's corresponding row.
- **Allocate and free, sixteen layouts present** — the high case, isolating how
  lookup scales with directory size.
- **Allocate, coerce to a trait object, dispatch, and free** — the row that
  lines up with the owning fat-pointer comparison, where the reference
  implementation's blind pools already appear. This is where the architectural
  claim is expressed as a number.

The cross-crate comparison set stays typed. Every pool in it is generic over
one element type, so the blind pool has no counterpart there; its cross-crate
row is the fat-pointer comparison, which is where the surveyed crates expose
their own heterogeneous pools.

Allocation tracking asserts that a warmed blind pool performs no system
allocations in steady state, including across a mix of layouts.
