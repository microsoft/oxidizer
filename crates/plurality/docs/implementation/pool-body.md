# The pool body

This document covers the machinery shared by both pool forms: the core data
structures, the slot lifecycle, the chunk directory, growth, pointer recovery,
the two reference counts, teardown, construction, failure and statistics. Back
to the [implementation hub](../IMPLEMENTATION.md).

## Core data structures

**`PoolCore`** is the type-erased shared state. It is the only part of the pool
a reclaiming handle reaches, and it carries exactly what reclamation needs:

```rust
#[repr(C)]
pub(crate) struct PoolCore {
    pub(crate) free_head: AtomicU32,        // FREE_END when the pool must grow
    pub(crate) pool_refcount: AtomicUsize,  // pool object + detached allocations
    pub(crate) teardown: unsafe fn(NonNull<Self>),
}
```

**`PoolInner<A, G>`** is the concrete heap state, generic over the allocator `A`
and the geometry provider `G`:

```rust
#[repr(C)]
pub(crate) struct PoolInner<A, G> {
    pub(crate) core: PoolCore,        // first field — see below
    pub(crate) me: NonNull<PoolCore>, // this allocation's own address
    pub(crate) chunk_size: u32,       // slots per chunk, a power of two
    pub(crate) shift: u32,            // log2(chunk_size)
    pub(crate) mask: u32,             // chunk_size - 1
    pub(crate) max_chunks: Option<u32>,
    pub(crate) chunks_allocated: AtomicU32,
    #[cfg(feature = "stats")]
    pub(crate) bytes_allocated: AtomicUsize,
    pub(crate) chunk_layout: Layout,
    pub(crate) directory: UnsafeCell<Vec<NonNull<ChunkHeader>>>,
    pub(crate) allocator: A,
    pub(crate) geometry: G,
}
```

`#[repr(C)]` with `core` first is load-bearing on both structures. A chunk
header stores a `NonNull<PoolCore>` obtained by casting the *full* inner
pointer, so the pointer's provenance covers the whole allocation; the teardown
callback stored inside `core` casts it back to the concrete `PoolInner` it was
monomorphized for. Neither cast is valid without the guaranteed field order.
`me` is where that pointer comes from; see "Pointer recovery" below.

`chunk_layout` is the layout of one whole chunk, computed once at construction
from the geometry and the chunk size and stored rather than recomputed, because
it is needed on the growth path and again at teardown, which may run on a
thread that never held the pool object.

**`Pool<T, A>`** is one pointer, `NonNull<PoolInner<A, TypedGeometry<T>>>`.
`MultiPool` and `LayoutPool` are described in
[the multi pool](./multi-pool.md).

**`ChunkHeader`** sits at the start of every chunk allocation:

```rust
#[repr(C)]
pub(crate) struct ChunkHeader {
    pub(crate) pool: NonNull<PoolCore>,
    pub(crate) base_index: u32,   // chunk_index * chunk_size
    pub(crate) chunk_index: u32,
}
```

`base_index` lets a recovered slot compute its global index without consulting
the directory, which is what allows the free path to run without touching
allocation-path state.

**`SlotCell<T>`** is the compiler's rendering of the slot geometry:

```rust
#[repr(C)]
pub(crate) struct SlotCell<T> {
    pub(crate) value: UnsafeCell<MaybeUninit<T>>,
    pub(crate) refcount: AtomicU32,
    pub(crate) index: u32,
}
```

The value is field 0, so the slot's address and the value's address are the
same number. Every detachable handle stores the value pointer and recovers the
other two fields by offset. The `index` field is written once during chunk
initialization and never mutated, which is what makes the walk back to the
chunk header a pure subtraction.

The counter is context-typed. While the slot is occupied it is the shared
handles' reference count; while the slot is free it is the next free *global*
slot index, or the `FREE_END` sentinel (`u32::MAX`). The two roles never
overlap in time, so one word serves both. `MAX_POOL_SLOTS` bounds the number of
slots a pool may hold so that every global index stays below the sentinel; on
targets with pointers narrower than 64 bits the pool-level reference count is
the tighter bound and supplies the ceiling instead.

