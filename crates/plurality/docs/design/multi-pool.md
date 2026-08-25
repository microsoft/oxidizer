# Plurality — Multi pool

Pooling values of any type from one pool object: how a value is routed to the
pool serving its layout, how such a pool is sized and bounded, and where it
departs from a typed pool. Part of the [architecture](../DESIGN.md).

## Heterogeneous pooling

A typed pool fixes its element type at construction. A **multi pool** moves
that decision to the call site: one pool object accepts values of any type, and
the type parameter travels with the allocation instead of with the pool.

```rust
let pool = MultiPool::new();
let widget = pool.alloc_box(Widget::new());    // Box<Widget>
let count  = pool.alloc_arc(0_u64);            // Arc<u64>
let name   = pool.alloc_rc(String::new());     // Rc<String>
```

Everything the typed pool promises about a handle continues to hold: address
stability, detachable lifetime, one-pointer width for sized values, and
coercion to trait objects and slices ([handles](./handles.md)). A multi pool is
what lets a single pool back a heterogeneous working set — a scheduler holding
many distinct future types, a scene graph of unrelated node types, or any
collection stored as `Box<dyn Trait>` where the concrete types differ.

## The router and the layout pools

A multi pool owns a set of **layout pools**. A layout pool is the same
machinery as a typed pool with the element type replaced by a layout fixed at
construction: same chunks, same slots, same free list, same pair of reference
counts. The multi pool itself holds no slots and no values — it is a directory
that maps a layout to the pool serving it.

```text
   alloc_box(widget)
        │  Layout::new::<Widget>()  →  (size 24, align 8)
        ▼
   ┌───────────────────────────────┐
   │  multi pool: geometry         │
   │  directory                    │
   │   (0, 4)  →  layout pool ●    │
   │   (8, 8)  →  layout pool ●    │
   │  (24, 8)  →  layout pool ●────┼──► chunks · slots · free list
   └───────────────────────────────┘
        │
        ▼
   Box<Widget>  — one pointer, no reference to the directory
```

Types that share a geometry share a layout pool: `u64` and `i64` draw from the
same slots, and so do two structs of identical size and alignment. This is
memory sharing, not type confusion — each slot holds exactly one value, and the
handle that owns it carries the concrete type. A consequence worth stating
plainly: capacity reported for one type may be occupied by values of another.

## The router sits on the allocation path only

The architecturally important property of this arrangement is what the *free*
path does not do.

Reclamation is pointer recovery ([handles](./handles.md)): a handle walks from
the value's address to its slot, to the chunk header, to the type-agnostic pool
core, using nothing but arithmetic derived from the value's own size and
alignment. That walk arrives directly at the layout pool that owns the slot. It
never asks which pool the value came from, so it never consults the directory.

```text
 allocate:  directory lookup ──► layout pool ──► slot
 free:      value pointer ─────────────────────► layout pool   (no lookup)
```

This is why multi-pool handles are byte-for-byte the typed handles — one pointer for
a sized value, with no layout key, no pool reference, and no per-handle
metadata. It is also why freeing costs exactly what it costs in a typed pool,
and why frees remain concurrent and lock-free while the directory stays
confined to the single allocator thread.

## Exact sizes, no size classes

A multi pool routes on a value's **slot geometry** — the stride and the offsets
of the counter and index — which is a pure function of the value's size and
alignment. It never rounds sizes up, buckets nearby sizes together, or imposes
size classes.

Geometry is not an injective function of layout. Padding a value out to make
room for the trailing counter and index widens any alignment narrower than a
`u32`'s, so `[u8; 8]` and `[u16; 4]` — distinct Rust layouts — describe the
same slot exactly: same stride, same offsets, same chunk shape. **Pool identity
follows geometry**: those two types share one layout pool, with one set of
chunks, one free list, one line in the statistics and one share of any cap.
Merging them costs nothing, because the merged slot is the slot either type
would have been given alone. Sizes never merge, and alignments merge only where
the slot metadata had already forced them together.

Geometry is also the safety invariant that allocation and reclamation rest on:
the allocating pool and the reclaiming handle must agree on it. The handle
recomputes it from the value it holds ([handles](./handles.md)), so a value in
a merged pool recovers the same offsets its own layout implies — which is
precisely why the merge is invisible to it.

The rule pays for itself: a multi pool has no internal fragmentation from
rounding, and a value occupies exactly the space a typed pool would give it.
What it costs is one layout pool per distinct geometry — bounded in practice by
the set of types a program instantiates.

