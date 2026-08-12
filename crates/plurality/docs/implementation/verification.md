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
| Allocation tracking | Hidden allocations and leaks the type system does not catch |
| Static assertions | Auto-trait and variance behaviour, which no runtime test observes |
| Mutation testing and coverage | Assertions that do not constrain anything |

## Test targets

Tests are external integration targets, with one exception. That is a
deliberate constraint: it keeps the tests honest about the public surface, and
the few internals worth exercising directly are reached through a gated
re-export rather than by testing from the inside.

The exception is `src/geometry.rs`, which carries a unit test module. The
geometry formulas take a size and an alignment, not a type, and the property
worth asserting is that they agree with the compiler's layout of a slot struct
that is crate-private. There is no public surface to reach that through, so the
test sits beside the code.

| Target | Scope |
|---|---|
| `pool` | Construction and builder validation, introspection, growth, power-of-two rounding, slot reuse, bounded exhaustion and recovery, panicking closures and destructors, allocator failure, chunk release at teardown, handles outliving the pool, moving a pool across threads, concurrent frees and compare-exchange contention. |
| `blind_pool` | Heterogeneous mixes, two types sharing one layout, zero-sized and over-aligned values, coercion from a blind pool, handles outliving the pool, per-layout capacity exhaustion, the layout cap, the sizing clamps at both ends, allocator failure on both the chunk and metadata paths, and panic safety in construction closures. |
| `box`, `arc`, `rc`, `alloc` | Per-handle behaviour and the uninitialized placement tier. The `rc` target additionally covers the non-atomic-to-atomic handover when a freed slot is reused by an `Arc`. |
| `smart_ptr` | The shared macro-generated surface: pointer accessors, identity, uniqueness queries, construction-time pinning, `Unpin`, the auto-trait assertions and every forwarding impl. |
| `unsize` | Type erasure end to end: trait objects, generic and borrowed trait arguments, array-to-slice, destructors through a vtable, slice element drops, `dyn Future` with a pinned view, slot reuse after an unsized drop, cross-thread frees of erased handles, pin-preserving coercion, leak behaviour when a coercion panics, zero-sized and over-aligned reclamation, and metadata preservation across a raw round trip. |
| `unwind_safe` | The unwind-safety contracts, including negative probes that fail to compile if a bound is widened. |
| `send_bound_probe` | The argument that `Pool: Send` needs no `T: Send`, by moving pools of a deliberately thread-bound element type across threads. Interesting only under Miri, since the failures it looks for are aliasing and data-race violations. |
| `stats` | The counters, compiled only with the `stats` feature. |
| `bolero_pool` | Randomised operation streams. |
| `bolero_blind_pool` | Randomised operation streams over a spread of layouts. |
| `loom_pool` | Interleaving models, compiled only under `--cfg loom`. |
| `alloc_tracking` | Steady-state allocation behaviour under a tracking global allocator. |

Two obligations in this table are specific to the blind pool and are easy to
satisfy vacuously, so they are called out.

**The layout spread must be exercised through `Alloc` specifically**, not only
through the detachable handles. The bound owner is the sole consumer that reads
slots through the compiler's layout of the slot type (see
[handles](./handles.md)), and it is also what instantiates the typed geometry
provider for a type a blind pool would otherwise serve entirely through the
runtime one. Covering the spread only through `Box`, `Arc` and `Rc` would
exercise one geometry derivation against itself.

**Reentrancy is tested at each point the cold path releases control.**
Allocating from, and freeing into, a blind pool from inside a pooled value's
destructor and from inside a construction closure covers the ordinary points.
An allocator instrumented to re-enter the pool from `A::clone` covers the
layout-pool construction window, which is where the duplicate re-scan and the
cap re-check live and which no user-written closure can reach. Reentrancy from
the global allocator and from the pool's own allocator is excluded by
precondition (see [the blind pool](./blind-pool.md#reentrancy)).

## Undefined-behaviour checking

Miri runs the suite under several configurations mirroring CI, including
stacked borrows. It is the primary check on the pointer-recovery arithmetic,
which is exercised over a spread of layouts precisely because a divergence
between the two geometry providers surfaces there as an out-of-bounds or
misaligned access rather than as a wrong answer.

Two targets opt out of Miri: the fuzz target, which needs filesystem isolation
Miri does not provide, and the allocation-tracking target, whose global
allocator is not meaningful under Miri's own allocator.

## Interleaving exploration

Loom exhaustively explores the orderings of the free-list protocol and the
teardown handover. The models cover two shared handles on one slot, teardown
running on a worker thread after the pool object is gone, concurrent frees of
distinct slots, a free racing the splice at the end of growth — driven by an
allocator that stalls the second chunk allocation to force the race — and the
exactly-once destruction of a value. The blind pool adds teardown of one layout
pool on a non-allocator thread after the router is gone, and concurrent frees
across two layout pools.

Loom's atomics are substituted for the crate's own through the `atomic` module,
and the instrumented objects must be dropped rather than merely deallocated,
which is why teardown and the chunk guard carry loom-only drop loops.

## Property and fuzz testing

Bolero drives a byte stream interpreted as a sequence of allocate, clone and
drop operations, asserting that every value is destroyed exactly once and that
releasing every handle empties the pool. For the blind pool the stream ranges
over a spread of layouts, and the assertions add that a slot address is never
served for two different layouts — layout pools neither share chunks nor
release them, so an address that appears under two layouts is a slot that
crossed layouts.

## Allocation tracking

A tracking global allocator asserts that steady-state operation performs no
system allocations: fill-and-drop and rolling churn for each handle flavour, a
warmed blind pool across a mix of layouts, and the benchmark bodies behind the
published fat-pointer comparison, so that the claim in
[`PERF.md`](../PERF.md) is enforced rather than asserted. The target carries its
own copy of those bodies so that it pulls in no cross-target files.

Leak assertions belong here too: teardown returns the pool's metadata
allocation to the global allocator, and the drop glue it runs by hand — the
directory vector's buffer above all — is observable only through the global
allocator, not through the pool's own.

## Static assertions

Auto-trait and variance behaviour is not observable at run time, so it is
asserted at compile time: the negative assertions that pin `Rc` and `Alloc` as
neither `Send` nor `Sync`, the positive ones for `Box` and `Arc` under their
respective bounds, and the blind pool's own `Send`-ness depending on its
allocator alone. These assertions are what catch a marker field that silently
stops denying a trait.

## Mutation testing and coverage

Mutation testing guards the arithmetic that a weak suite would leave
unconstrained — the geometry formulas and the routing decision above all.
Individual sites are excluded with a recorded justification, and the exclusions
are limited to branches a test cannot reach, such as the reference-count
overflow guard, which requires a count no test can produce and aborts the
process if it fires.

Coverage instrumentation is likewise switched off for genuinely unreachable
paths rather than left to report them as gaps, using the same per-site
justification.

## Running it

A single script drives the correctness suites — Miri in its several
configurations, loom, the fuzz target, an arithmetic-checked build, and
doctests — with a flag per suite so that any one of them can be run alone. A
second script runs the benchmark suites and regenerates
[`PERF.md`](../PERF.md); its loop count must match the wall-clock harness's, or
the per-operation numbers it derives are wrong.