Access to the value goes through a small set of `unsafe` accessors on
`SlotCell` — read, mutate, write and drop — so that the `UnsafeCell` and
`MaybeUninit` handling lives in one place rather than at every call site.

## Geometry parameterisation

`PoolInner` carries a geometry provider rather than an element type, and three
paths are expressed in terms of it:

- **Slot addressing.** Stepping to a slot is `base + slots_offset + offset *
  stride`, and stepping back to a chunk header is its inverse. For the typed
  geometry the stride and the offset are constants and the emitted code is what
  pointer arithmetic over a typed slot pointer produces.
- **Growth.** Growth writes the two metadata words at their computed offsets
  and leaves the value storage uninitialized, without naming a value type.
- **Teardown.** The teardown hook monomorphises over the geometry provider and
  the allocator. Under `loom`, where teardown must drop the instrumented atomic
  in each slot, it finds that atomic at the geometry's reference-count offset.
  Teardown therefore has no element-type dependence at all, which matters
  because it may run long after the pool object is gone.

Everything else in this document is common to both geometry providers.

## The slot lifecycle

**Claiming a slot** pops the free list. The head is loaded with `Acquire`; the
sentinel means the pool must grow. Otherwise the head index is resolved to a
slot address, the slot's counter word is read to obtain the next index, and the
head is swapped for it with a compare-exchange. This is a plain compare-exchange
loop with no ABA tag, which is sound because pops happen only on the allocator
thread. In the free-slot flow, reclaiming handles are the producer side and can
only ever install a different head, which the loop retries.

**Occupying a claimed slot** differs by handle flavor, and the differences are
deliberate:

| Handle | Slot counter | Pool refcount | Value |
|---|---|---|---|
| `Arc`, `Rc` | stored as `1` | incremented | written |
| `Box` | left as-is | incremented | written |
| `Alloc` | left as-is | untouched | written |

`Box` and `Alloc` never read the slot counter, so initializing it would be
wasted work; the stale free-list link stays in the word until the slot is
pushed back onto the free list, which overwrites it. `Alloc` skips the pool
reference count because its `'pool` borrow already proves the pool outlives it.

**Returning a slot** pushes it back onto the free list:

```text
loop {
    head = free_head.load(Relaxed)
    slot.refcount.store(head, Relaxed)     // link this slot to the old head
    if free_head.compare_exchange_weak(head, global_index, Release, Relaxed) { break }
}
```

The load needs only a recent head — a stale one makes the compare-exchange
retry — so it is `Relaxed`. The `Release` on the swap publishes the link store
to the allocator's `Acquire` load on the pop side, which is what makes the
popped slot's next-index read valid.

The value's destructor runs before the push, and it may unwind. RAII guards
therefore own the return: the guard pushes the slot in its own `Drop`, so a
panicking destructor loses the value but not the slot.

## The chunk directory

The directory is a `Vec<NonNull<ChunkHeader>>` indexed by chunk number, held
behind an `UnsafeCell` because allocation takes `&self`. It is written only on
the allocator thread and read there on the pop path; teardown reads it once the
pool is quiescent. `!Sync` is the gate that makes this sound.

A global slot index decomposes into a chunk number and an in-chunk offset by
shifting and masking:

```text
chunk_no = global >> shift
offset   = global & mask
slot     = directory[chunk_no] + slots_offset + offset * stride
```

`shift` and `mask` are derived from the chunk size at construction, which is
why the chunk size is rounded up to a power of two. The directory lookup is an
unchecked index: the index came from the free list, and only indices belonging
to allocated chunks are ever on it.

## Growth

Growth runs only when the free list is empty, only on the allocator thread, and
is marked cold and never-inlined so that the allocation path stays a straight
line.

Chunk size never varies within a pool: every chunk holds exactly `chunk_size`
slots, so a chunk's first global index is `chunk_index * chunk_size` and the
index arithmetic above stays exact. There is no geometric growth. The rationale
is that doubling would buy a shorter directory at the cost of turning slot
addressing from a shift and a mask into a search, and the directory is never
long enough for its length to matter.