Values of zero size are ordinary participants. Their slots carry only the
counter and the index, which is what a typed pool already does for a
zero-sized element type.

Over-aligned values are ordinary participants too, and pay the same overhead a
typed pool charges them: a slot is padded to the value's alignment, so a value
whose alignment exceeds its size is stored in a slot larger than itself, and a
chunk reserves alignment-sized padding ahead of its first slot. This matches
the typed pool, but a multi pool is likelier to meet such types, so it is worth
stating.

## Sizing chunks by bytes

A typed pool sizes chunks in **slots**, because it knows how big a slot is. A
multi pool serves layouts spanning several orders of magnitude, so a uniform
slot count would make chunk sizes just as uneven — a few hundred bytes for a
pool of small integers and many megabytes for a pool of large aggregates.

A multi pool therefore sizes chunks by a **byte target**. Each layout pool
derives its own slot count by dividing the target by its slot stride, clamped
to the representable slot-count range and rounded down to a power of two
(chunk sizes must remain powers of two so that slot addressing stays
shift-and-mask arithmetic). Small values get many slots per chunk, large values
get few, and every layout commits a comparable amount of memory per growth
step.

A fixed slot count is available for callers who want the typed pool's
predictability. It is a request that every layout starts from, rounded to a
power of two and subject to the clamping below.

## Clamping and effective sizing

The chunk size and chunk cap a multi pool uses for a layout are derived values,
not the configured ones. A single configuration meets many layouts, so a multi
pool clamps rather than rejecting:

- The per-layout chunk size is clamped so that the chunk's own memory layout
  cannot overflow. A layout large enough that the requested slot count would
  overflow it gets fewer slots per chunk.
- The per-layout chunk cap is clamped to the number of chunks the pool's maximum
  slot count permits at that chunk size. The effective cap is the smaller of the
  configured cap and that ceiling.

Both effective values are observable. Per-layout queries report the chunk size
and the chunk cap in force for a named type's layout, in the same way a typed
pool reports the rounded-up chunk size it settled on. A caller that fixes a
large slot count for a large layout, or sets a high chunk cap, reads back the
figures the pool will actually use, and its capacity planning can rest on those
rather than on what it asked for.

## Bounding growth

The chunk cap is **per layout pool**, which means per distinct geometry. Types
that share a geometry share one allowance. Each layout pool independently
refuses to grow past the cap and reports capacity exhaustion, exactly as a
typed pool does.

A per-geometry cap alone does not bound the pool, because the number of
distinct geometries is a property of the whole program's type set — including
types from dependencies — and is not something a caller can enumerate. A multi
pool therefore also accepts a **cap on the number of layout pools**. Once
reached, a request for an unseen geometry reports capacity exhaustion rather
than creating a pool for it. Like the typed pool's chunk cap, it is optional:
growth in both dimensions is unbounded unless the caller says otherwise.

When both caps are set, they bound the pool's memory as layout pools times
chunks times the size of a chunk; with either left open, memory is unbounded in
that dimension. That last factor tracks the byte target only for layouts whose
values are small relative to it. A chunk always holds a minimum number of
slots, so a value large enough to exceed the target on its own gets a chunk
sized by the value rather than by the target; and when a caller fixes the slot
count instead, chunk size is the slot count times the stride, subject to the
clamping above. The bound is therefore in chunks, and converting it to bytes
requires knowing the largest layout the program will present.

An aggregate byte budget shared across layout pools would bound memory in bytes
directly, but it requires cross-pool mutable state that outlives every layout
pool. That is a genuine extension rather than a variation, and is recorded in
[`TODO.md`](../TODO.md).

Because the cap is per layout pool, the *first* allocation of a previously
unseen geometry does not compete for capacity with those already in the pool:
it fails only against the layout cap, never against another pool's chunk cap.

## Memory is monotonic per layout pool

A layout pool is never retired. Once created it lives until the multi pool is
dropped, even after every value it serves has been freed, and chunk memory
is never returned to the allocator. A multi pool's memory is therefore
monotonic: the capacity and the chunk count reported for a layout
are a high-water mark over the pool's lifetime, not a measure of what is in use.

