# Allocator reentrancy

A pool calls its allocator to obtain chunk memory. That call hands control to
code the pool does not own, and that code may allocate from the very pool it is
serving — an arena whose backing store is itself pooled does exactly this.

The pools place no obligation on the allocator. Reentrancy works, and this
document covers how the cold paths are ordered so that it does. Back to the
[implementation hub](../IMPLEMENTATION.md). For the allocator model see
[the allocation design](../design/allocation.md).

Reentrancy reaches a pool through three doors, and only the first needs care:

- `Allocator::allocate` and `Allocator::deallocate`, called on the cold growth
  paths while the pool is mid-flight. This is the door discussed below.
- `Clone::clone` on a multi pool's allocator, called once per new layout pool.
  This is on the same cold path and is covered by the same ordering.
- Destructors of pooled values and the closures passed to the `_with`
  constructors. These run with no pool state in flight and need no ordering.

Two distinct allocators can re-enter, and both are supported:

- The pool's own `A`, through the doors above.
- The `#[global_allocator]`, which the directory `Vec`s call directly. `A` has
  no part in directory growth, so the reservation paths below are reachable
  only from a reentrant global allocator, never from a reentrant `A`. The
  chunk-growth paths are the mirror image: they are reachable from `A`, and
  from the global allocator only where a directory grows alongside.

An allocator that re-enters unconditionally recurses until the stack is
exhausted, because serving the nested allocation calls it again. That is a
livelock in the allocator's own logic, not a memory-safety problem, so it is
the allocator author's to avoid — nothing in the pool bounds the depth.

## The two hazards

Reentry is a control transfer, not a data race; the pools are `!Sync` and the
nested call runs on the same thread, nested inside the outer one. What it
breaks is any invariant the outer call was relying on holding across the
allocator call.

**A stale chunk count.** A chunk's identity — its `base_index`, the first
global slot index it owns — is derived from the number of chunks allocated so
far. If the outer call reads that count before calling the allocator and uses
it afterwards, a nested `grow` that completed in between has already claimed
the range the outer call is about to claim. Both chunks then own one range of
global indices. Two later allocations hand out one slot to two live handles,
and a freed slot resolves through the directory to the wrong chunk.

**A live directory borrow.** The directory is a `Vec` in an `UnsafeCell`.
`Vec::push` and `Vec::try_reserve` hold `&mut` across their call into the
global allocator, so a nested access takes a second borrow of the same `Vec`.
That is an aliasing violation under Stacked and Tree Borrows even though no
data race occurs, and a concurrent `push` would also invalidate the outer
call's view of the buffer.

## Growth

`Pool::grow` closes both hazards by ordering alone:

1. Check the chunk cap, so an already-full pool does no work.
2. Call `Allocator::allocate`. Control leaves the pool here with nothing
   borrowed and nothing claimed. A nested `grow` runs to completion and
   publishes its chunk.
3. Reserve one directory slot (see below). This is the last point at which
   control leaves the pool.
4. Re-read the chunk count, now that control is back, and re-check the cap
   against it. Without the re-check the pool overshoots by the reentry depth,
   and an unbounded pool could derive slot indices that reach the `FREE_END`
   sentinel.
5. Derive `base_index` from that count and initialize the header and slots.
6. Push the chunk into the reserved capacity and publish the new count.

Steps 4 through 6 neither allocate nor panic, so the chunk is published
atomically with respect to reentry: no nested call can observe a half-built
directory, and no partial publication needs unwinding.

The two failure paths before step 5 return the chunk to the allocator. A
failure to grow is not by itself a failure to allocate: a nested allocation may
have published a chunk whose remaining slots are free, so the caller re-examines
the free list and reports capacity exhaustion only when there is genuinely
nothing to hand out.