The chunk cap is the caller's `max_chunks`, or, for an unbounded pool, the
chunk count that keeps every global index below the sentinel. Reaching it
reports capacity exhaustion; a failing allocator reports allocator failure.

A new chunk is fully initialized before it is published. The header is written
first, then every slot's counter is set to the *next* global index, which
pre-threads the whole chunk as a free chain. The chunk is then pushed into the
directory, and only afterwards is the chunk count published with a `Release`
store, which teardown pairs with. The push runs into capacity reserved before
the chunk was allocated, so it neither allocates nor fails; ownership of the
chunk passes to the pool at that point, and every earlier exit deallocates it
explicitly. Ref: [allocator reentrancy](./reentrancy.md).

Finally, the chunk's slots from the second onwards are spliced onto the free
list in one compare-exchange, and the first slot is handed to the caller
without ever being published. Handing over an unpublished slot is what keeps
growth from racing a concurrent free for the slot it just created.

## Chunk sizing

A typed pool takes a slot count directly. A multi pool sizes chunks from a byte
target, because one slot count across layouts spanning orders of magnitude
would make chunk sizes just as uneven:

```text
slots = clamp(target_bytes / stride, MIN_SLOTS, MAX_SLOTS)
slots = largest power of two not exceeding slots
```

Rounding to a power of two is required, not cosmetic: mapping a global slot
index to a chunk and an offset is shift-and-mask arithmetic on the hot
allocation path. Because the lower clamp bound is itself a power of two,
rounding cannot push the result below it.

The lower bound keeps every chunk usable even when one value is larger than the
byte target. The upper bound keeps power-of-two rounding representable and the
slot-index arithmetic within its supported range. The default target is a
small multiple of a page: large enough that small values grow rarely, small
enough that a layout touched once does not cost much.

A caller may instead request a slot count, which every layout starts from
uniformly before power-of-two rounding and per-layout clamping. The per-layout
clamps that keep a derived size legal are described in
[the multi pool](./multi-pool.md).

## Pointer recovery

Reclamation starts from a value pointer and ends at `PoolCore`, using nothing
but the value's own size and alignment:

```text
index    = *(value + index_off)              as u32
refcount = &*(value + refcount_off)          as &AtomicU32
header   = &*(value - index * stride - slots_off)
core     = header.pool
global   = header.base_index + index
```

The offsets are the formulas in [slot geometry](./geometry.md), evaluated over
`size_of_val` and `align_of_val` of the value being freed. Because the walk
never names `SlotCell<T>`, it works for unsized values, for which no such type
can be written; and because `PoolInner` and `ChunkHeader` layouts do not depend
on the element type, the header read is valid whatever the value is.

For a sized value the runtime size and alignment fold to the same constants the
compiler used, so the arithmetic collapses to the identical offsets and the
sized and unsized paths agree by construction rather than by convention.

`drop_and_free_val` is the entry point. It reads the size and alignment
*before* running the destructor, because afterwards the value is gone and its
metadata with it. When the value type needs no destructor the guard is skipped
entirely; otherwise the guard is armed, the value is dropped in place, and the
guard pushes the slot. A second entry point recovers only the reference count,
for the shared handles' clone and drop paths, using the same offset formula.

The bound owner is the exception: it stores a `NonNull<SlotCell<T>>` and uses a
monomorphized path that reads the slot through the compiler's layout. See
[handles](./handles.md) for why that consumer is kept independent.

The core pointer in a chunk header is the one link in that walk that cannot be
derived on demand. It ends up freeing the pool allocation when the last handle
outlives the pool object, and a pointer taken from a `&self` borrow permits
only reads and interior-mutable writes — deallocating through one is undefined
behaviour, which Miri reports as a borrow-stack violation at teardown. Each
pool therefore records its own address once at construction, from the pointer
its allocation was created with, and every chunk header copies that value.

The modules carrying this arithmetic opt out of the lint against multiple
unsafe operations per block, with the rationale recorded at each module head:
one recovery step is not independently meaningful, and splitting the walk into
one block per dereference would multiply the safety comments without adding
information to any of them.

## The two reference counts

