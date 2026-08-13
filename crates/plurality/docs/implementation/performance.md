# Performance

This document covers the cost model of the pool forms, the properties that must
not regress, and the benchmark decomposition that attributes each cost to its
source. Back to the [implementation hub](../IMPLEMENTATION.md). Measured
numbers live in [`PERF.md`](../PERF.md).

## Cost model

- **Allocate, typed pool:** pop the free list, write the value, and — for the
  shared handles — initialize the slot counter and increment the pool
  reference count.
- **Allocate, multi pool:** the above, plus a scan of the key vector, a load of
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
multiplies by a loaded stride from the pool body, where a typed pool's constant
stride folds into an addressing mode. Measurement puts the difference at zero
instructions — the multiply and its loads displace the address arithmetic the
typed path performs instead — so this is a difference in shape rather than in
work. It is confined to `LayoutPool`; the typed path keeps its constant.

## Benchmarks

Multi-pool coverage follows the workspace conventions: identical operation
bodies in the wall-clock and instruction-count harnesses, single-threaded,
measuring elementary operations against a pre-warmed pool with growth and
first-use effects outside the measured region.

### Attributing the routing cost

The routed path adds two costs over the typed path — a runtime stride on the
addressing path and a directory scan — and a benchmark comparing only the two
ends reports their sum as one number. The scan is separated out by varying the
directory size instead: the same operation runs against a pool holding one
layout and against a pool holding sixteen, with the measured layout registered
last so the scan runs its full length. The slope between them is the per-entry
scan cost; the intercept is everything else the routed path adds.

That pair of layout counts is the whole parameterisation. One low value and one
high value make the per-entry cost legible, and further values would add rows
without adding information.

The intercept is dominated not by the lookup but by the shape of the call that
performs it. Every allocation entry point funnels through one routing helper
that takes the value, or its constructor, as a closure; the compiler emits that
helper out of line. Measured against the typed pool on x86-64, for one
allocation from a warm pool holding a single directory entry:

| Cost | Instructions |
|---|---:|
| Out-of-line call: frame, argument setup, `Result` returned through memory | 18 |
| Second copy of the payload into the closure's slot | 8 |
| Directory: borrowing both vectors, loop setup, one key comparison, the bounds check on the parallel vector | 15 |
| Extra hop from the pool vector through the layout pool to the pool body, and one spill of the result | 3 |
| Free-list pop and slot addressing | 0 |

Two of these are worth stating plainly because they are not what the shape of
the code suggests.

The runtime stride costs nothing. Slot addressing on the routed path loads the
stride, offset and slots offset from the pool body and multiplies, where the
typed path folds a constant into an addressing mode — and the two encode to the
same instruction count. The routed path trades an address computation for a
multiply and three loads. The geometry it loads is not copied wholesale either:
only the fields an operation uses are read, whatever the source reads into a
local.

The lookup itself is the smaller half. Roughly two thirds of the intercept is
the cost of getting to the lookup rather than the lookup, and it would be paid
by any helper of this shape regardless of what it did.

The per-entry slope is what the linear scan actually stakes its case on, and it
is the part that scales. The intercept is a fixed toll on the routed path only.

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
  implementation's multi pools already appear. This is where the architectural
  claim is expressed as a number.

The cross-crate comparison set stays typed. Every pool in it is generic over
one element type, so the multi pool has no counterpart there; its cross-crate
row is the fat-pointer comparison, which is where the surveyed crates expose
their own heterogeneous pools.

Allocation tracking asserts that a warmed multi pool performs no system
allocations in steady state, including across a mix of layouts.