Heterogeneity makes this far more visible than it is in a typed pool. Chunks
hold a minimum number of slots regardless of how large the layout is, so a
single value of a large layout commits that chunk for as long as the pool
lives, and a program that presents many transient layouts accumulates a layout
pool for each one it touches. A program of that shape should scope its multi
pool to the phase that uses those layouts, so that the memory is released when
the pool is dropped.

## Lifetimes and teardown

A multi pool holds one unit of the pool-level reference count on each of its
layout pools, in the same way a typed pool object holds one unit of its own.
Dropping the multi pool drops the directory, releasing one unit from every
layout pool it created. Any layout pool with outstanding detachable handles
survives and tears down when its last handle departs, on whichever thread that
happens to be.

Handles from a multi pool therefore outlive it individually and independently:
two values of different layouts, allocated from the same multi pool, may keep
two separate layout pools alive for different durations.

## Concurrency

The multi pool follows the same single allocator, multiple reclaimers
discipline as the typed pool ([concurrency](./concurrency.md)), and for the
same reason:
allocation mutates the directory and must not overlap with itself, while frees
never touch the directory at all. The pool object may be moved between threads
but not shared, and its handles carry their own thread-mobility rules.

A multi pool is `Send` whenever its allocator is, on the same terms as a typed
pool and for the same reasons. The only bound specific to the multi pool is on
the allocator: because each layout pool owns its own clone of it, and because
layout pools tear down independently, two clones may be in use on two threads
at once — one tearing down a layout pool whose last handle just departed,
another serving the pool object elsewhere. This is exactly what `Send` plus
`Clone` already licenses for any type, so it imposes no new bound; it is stated
because per-layout cloning makes the situation reachable in a way a typed
pool's single allocator instance never is.

Reentrancy reaches a multi pool through the same doors described by the
implementation guide:

- `Allocator::allocate` and `Allocator::deallocate` may allocate from, and free
  into, the pool they serve. This is the door that relies on cold-path
  ordering.
- `Clone::clone` on the multi pool's allocator runs once per new layout pool and
  may re-enter; the install path uses the same ordering.
- Pooled values' destructors and the closures passed to `_with` constructors run
  with no pool state in flight, so they may allocate from and free into the pool
  freely.

An allocator that re-enters unconditionally recurses until the stack is
exhausted; the pool does not bound recursion depth. The ordering details are in
[allocator reentrancy](../implementation/reentrancy.md).

## Allocation surface

The multi pool mirrors the typed pool's allocation surface method for method
([allocation](./allocation.md)). Every handle flavor offers the by-value,
closure, and uninitialized-then-initialize forms, each with a panicking and a
fallible variant, and the shared flavors additionally offer their pinned
constructors. The only change is where the type parameter sits:

```rust
typed.alloc_box(value);          // Pool<Widget>  — type fixed by the pool
multi.alloc_box(value);          // MultiPool     — type inferred from the value
multi.alloc_uninit_box::<Widget>();  // named where it cannot be inferred
let erased: plurality::Box<dyn core::fmt::Display> =
    plurality::Box::unsize(
        multi.alloc_box(1.5_f64),
        plurality::coerce!(dyn core::fmt::Display),
    );
```

Introspection splits into two tiers, because a multi pool has no single slot
size. Aggregate queries report the whole pool — the count of live detachable
allocations (`Box`, `Arc`, and `Rc`), the chunks held, and the layout pools in
play. Lifetime-bound `Alloc` handles can occupy slots, but they do not hold
a pool-level reference and do not contribute to `len`; `is_empty` uses the same
definition, so it is not a physical occupancy test. Per-layout queries are
named for a type and report that type's layout pool: its capacity, its
detachable-allocation count, and the effective chunk size and chunk cap in
force for that layout. Per-layout queries never create a layout pool, so asking
about a type the pool has not yet seen simply reports an empty pool at that
layout's effective sizing. A type that shares its geometry with another reports
that shared pool's figures, so the counts it returns may include values it did
not contribute.

Aggregate queries cost time proportional to the number of layout pools, since
they sum over them. They inherit the typed pool's imprecision under
concurrent frees, and compound it: a sum of independently read counters may
describe a state the pool was never in. They are reporting instruments, not
control-flow inputs.

## How the multi pool differs from a typed pool

