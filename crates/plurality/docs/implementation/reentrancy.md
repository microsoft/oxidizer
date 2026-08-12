# Reentrancy

The pool is an interior-mutable container that calls out to an allocator while
holding state no observer is fit to see. This document describes how the
implementation makes reentrancy safe by construction rather than by asking the
caller not to attempt it.

## The protected windows

Two windows exist, one per allocator the crate reaches.

**Chunk growth** (`PoolInner::grow`, `pool.rs`) reads the chunk count, derives
the new chunk's global slot-index range from it, then calls the pool's
configured allocator `A`, and only publishes the incremented count once the
chunk is initialized and in the directory. A nested growth entering between the
read and the store would derive the same base index a second time, and the two
chunks would then claim one set of global slot indices — the free list would
link a slot to a different slot's storage. The window also spans the directory
`push`, which reaches the global allocator with a `&mut Vec` live.

**Directory reservation** (`BlindPool::try_reserve_one`, `blind_pool.rs`)
reserves room in the router's two vectors before a layout pool is installed.
Each reservation holds `&mut` over a vector while calling the global allocator,
and may free the vector's old buffer. A nested access to either vector would
alias that mutable borrow, and a read of the reallocated vector would address
freed memory.

## The latch

`ReentrancyLatch` (`reentrancy.rs`) is a single cell holding whether a protected
window is in progress. `enter` claims the window and yields a token whose `Drop`
releases the claim, so an unwinding panic cannot leave the latch stuck. The
pool's `!Sync` bound confines each window to one thread at a time, so no atomic
is needed and the cost is a predictable branch on a path that already allocates.

Each protected window has its own latch, held by the state it protects: growth
by `PoolInner`, reservation by `BlindPool`. Two layout pools of a blind pool
therefore never contend, and routing between layouts is unaffected.

An entry that finds the latch held is refused:

- A fallible allocation returns `AllocError::ALLOCATOR_FAILED`. The pool cannot
  serve the request, and the caller's existing failure handling applies.
- Introspection that reads the layout directory panics, since it has no error
  to report. `BlindPool::assert_readable` performs that check.

The router refuses at `pool_for`, before its first directory read, which covers
`lookup` and `install` in one branch off the allocation path.

## Keeping callbacks out of borrows

Rejecting a nested entry protects the two windows. Everything else the pool does
avoids creating such a window in the first place, by never running extensible
code while a borrow into pool state is live.

The router's cold path is ordered so that every step that releases control —
constructing a layout pool, dropping a rejected value, cloning the allocator —
runs with no directory borrow held; `blind-pool.md` derives that ordering step
by step.

The introspection helpers follow the same rule. `LayoutPoolRef` is a `Copy`,
borrow-free view of a layout pool, valid independently of the directory because
a layout pool's state is heap-allocated, never moves and is never retired.
`sum_pools` copies one view out per index and releases the directory borrow
before invoking the callback; `with_layout_of` does the same for a single
layout. A future callback that allocates from the pool therefore cannot alias a
directory borrow, whatever it captures.

## Verification

`tests/reentrant_allocator.rs` drives an allocator that allocates from the pool
it serves, covering both windows and both refusal modes.
