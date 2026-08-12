# Plurality — Concurrency

Which operations may overlap, which threads may perform them, and what the
caller supplies. Part of the [architecture](../DESIGN.md).

## Concurrency model

The pool follows a **single-producer / multi-consumer** discipline, and this
single decision shapes the whole design.

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
  happen on exactly one thread at a time. The pool object can be *moved* between
  threads, but only one thread ever holds it, so these operations are
  uncontended and need no locking among themselves.
- **Frees are concurrent.** The owning and shared handles are thread-mobile, so
  many threads may drop handles — and thus return slots — simultaneously.

The consequence is an asymmetric design: one thread pushes new capacity and pops
slots without contention, while any number of threads concurrently return slots.
Only the hand-off point between them needs synchronization, and it is expressed
entirely with atomics — **there is no mutex anywhere in the pool**. State touched
only by the single allocator thread (notably the directory of chunks) needs no
synchronization at all; its confinement to that one thread is itself the
soundness argument. The [free list](./memory.md) is where the hand-off happens.

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

A pooled value's destructor and a construction closure may allocate from, and
free into, the same pool. There is no lock to re-enter, and no directory borrow
is held across user code.

The pool's allocator and the global allocator are held to the opposite rule:
neither may allocate from, or free into, a plurality pool from within
`allocate` or `deallocate`, because the pool's own state is mid-update while
those calls are outstanding. The
[invariant list](../DESIGN.md#design-invariants-at-a-glance) states this in
full.

