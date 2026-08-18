# Performance

This document covers the cost model of the pool forms, the properties that must
not regress, and the benchmark decomposition that attributes each cost to its
source. Back to the [implementation hub](../IMPLEMENTATION.md). The benchmark
suites under `benches/` produce the numbers cited here.

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
every geometry expression on that path folds to a constant. The check is the
typed rows of the Callgrind suite, read against the counts recorded below.
Nothing enforces it automatically: no baseline is committed, and the suite is
not a CI gate. Ref: docs/callgrind-benchmarks.md.

The one genuinely new cost on the runtime path is that slot addressing
multiplies by a loaded stride from the pool body, where a typed pool's constant
stride folds into an addressing mode. Measurement puts the difference at one
instruction — the multiply and its loads all but displace the address
arithmetic the typed path performs instead — so this is very nearly a
difference in shape rather than in work. It is confined to `LayoutPool`; the
typed path keeps its constant.

## Benchmarks

Multi-pool coverage follows the workspace conventions: identical operation
bodies in the wall-clock and instruction-count harnesses, single-threaded,
measuring elementary operations against a pre-warmed pool, with growth and
first-use effects outside the measured region except in the first-touch row,
which exists to measure them. [`PERF.md`](../PERF.md)
publishes a curated wall-clock subset; the instruction-count suites stay in the
repository for optimization work.

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

Read straight off the two rows, the intercept is 37 instructions, and not all
of it belongs to the pool. The benchmark function receives its pool by value
and hands it back, and a `MultiPool` value is wider than a `Pool<T>` value,
which costs the routed wrapper 8 instructions the typed wrapper does not pay.
The routed operation body is shared with the sixteen-layout row, and the
compiler emits it out of line, for 4 more. Setting both aside leaves 25
instructions that routing costs a caller. The fat-pointer pair, whose
operation bodies are inlined on both sides, reaches the same figure after the
same subtraction and attributes it to the same source files instruction for
instruction.

Measured against the typed pool on x86-64, for one allocation from a warm pool
holding a single directory entry, those 25 divide as:

| Cost | Instructions |
|---|---:|
| Directory scan: borrowing both vectors, loop setup, one key comparison | 13 |
| Reaching the pool body: the bounds check on the parallel vector and copying the view out of it | 5 |
| Routing helper: moving the payload into the closure and threading the `Result` back out | 6 |
| Slot addressing from a loaded stride rather than a folded constant | 1 |

That last row is the whole of what the runtime geometry costs. Every allocation
entry point funnels through one routing helper that takes the value, or its
constructor, as a closure, and that helper is inlined unconditionally. Without
the annotation the compiler emits it out of line, and the resulting frame,
argument setup, `Result` returned through memory and second copy of the payload
into the closure's slot add 11 instructions to every routed allocation and 16
to the fat-pointer row — more than everything the helper does. The price is
code size, since the directory scan is replicated per entry point per element
type. The typed path does not route and is unaffected either way.

Parts of this are not what the shape of the code suggests.

The runtime stride is all but free. Slot addressing on the routed path loads
the stride, offset and slots offset from the pool body and multiplies, where
the typed path folds a constant into an addressing mode, and the two come out
one instruction apart: the routed path trades an address computation for a
multiply and its loads. The geometry it loads is not copied wholesale either —
it costs a couple of loads across the whole operation, not one per field at
every accessor that takes `self` by value — so only what an operation uses is
read, whatever the source reads into a local.

The lookup is where the intercept sits. Getting to the lookup is a quarter of
it; the scan and the pool it selects are the rest, and the free-list pop and
the value write cost the same on both paths.

The per-entry slope is what the linear scan actually stakes its case on, and it
is the part that scales. Across the two rows it comes out at 6 instructions per
directory entry. The intercept is a fixed toll on the routed path only.

Instructions are not time, and for the scan the gap is wide. Sixteen layouts
run roughly twice the instructions of one, yet the wall-clock rows for the two
land within a few percent of each other, with the longer scan sometimes the
faster of the two. The scan is a predictable walk over a contiguous key vector
with no dependency on the pool's own pointer chasing, so the processor overlaps
it; what is left is below the swing that heap and code placement produce
between builds. This is why the routing cost is stated in instructions, why the
published wall-clock report prices type erasure at the whole step from the
typed pool rather than at the difference between layout counts, and why work on
shortening the scan is worth judging by the intercept it removes rather than by
the slope.

Reclamation is the same code on both paths and contributes equally to every
row, so it cancels in the differences. That also gives the design's claim that
reclamation costs what it costs in a typed pool a way to fail — a divergence
would show up as a delta larger than the modeled addition explains.

### First touch of a layout

Routing branches on whether the directory already holds the key, and both ends
of that branch are measured. The miss row allocates a layout the pool has never
seen from a directory one entry long, so it reads directly against the
one-layout hit row, which scans the same distance and finds its pool. What the
row prices is the whole first touch — the scan that fails, the cloned allocator
and layout-pool metadata, the directory push, and the first chunk the growth
path then allocates — because that is what presenting a new layout costs a
caller. It is not a measurement of the scan alone, and the row cannot separate
the two contributions.

The chunk dominates the result, which places the row orders of magnitude above
the hit row and ties it to the configured chunk size rather than to anything
the router does. It is therefore read as a first-touch figure in its own right,
not against the steady-state rows. Its wall-clock counterpart builds a fresh
pool for every measured iteration, so it reports one allocation per iteration
where the steady-state rows report a block of them.

### Scenarios

- **Allocate and free, one layout present** — the low case for routing cost and
  the direct comparison against the typed pool's corresponding row.
- **Allocate and free, sixteen layouts present** — the high case, isolating how
  lookup scales with directory size.
- **Allocate a layout the pool has never seen** — the miss end of the routing
  branch, covering installation of a layout pool and its first chunk.
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
