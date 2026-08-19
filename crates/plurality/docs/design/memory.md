# Plurality — Memory

How the pool holds memory and finds its way around it: chunks, slots, and the
free list that connects the allocating thread to the freeing ones. Part of the
[architecture](../DESIGN.md).

## Memory layout

Memory is acquired in **chunks** — power-of-two-sized batches of slots. Each
chunk is a single allocation laid out as a small header followed by its array of
slots:

```text
 chunk:  ┌────────┬─────┬────────┬────────┬─────┬──────────┐
         │ header │ pad │ slot 0 │ slot 1 │ ... │ slot N-1 │
         └────────┴─────┴────────┴────────┴─────┴──────────┘
              │                 ▲
              │ back-reference  │ each slot: value + refcount + in-chunk index
              ▼                 (fixed stride, so addressing is pure arithmetic)
        shared pool state
```

Two properties of this layout are load-bearing:

- **Chunks never move and are never individually freed.** They live until the
  entire pool tears down. That means a chunk header can hold a plain
  back-reference to the shared pool state with no risk of a dangling pointer and
  no reference cycle.
- **Slot addressing is arithmetic, not lookup.** Because chunk size is a power of
  two and slot stride is fixed, mapping a global slot index to chunk-and-offset
  is shift/mask arithmetic, and stepping from a value's address back to its
  chunk header is fixed-offset arithmetic. No per-object bookkeeping table is
  consulted on the hot path.

Capacity is monotonic. A pool acquires chunks as it grows and releases them all
at teardown, so it neither shrinks nor compacts, and the capacity it reports is
a high-water mark rather than a measure of what is in use. Growth stops at the
configured chunk cap or, for an unbounded pool, at the ceiling the slot index
imposes: indices are 32 bits wide, and on targets with narrower pointers the
pool-level reference count is the tighter bound. The equivalent statement for a
pool serving many layouts is in [multi pool](./multi-pool.md).

### The slot and its dual-purpose counter

Each slot holds three things: storage for the value, a small counter, and its
own immutable index within the chunk. The counter is **contextual** — it means
different things depending on whether the slot is occupied or free:

- **Occupied by a shared handle:** it is the value's reference count (how many
  shared handles point at it).
- **Occupied by a unique handle:** it is not read; the stale free-list link is
  overwritten when the slot is freed.
- **Free:** it is a link — the index of the next free slot in the free list.

The count and link roles never collide because a shared slot is only read as a
count and a free slot is only read as a link. The slot's stored in-chunk index
is what makes single-pointer recovery
possible: from a bare value pointer, the pool can find the index, step back to
the chunk header, and from there reach the shared pool state — all without the
handle carrying any extra data. The walk that exploits this is described in
[handles](./handles.md).

## The free list

Free slots are threaded together into a **lock-free stack** whose links live
inside the slots themselves (reusing the dual-purpose counter). This is the
concurrency hand-off point:

- **Popping** a slot happens only on the single allocator thread, so there is
  exactly one free-slot consumer. This eliminates the classic ABA hazard by
  construction — a free slot is never simultaneously popped by two threads or
  re-pushed while still free — so the design carries neither tag counters nor
  hazard pointers.
- **Pushing** a freed slot can happen on any reclaimer thread. Free-slot
  producers race only on the head of the stack, resolved with a compare-and-swap
  retry loop.

There is **no growth lock**: adding a chunk, extending the directory, and
splicing the new slots onto the free list all run on the sole allocator thread,
racing only against concurrent free-slot producer pushes at the head.

Growth is a **cold, rare path**. When the free list is empty, the allocator
reserves one slot from a freshly acquired chunk for the immediate request and
splices the remainder onto the free list in one step. Handing back the reserved
slot directly — rather than looping back to re-pop — keeps the grow-then-allocate
path bounded, with no window where a lost race could re-empty the list.

The threading rules that make one free-slot consumer and many free-slot
producers the operative model are stated in [concurrency](./concurrency.md).
The formulas behind slot stride and chunk layout are given in
[`implementation/geometry.md`](../implementation/geometry.md), and the
structures themselves in
[`implementation/pool-body.md`](../implementation/pool-body.md).
