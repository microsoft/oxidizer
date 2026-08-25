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
- **Allocator failure** — acquiring memory failed, whether for a new chunk from
  the configured allocator or for the pool's own bookkeeping from the global
  allocator.

On failure the rejected value is dropped and no construction closure is invoked,
matching the standard fallible-allocation convention.

A [multi pool](./multi-pool.md) reports the same two failures, widened to cover
its layout cap and its own metadata; it adds no third.
Global-allocator out-of-memory handling on paths that are not represented as
`AllocError` follows the global allocator's own behavior.

## `no_std` and allocator integration

The pool depends only on `alloc` — no `std`, and no operating-system
synchronization primitives. This is feasible precisely because of the concurrency
model: allocation and growth are single-threaded, and the free list is a
lock-free stack, so only plain atomics are required. Chunk acquisition goes
through the standard allocator abstraction, so custom and instrumented allocators
compose naturally.

Allocators supplied to pools carry no plurality-specific reentrancy
requirement. `Allocator::allocate` and `Allocator::deallocate` may allocate
from, and free into, the pool they serve. Cold growth and directory-reservation
paths are ordered so such reentry is safe, and `Clone::clone` on a multi pool's
allocator is covered by the same ordering. Pooled values' destructors and
`_with` construction closures run with no pool state in flight and are
unrestricted. An allocator that re-enters unconditionally recurses until the
stack is exhausted; the pool does not bound recursion depth. See
[allocator reentrancy](../implementation/reentrancy.md).