**The slot counter** owns the value for shared handles. `Arc` increments it
with `Relaxed`, since a clone requires an existing reference, and decrements
with `Release`; the handle that observes the previous value `1` issues an
`Acquire` fence and then destroys the value, which is the standard pattern
that makes every prior write to the value visible to the destructor. `Rc`
performs the same protocol non-atomically through the atomic's raw pointer,
which is sound because `Rc` is unconditionally `!Send` and `!Sync`, and needs
no fence for the same reason. Under `loom` those accesses go through
instrumented atomic operations instead, because the model checker cannot see
raw-pointer traffic. The counter is guarded against overflow at the same
ceiling `alloc::sync::Arc` uses; exceeding it aborts.

**The pool-level reference count** owns the memory. It starts at one for the
pool object and gains one unit per live detachable allocation. `Alloc` handles
are deliberately excluded: their borrow already proves the pool outlives them,
so counting them would pay for a fact the type system has established.
Releasing a unit uses `Release`, and the thread that observes the previous
value `1` issues an `Acquire` fence and runs teardown.

The live-value query reads this counter and subtracts the pool object's own
unit, which is why it is documented as approximate under concurrent frees and
as excluding bound owners.

## Teardown

`PoolCore` stores teardown as an `unsafe fn(NonNull<PoolCore>)`, populated at
construction with the callback monomorphized for this pool's geometry and
allocator. Whichever owner of a pool-level unit — the pool object or the final
handle of a detachable allocation — drops the count to zero calls it through
that pointer. A handle therefore tears the pool down without naming the
geometry, without naming the allocator, and without reaching the pool object,
which may have been dropped long before and on another thread.

Teardown itself begins by loading the chunk count with `Acquire`, pairing with
the `Release` store in growth; the pool-level reference count's increments are
`Relaxed` and do not by themselves make the directory visible to a thread that
did not allocate. It then walks the directory, deallocating each chunk with the
stored chunk layout, and finally reconstitutes the metadata allocation as a box
and drops it.

