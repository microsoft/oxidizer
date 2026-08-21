# Verification

This document covers how the implementation is validated: the layered strategy,
the test targets that implement it, and the tools each layer runs under. Back
to the [implementation hub](../IMPLEMENTATION.md). The design-level summary is
in [`DESIGN.md`](../DESIGN.md).

Each layer targets a failure class the others cannot reach. The pool's
correctness rests on unsafe pointer arithmetic, a lock-free protocol and
compile-time trait behaviour, and no single technique covers all three.

| Layer | Failure class |
|---|---|
| Functional tests | Behaviour of the handle surface, panic paths, allocator behaviour, capacity limits |
| Undefined-behaviour checking | Aliasing and provenance errors in pointer recovery |
| Interleaving exploration | Missing or misordered atomic synchronisation |
| Property and fuzz testing | Invariant violations under sequences nobody thought to write |
| Allocation tracking | Hidden steady-state allocations the type system does not catch |
| Static assertions | Auto-trait and variance behaviour, which no runtime test observes |
| Mutation testing and coverage | Assertions that do not constrain anything |

## Test targets

Tests are external integration targets. That is a deliberate constraint: it
keeps the tests honest about the public surface. Two modules carry unit tests
instead, because what they assert cannot be reached from outside the crate.

`src/geometry.rs` asserts that the geometry formulas agree with the compiler's
layout of a slot struct that is crate-private. The formulas take a size and an
alignment, not a type, so there is no public surface to drive them through.

`src/layout_pool.rs` asserts the sizing floor: a value layout so large that a
single-slot chunk cannot be described. Reaching it requires a `Layout` no Rust
type on a 64-bit target can have (see [multi pool](./multi-pool.md), "Clamping
the sizing configuration"), so the tests construct the layout directly and call
the crate-private constructor.

| Target | Scope |
|---|---|
| `pool` | Construction and builder validation, introspection, growth, power-of-two rounding, slot reuse, bounded exhaustion and recovery, panicking closures and destructors, allocator failure, chunk release at teardown, handles outliving the pool, moving a pool across threads, concurrent frees and compare-exchange contention. |
| `multi_pool` | Heterogeneous mixes, two types sharing one layout, zero-sized and over-aligned values, coercion from a multi pool, handles outliving the pool, per-layout capacity exhaustion, the layout cap, the sizing clamps at both ends, allocator failure on both the chunk and metadata paths, and panic safety in construction closures. |
| `box`, `arc`, `rc`, `alloc` | Per-handle behaviour and the uninitialized placement tier. The `rc` target additionally covers the non-atomic-to-atomic handover when a freed slot is reused by an `Arc`. |
| `smart_ptr` | The shared macro-generated surface: pointer accessors, identity, uniqueness queries, construction-time pinning, `Unpin`, the auto-trait assertions and every forwarding impl. |
| `unsize` | Type erasure end to end: trait objects, generic and borrowed trait arguments, array-to-slice, destructors through a vtable, slice element drops, `dyn Future` with a pinned view, slot reuse after an unsized drop, cross-thread frees of erased handles, pin-preserving coercion, leak behaviour when a coercion panics, zero-sized and over-aligned reclamation, and metadata preservation across a raw round trip. |
| `unwind_safe` | The unwind-safety contracts, including negative probes that fail to compile if a bound is widened. |
| `send_bound_probe` | Representative evidence for the argument that `Pool: Send` needs no `T: Send`, by moving pools of a deliberately thread-bound element type across threads. Interesting only under Miri, since the failures it looks for are aliasing and data-race violations. |
| `stats` | The counters, compiled only with the `stats` feature. |
| `bolero_pool` | Randomised operation streams. |
| `bolero_multi_pool` | Randomised operation streams over a spread of layouts. |
| `loom_pool` | Interleaving models, compiled only with the `loom` marker feature and `--cfg loom`. |
| `alloc_tracking` | Steady-state allocation behaviour under a tracking global allocator. |

Two obligations in this table are specific to the multi pool and are easy to
satisfy vacuously, so they are called out.

**The layout spread must be exercised through `Alloc` specifically**, not only
through the detachable handles. The bound owner is the sole consumer that reads
slots through the compiler's layout of the slot type (see
[handles](./handles.md)), and it is also what instantiates the typed geometry
provider for a type a multi pool would otherwise serve entirely through the
runtime one. Covering the spread only through `Box`, `Arc` and `Rc` would
exercise one geometry derivation against itself.

**Reentrancy is exercised through the same doors the design permits.** The
allocator entry-point door is driven by an allocator whose `allocate` allocates
from the pool it serves, with single-slot chunks so every allocation grows. The
tests assert that the outer and nested allocations receive distinct chunks,
that both values survive, and that the cap re-check holds the pool to its
limit.

