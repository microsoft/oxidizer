# Plurality — Concurrency

Which operations may overlap, which threads may perform them, and what the
caller supplies. Part of the [architecture](../DESIGN.md).

## Concurrency model

The pool follows a **single allocator, multiple reclaimers** discipline, and
this discipline shapes the whole design.

```text
        ┌──────────────────────────────────────────────┐
        │            one allocator thread              │
        │   (holds the pool; grows it; hands out slots)│
        └───────────────┬──────────────────────────────┘
                        │ allocate
                        ▼
                 ┌─────────────┐   free (many threads)
                 │  free list  │ ◄───────────────┬───────────┐
                 └─────────────┘                 │           │
                        ▲                     ┌───┴───┐   ┌───┴───┐
                        │ pop                 │ drop  │   │ drop  │  …
                        │                     └───────┘   └───────┘
```

- **Allocation is single-threaded.** Growing the pool and popping free slots
  happen on exactly one thread at a time. This is not a convention the caller
  must uphold: the pool is `Send` but `!Sync`, so the shared reference every
  allocation entry point takes can never be observed from two threads at once.
  The pool object can be *moved* between threads and resumed there; what the
  type system forbids is two allocations overlapping in time.
- **Frees are concurrent.** Handles whose `Send` bounds are satisfied may be
  dropped on many threads simultaneously; non-`Send` handles remain local to
  their thread.

The consequence is an asymmetric design: one thread pushes new capacity and pops
slots without contention, while any number of threads concurrently return slots.
Only the hand-off point between them needs synchronization, and it is expressed
entirely with atomics — **there is no mutex anywhere in the pool**. State touched
only by the single allocator thread (notably the directory of chunks) needs no
synchronization at all; its confinement to that one thread is itself the
soundness argument. The [free list](./memory.md) is where the hand-off happens.

That confinement does more than avoid a lock. The chunk directory is a growable
vector of chunk pointers, so acquiring a chunk may reallocate it. Reclamation
therefore never consults the directory: it recovers the slot, the chunk and the
pool's shared state from the value pointer alone ([handles](./handles.md)). The
allocator thread reads the directory when it turns a free-list index into an
address, and teardown reads it once more, with the pool quiescent.

"Single-threaded allocation" is a rule about the pool object, not a prescription
for how programs are written. Serving several threads from one pool is ordinary
and expected: the caller wraps the pool in a `Mutex` — or any other exclusion it
prefers — and the critical section covers allocation only. Drops of detachable
handles stay outside it, because reclamation reaches the free list by arithmetic
and never touches the directory. The pool-bound handle is the exception: it
borrows the pool, so a guard it was allocated through must outlive it.
Externalising the lock this way is a deliberate division of labour: the pool
declines to choose a synchronization primitive, an allocation batching policy,
or a poisoning story on the caller's behalf, and in exchange the caller gets a
lock whose scope is the allocation call rather than the value's whole lifetime.

## Thread mobility

Moving a pool is independent of moving its values, and a pool is `Send`
whenever its allocator is — with no bound on the values it serves. A pool
object owns no values: every safely reachable value is owned through a handle,
the pool offers no iteration, and teardown reclaims chunk memory without ever
reading or dropping element storage. A thread that receives a pool object
therefore has no route to a value another thread placed in it, and can only
draw free slots, which hold nothing live. Thread mobility for values is carried
entirely by the handles — a handle to a non-`Send` value is itself non-`Send`
and stays on its thread regardless of where the pool goes.

This relaxation rests on the pool object never yielding or dropping a value,
which the [invariant list](../DESIGN.md#design-invariants-at-a-glance) records
as a standing constraint on the pool's API.

## Reentrancy

Reentrant pool use is part of the single-allocator-thread model: a nested call
runs on the same thread rather than overlapping another allocator thread.

The following apply:

- `Allocator::allocate` and `Allocator::deallocate` may allocate from, and free
  into, the pool they serve. Cold growth and directory-reservation paths order
  their state updates around those calls; see
  [allocator reentrancy](../implementation/reentrancy.md).
- `Clone::clone` on a multi pool's allocator may re-enter while a new layout
  pool is installed and is covered by the same ordering.
- Pooled values' destructors and the closures passed to `_with` constructors
  run with no pool state in flight, so they may allocate from and free into the
  pool freely.

An allocator that re-enters unconditionally recurses until the stack is
exhausted; the pool does not bound recursion depth.