That last step runs the pool's own drop glue — the directory vector's buffer,
the allocator instance, and under `loom` the instrumented atomics in the core —
which a bare deallocation would skip. The leak would escape the
allocation-tracking tests, which observe the pool's own allocator rather than
the global one, so it is asserted separately. A metadata block obtained raw
rather than from a box (see [Failure](#failure)) is reclaimed by the same step,
because both come from the global allocator with the same layout.

Teardown never reads or drops element storage. Every value was destroyed by its
own handle before that handle released its unit of the count, so a pool that
reaches teardown holds no live values by construction.

Under `loom`, teardown additionally drops each slot's instrumented atomic
before deallocating the chunk, because loom tracks those objects and reports
them as leaked otherwise. The same loop appears on the growth path's failure
exits, which deallocate a chunk the pool never took ownership of. Both compile
to nothing outside `loom`.

## Concurrency discipline

`Pool` is `Send` when its allocator is, and is never `Sync`. The architecture is
single allocator, multiple reclaimers. Every piece of cross-thread state is
atomic; the one piece that is not — the chunk directory — is confined to the
`&self` operations on the pool object by the absence of `Sync`. That excludes
another thread but not a reentrant allocator on this one, which the growth
ordering handles rather than forbids. Ref:
[allocator reentrancy](./reentrancy.md).

"The allocator thread" means whichever single thread holds a shared reference
to the pool at a given moment. Because the pool is `!Sync`, such a reference
cannot be shared across threads, so the free-list pop, the directory read on
the addressing path, and the directory write during growth cannot overlap with
themselves. Frees run from any thread, are lock-free, and touch only atomics and
immutable chunk state. In producer/consumer terms, allocated-value flow is
single-producer/multi-consumer, while free-slot flow is
multi-producer/single-consumer.

`Send` deliberately carries no `T: Send` bound. A pool object owns no values:
values are owned by handles, whose own `Send` bounds govern where they may
travel. The pool exposes no iteration or drain, so a thread receiving a pool
has no route to a value another thread placed in it, and teardown deallocates
chunks without reading or dropping element storage. The bound would therefore
constrain the pool for a hazard the pool cannot express, and a dedicated test
suite pins that argument down.

The design-level statement of this discipline is in
[the concurrency model](../design/concurrency.md).

## Construction

`PoolBuilder<T, A>` collects a chunk size, an optional chunk cap and an
allocator; the allocator setter changes the builder's type parameter, so the
allocator type is inferred rather than annotated. The builder is reached from
the pool type rather than constructed directly, per the workspace's builder
convention.

`build()` validates by assertion, treating invalid sizing as caller error: the
chunk size must be at least one and small enough that rounding it up to a power
of two cannot overflow, and the product of the rounded chunk size and the chunk
cap must fit the addressable slot ceiling. The chunk layout is computed here
too, so a layout that cannot exist is rejected at construction rather than at
first use.

The rounded chunk size yields `shift` and `mask`. The free list starts at the
sentinel, the pool reference count starts at one, and the directory starts
empty, so a freshly built pool holds no chunks and does not call its configured
chunk allocator until first use.

The two pool forms obtain their metadata block differently, because their
failure contracts differ. The typed builder allocates it with `Box::new`, so a
failure to obtain it is handled by the global allocator on its own terms;
`build()`'s documentation promises a panic for invalid configuration and says
nothing about how an allocation failure is disposed of. `LayoutPool::new`
allocates the same block through the raw global allocator and returns
`Result`, because the multi pool's cold path must report a metadata failure as
an `AllocError`.

## Failure

`AllocError` is a `Copy` newtype over a private enum with two cases, so that
the discriminants are not part of the public API and can be matched only
through predicate methods:

- **Capacity exhausted** — every slot is occupied and the pool cannot grow,
  because it reached its chunk cap or the addressable slot ceiling. For a multi
  pool it also covers a request for an unseen layout when the layout cap is
  reached.
- **Allocator failure** — memory for the pool's own use could not be obtained.
  That covers a chunk, the chunk directory that indexes it, and, on the multi
  pool's cold path, the layout directory and the metadata of a new layout pool.
  These allocator failures share a case because the caller's recourse is
  identical, and because a third case would be a breaking change to an error
  type callers match on.

Failures are handled in three ways. The fallible allocation family returns
`Result`. The panicking family routes the same error through one cold function
that panics, so the panic site is shared and the allocation paths stay small.
Reference-count overflow aborts through a deliberate double panic so that it
works without `std`. Global-allocator out-of-memory handling on paths that are
not represented as `AllocError` follows the global allocator's own behavior.

Panic safety on the allocation path comes from ordering: the fallible
`_with` constructors obtain an uninitialized handle first and run the caller's
closure afterwards, so if the closure panics the handle's own drop returns the
slot and no capacity leaks.

## Statistics

Structural queries — chunk size, chunk cap, chunks allocated, capacity, live
count, availability — are always compiled in and read the fields above
directly. The live count and availability are derived from the pool-level
reference count and are therefore approximate under concurrent frees and blind
to bound owners; they are reporting instruments, not control-flow inputs.

The `stats` feature adds a cumulative byte counter to `PoolInner` and a
`stats()` accessor returning a `#[non_exhaustive]` snapshot of cumulative
chunks and bytes. Both counters are monotonic, because chunks are retained
until teardown. The counter is incremented with `Relaxed`: it is read only
through `stats()` and never used to establish a happens-before relationship.
Compiling it out entirely when the feature is off is the point of the gate —
the pool carries no tracking overhead a caller has not asked for.

The multi pool's aggregate queries sum over its layout pools; see
[the multi pool](./multi-pool.md).

## Compilation configuration

| Configuration | Effect |
|---|---|
| `std` (default) | Forwards to the allocator-abstraction dependency's `std` feature. The crate itself is `no_std` regardless. |
| `stats` | Compiles in the cumulative counters and `PoolStats`. |
| `loom` | A marker feature whose only job is to select the model-checking test target through `required-features`; the instrumented build is driven by `--cfg loom`. |
| `--cfg loom` | Swaps every atomic for loom's instrumented equivalent, switches the non-atomic shared-handle path to instrumented operations, and enables the teardown drops loom's leak checking requires. |
| `--cfg docsrs` | Enables the feature annotations on gated items. |
| `--cfg coverage_nightly` | Enables the attribute that excludes genuinely unreachable paths from coverage. |