The multi-pool allocator-clone door is covered by an allocator whose
`Clone::clone` allocates from the same multi pool while a layout pool is being
installed. The tests cover a nested allocation of the same layout, a nested
allocation that consumes the remaining layout allowance, and a nested directory
growth between reservation and publication. Pooled value destructors and
`_with` construction closures allocate from the same multi pool after directory
borrows have been released.

Directory reservation is driven through a custom global allocator, because
directory buffers come from the global allocator rather than the pool's own and
no pool-level allocator can reach them. One test refuses the reservation and
asserts the chunk allocated before it is returned rather than published;
another allocates from the pool while the reservation is outstanding, filling
the buffer it prepared so that the reservation must start over with a larger
one.

## Undefined-behaviour checking

Miri runs the non-Bolero, non-allocation-tracking suite under the configurations
CI uses: the default stacked-borrows model, tree borrows, strict provenance, and
multi-seed race coverage. `cargo careful` runs alongside it. Miri is the primary
check on the pointer-recovery arithmetic, which is exercised over a spread of
layouts precisely because a divergence between the two geometry providers
surfaces there as an out-of-bounds or misaligned access rather than as a wrong
answer.

The mixed-layout property target runs under Bolero in native execution rather
than under Miri, because Bolero needs filesystem isolation Miri does not
provide. The allocation-tracking target also opts out, because its global
allocator is not meaningful under Miri's own allocator.

## Interleaving exploration

Loom exhaustively explores the orderings of the typed pool's free-list protocol
and teardown handover. The models cover two shared handles on one slot,
teardown running on a worker thread after the pool object is gone, concurrent
frees of distinct slots, a free racing the splice at the end of growth — driven
by an allocator that stalls the second chunk allocation to force the race —
and the exactly-once destruction of a value.

Loom's atomics are substituted for the crate's own through the `atomic` module,
and the instrumented objects must be dropped rather than merely deallocated,
which is why teardown and the chunk guard carry loom-only drop loops.

## Property and fuzz testing

Bolero drives a byte stream interpreted as a sequence of allocate, clone and
drop operations in native execution, against a chunk size small enough that
growth and slot reuse are constant, asserting that every value is destroyed
exactly once and that releasing every handle empties the pool. For the multi
pool the stream ranges over a spread of layouts, and the assertions add that a
slot address is never served for two different layouts — layout pools neither
share chunks nor release them, so an address that appears under two layouts is
a slot that crossed layouts.

## Allocation tracking

A tracking global allocator asserts that warmed steady-state operation spans
perform no system allocations: fill-and-drop and rolling churn for each handle
flavor, a warmed multi pool across a mix of layouts, and the benchmark bodies
behind the published fat-pointer comparison, so that the claim in
[`PERF.md`](../PERF.md) is enforced rather than asserted. The target carries
its own copy of those bodies so that it pulls in no cross-target files. These
assertions measure `total_bytes_allocated == 0` inside operation spans after
construction, warm-up and holding-vector reservation; they do not balance
outstanding global allocations across pool construction and teardown.

## Static assertions

Auto-trait and variance behaviour is not observable at run time, so it is
asserted at compile time: the negative assertions that pin `Rc` and `Alloc` as
neither `Send` nor `Sync`, the positive ones for `Box` and `Arc` under their
respective bounds, and the multi pool's own `Send`-ness depending on its
allocator alone. These assertions are what catch a marker field that silently
stops denying a trait.

The `send_bound_probe` target adds representative compile-time and Miri runtime
evidence for the `Pool: Send` proof. Its scenarios are finite, and its
concurrent test observes the schedule Miri runs, so it supports the proof
rather than exhaustively enumerating every scenario or interleaving.

## Mutation testing and coverage

Mutation testing guards the arithmetic that a weak suite would leave
unconstrained — the geometry formulas and the routing decision above all.
Individual sites are excluded with a recorded, reproducible justification.
Exclusions cover unreachable branches, demonstrably equivalent alternatives, or
otherwise unviable mutations, such as the reference-count overflow guard, which
requires a count no test can produce and aborts the process if it fires, and the
initialization of a slot, whose removal would itself be undefined behaviour.
Because every exclusion carries its reason, a surviving mutant that is not
excluded is a genuine gap in the suite.

Coverage instrumentation is likewise switched off for genuinely unreachable
paths rather than left to report them as gaps, using the same per-site
justification.

## Running it

A single script drives the correctness suites — Miri in its several
configurations, loom, the configured Bolero target, an arithmetic-checked
build, and doctests — with a flag per suite so that any one of them can be run
alone. A second script runs the benchmark suites and regenerates
[`PERF.md`](../PERF.md); its loop count must match the wall-clock harness's, or
the per-operation numbers it derives are wrong.