| Aspect | Typed pool | Multi pool |
|---|---|---|
| Type parameter | On the pool | On the allocation |
| Handles | Four flavors | The same four |
| Chunk sizing | Slot count | Byte target, or a slot count, clamped per layout |
| Chunk cap | Per pool | Per layout pool, clamped, plus a cap on layout pools |
| Capacity queries | Single tier, constant time | Two tiers; aggregates scale with layout pools |
| Allocator | Held once | Cloned per layout pool, so it must be cloneable |
| First use of a layout | — | Cold path that allocates pool metadata |

Everything else is common: the handle types and their guarantees, coercion to
unsized values, pinning rules, the uninitialized tiers, the error currency, the
panic-versus-`Result` split, and the `Send` rules for both the pool object and
its handles.

Two capabilities are deliberately absent, matching the typed pool. There is no
iteration over pool contents — handles are the only way to reach a value. And
there is no type identity or downcasting: the pool records a layout, not a
type, so a value's concrete type is recovered from its handle or not at all.

Converting a multi pool into a typed pool for one of its layouts, or the
reverse, is likewise not offered. The obstacle is not geometry — both forms
derive the same slot geometry from the same layout, which allocation and
reclamation safety require. It is that a typed pool and a layout pool are
distinct instantiations with their own teardown hooks and their own sizing and
capping policies, so neither can adopt the other's slots without adopting its
configuration and its pool-metadata teardown as well.

## Failure

The multi pool reports the same two failures as the typed pool, and adds no
third.

Capacity exhaustion covers two cases: the layout pool serving the request
cannot grow further, or the request is for an unseen layout and the pool
already holds its maximum number of layouts.

Allocator failure additionally covers the cold path where a previously unseen
layout is encountered and router or layout-pool metadata must be allocated.
That path is fallible end to end, so a failure there is reported rather than
aborting. The error therefore means memory could not be obtained for the
pool's own use — a chunk, directory capacity, or the metadata of a new layout
pool — rather than naming chunks specifically.

## Comparison with `infinity_pool::BlindPool`

The reference design for this feature is `infinity_pool`'s multi pool family.
The capability sets are close; the structural difference is where the
type-to-pool mapping is consulted, and who owns the lock.

Both designs need mutual exclusion to serve several threads, and neither
escapes that. The difference is that `infinity_pool` embeds a lock in the pool
and takes it on every operation including every handle drop, while plurality
hands the choice to the caller: a pool used from several threads is wrapped in
whatever the caller prefers, and only allocation needs to be inside it.
Reclamation stays outside, because a handle can find its own pool without
consulting the directory. The common deployment is therefore a `Mutex<Pool>`
whose critical section covers allocation only, with drops running unlocked and
in parallel — not a single-threaded pool.

The `infinity_pool` column below describes version 0.8.

| | `infinity_pool` 0.8 | plurality |
|---|---|---|
| Pool types | Three, by lifetime discipline | One |
| Handle types | Two per pool type | Four, shared with the typed pool |
| Handle width (sized value) | Three to five words | One word |
| Free path | Locked directory lookup per drop | Pointer recovery, no lookup, no lock |
| Directory | `BTreeMap` behind a mutex or cell | Contiguous scan, allocation path only |
| Locking | Embedded in the pool | Caller's choice, and only around allocation |
| Multithreaded use | Lock taken on allocate and on every drop | Lock taken on allocate; drops run unlocked |
| Value destructors and construction closures | Deadlock or panic if they touch the pool | May allocate from and free into the pool |
| Fallible allocation | Not offered | Full `try_` family |
| Zero-sized values | Rejected at run time | Supported |
| Unsizing to trait objects | Per-trait macro, invoked per crate | Coercion token, no per-trait setup |
| Iteration | Not offered | Not offered |
| Type identity / downcasting | Not offered | Not offered |
| Pinning | Every value pinned | Opt-in pinned constructors |

The row on value destructors and construction closures is about the user code a
pool runs. Allocator `allocate` and `deallocate` callbacks, and allocator
`Clone::clone`, are separate reentry doors supported by the ordering described
in [allocator reentrancy](../implementation/reentrancy.md).

The single-word handle and the lookup-free drop are direct consequences of the
pointer-recovery architecture the typed pool already rests on. Because a handle
can find its own pool, it does not need to carry a key to it, and the directory
never has to be reachable — or lockable — from a destructor. That is also what
makes externalising the lock viable: a design whose free path needed the
directory could not let the caller hold the lock, because every drop would have
to reacquire it.

The router, the layout pools, and the ordering that keeps the directory
consistent are described in
[`implementation/multi-pool.md`](../implementation/multi-pool.md).