That second look at the free list lives in `alloc_slot_by_growing`, which is
marked `#[cold]` and `#[inline(never)]`. Reentry is rare and growth is rarer,
so none of this belongs in the code the allocation path actually executes;
inlining it costs the free-list pop its fall-through position and forces the
slot pointer through a stack slot, because the two pops then merge there
instead of in a register.

## Reserving without a live borrow

`directory::reserve_one` grows a directory `Vec` without ever holding a borrow
across an allocation:

1. Sample the length and capacity, releasing the borrow immediately. Return at
   once if there is already room.
2. Allocate a fresh buffer with room for at least one more element, with no
   borrow live. The target doubles the current capacity, with a floor, so
   repeated reservation stays amortized constant rather than reallocating on
   every growth.
3. Re-read the length. The allocation released control, so a nested push may
   have grown the vector. Copying from the pre-allocation sample would silently
   drop that entry. If the vector has outgrown the buffer prepared for it,
   retry from step 1.
4. Move the elements into the fresh buffer, clear the source length so its
   destructor does not run on moved-out values, and swap the buffers under a
   short borrow that cannot allocate.
5. Return the displaced buffer to the caller **without freeing it**.

The last point matters more than it looks. Freeing the old buffer is a call
into the global allocator, and therefore one more release of control — after
the reservation. A nested push could consume the room just reserved and force
the supposedly infallible push to reallocate. Handing the buffer back as a
`Displaced<T>` moves that free to the end of the caller's critical section,
which is what makes step 3 of `grow` the genuinely last release of control.

## Reserving two vectors at once

A caller that needs room in two vectors cannot reserve them one after the
other and assume both reservations survive. The second `reserve_one` may
allocate, and that allocation is a release of control: a nested install can
push into the first vector and consume exactly the room reserved for the
caller. The caller would then push into a full vector, reallocating under the
`&mut` it holds — the aliasing hazard the reservation exists to prevent.

`MultiPool::try_reserve_one` therefore reserves both vectors and then confirms
that both still have room, retrying until they do. Only a reentrant install
consumes reserved room, and it consumes it by publishing a layout of its own,
so the installed layout count strictly increases across attempts and the retry
terminates. The buffers displaced by an abandoned attempt are freed before the
next one begins, keeping that free off the path between the successful
reservation and the pushes.

## The multi pool

`MultiPool::install` follows the same shape across its two directories, and
predates the growth fix. It constructs the layout pool before reserving,
reserves both directories after, re-scans for a pool a nested miss may have
installed for the same layout, re-checks the layout cap, and pushes `pools`
before `layouts` so that `layouts.len() <= pools.len()` holds at every point a
nested call could observe. Its reservation covers two directories at once and
returns two `Displaced` buffers, freed after both pushes, for the reasons given
above.

## Verification

The reentrancy tests drive an allocator that allocates from the pool it serves
from inside `allocate`, with single-slot chunks so that every allocation grows.
They assert that the two allocations land in distinct chunks, that both values
survive intact, and that returning both slots resolves each global index to the
chunk that owns it. A bounded variant asserts the cap re-check holds the pool
to its limit whatever the reentry depth, and another asserts that a pool which
can no longer grow still hands out a slot a nested allocation left free. A
multi-pool variant re-enters from `Clone::clone` instead.

Those tests run under Miri with Tree Borrows, which is what actually rules out
the aliasing hazard; the assertions alone would not catch it. Reentry into the
directory reservation is driven through a custom global allocator instead, and
is therefore excluded from Miri, whose allocator model gives no meaning to a
global allocator forwarding to the system one. That path holds no borrow across
the allocation by construction, which is what the ordering above establishes.

The two doors therefore carry different evidence. Reentry from `A` is checked
under Miri with Tree Borrows and so is verified as an aliasing question.
Reentry from the global allocator is checked only under the ordinary test
runner, where the assertions cover the reachable outcomes — one pool per
layout, the cap respected, room confirmed before every push — but no tool
inspects the borrows. Its aliasing argument rests on the ordering, which keeps
every allocator call outside every borrow, rather than on a checker.
