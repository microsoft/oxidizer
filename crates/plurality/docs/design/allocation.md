# Plurality — Allocation

The shape of the allocation entry points, how a pool reports failure, and the
allocator abstraction it draws memory through. Part of the
[architecture](../DESIGN.md).

## Allocation surface and failure

Each handle flavor ([handles](./handles.md)) offers the same shape of
allocation entry points:

- a **by-value** form for convenience,
- a **closure-based** form that defers value construction until a slot is
  available, and
- an **uninitialized-then-initialize** form, the guaranteed zero-copy path,
  mirroring the standard library's `new_uninit` idioms.

Every form has an infallible variant and a **fallible** sibling. The
infallible variant panics for failures represented as `AllocError`; the
fallible sibling returns them. A pool reports one of two architecturally
distinct reasons, and the error distinguishes them:

- **Capacity exhausted** — a configured chunk cap (or the intrinsic index
  ceiling of an unbounded pool) is reached and no slot is free.
- **Allocator failure** — acquiring a new chunk from the underlying allocator
  failed.

On failure the rejected value is dropped and no construction closure is invoked,
matching the standard fallible-allocation convention.

A [blind pool](./blind-pool.md) reports the same two failures, widened to cover
its layout cap, directory capacity and its own metadata; it adds no third.
Global-allocator out-of-memory handling on paths that are not represented as
`AllocError` follows the global allocator's own behavior.

## `no_std` and allocator integration

The pool depends only on `alloc` — no `std`, and no operating-system
synchronization primitives. This is feasible precisely because of the concurrency
model: allocation and growth are single-threaded, and the free list is a
lock-free stack, so only plain atomics are required. Chunk acquisition goes
through the standard allocator abstraction, so custom and instrumented allocators
compose naturally.

An allocator supplied to a pool must not allocate from, or free into, a
plurality pool from within `allocate` or `deallocate`. The
[invariant list](../DESIGN.md#design-invariants-at-a-glance) states the rule and
its scope; it constrains allocator callbacks only, and never pooled values'
destructors or construction closures.
