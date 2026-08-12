# Performance

This document covers the cost model of the pool forms, the properties that must
not regress, and the benchmark decomposition that attributes each cost to its
source. Back to the [implementation hub](../IMPLEMENTATION.md). Measured
numbers live in [`PERF.md`](../PERF.md).

## Cost model

- **Allocate, typed pool:** pop the free list, write the value, and — for the
  shared handles — initialise the slot counter and increment the pool
  reference count.
- **Allocate, blind pool:** the above, plus a scan of the key vector, a load of
  the layout pool's pointer, and a slot address computed from a loaded stride
  rather than a constant. With one layout present the scan is a single
  comparison.
- **Free:** the same erased reclamation path in both forms. The router is not
  involved, and the arithmetic is what a typed pool's handle runs.
- **First use of a layout:** cold. One global allocation for the pool metadata,
  two vector pushes, then the ordinary growth path.
- **Growth:** cold and never inlined, so it costs one allocation plus the
  initialisation of a whole chunk, amortised over the chunk's slots.

## What must not regress

Parameterising the pool body over geometry must leave the typed pool's emitted
code identical to what a body written against the element type produces.
`TypedGeometry<T>` is zero-sized and its accessors are constant-evaluable, so
every geometry expression on that path folds to a constant. The gate is
instruction counts: the typed rows in [`PERF.md`](../PERF.md) must hold.

The one genuinely new cost on the runtime path is that slot addressing
multiplies by a loaded stride. The stride shares a cache line with fields the
same code already loads, so the expected cost is one multiply plus an L1 hit.
Where a typed pool's stride is a power of two the compiler emits a shift or
folds the scaling into an addressing mode, so this is a real addition rather
than a relocation of work the compiler was going to do anyway. It is confined
to `LayoutPool`; the typed path keeps its constant.

## Benchmarks

Blind-pool coverage follows the workspace conventions: identical operation
bodies in the wall-clock and instruction-count harnesses, single-threaded,
measuring elementary operations against a pre-warmed pool with growth and
first-use effects outside the measured region.

### The three-rung ladder

The blind path adds a runtime stride on the addressing path and a directory
scan, and a benchmark that compares only the typed and blind ends reports their
sum as one number. Each operation is therefore measured on three rungs, each
differing from the one below it by a single cost:

| Rung | Path under test | Delta from the rung below |
|---|---|---|
| `Pool<T, A>` | Typed geometry, no router | — |
| `LayoutPool<A>` | Runtime geometry, no router | Runtime stride |
| `BlindPool<A>` | Runtime geometry, routed | Directory scan |

Each row measures one allocate-and-free pair, so its absolute count folds both
halves of the operation together. The deltas are what carry the attribution:
reclamation is the same code on all three rungs and contributes equally to each
row, so it cancels in the differences, leaving each delta attributable to the
one cost its rung adds. That also gives the design's claim that reclamation
costs what it costs in a typed pool a way to fail — a divergence there would
appear as a delta larger than the modelled addition explains.

The middle rung is a crate-private type, reached by the benchmark targets
through a `#[doc(hidden)]` re-export behind an internal feature. The workspace
guidance prefers benchmarking public API and accepts benchmarking an internal
step when the public chain is too coarse to localise a delta, which is exactly
the situation here.

### Scenarios

- **Allocate and free, one layout present** — the three rungs, for every handle
  flavour. This is the low case for routing cost and the direct comparison
  against the typed pool's corresponding rows.
- **Allocate and free, many layouts present** — the high case, applied to the
  routed rung, isolating how lookup scales with directory size. The pool is
  pre-populated with distinct layouts and the measured allocation targets one
  that is not first in the scan. The pair of layout counts is the whole
  parameterisation of the scan: one low value and one high value make the
  per-entry cost legible, and further values would add rows without adding
  information.
- **Allocate, coerce to a trait object, dispatch, and free** — the row that
  lines up with the owning fat-pointer comparison, where the reference
  implementation's blind pools already appear. This is where the architectural
  claim is expressed as a number.
- **Churn against the cross-crate comparison set** — the blind pool alongside
  the typed pool and the surveyed pool crates.

Allocation tracking asserts that a warmed blind pool performs no system
allocations in steady state, including across a mix of layouts.
