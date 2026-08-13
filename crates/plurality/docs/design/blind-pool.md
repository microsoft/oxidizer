# Plurality — Blind pool

Pooling values of any type from one pool object: how a value is routed to the
pool serving its layout, how such a pool is sized and bounded, and where it
departs from a typed pool. Part of the [architecture](../DESIGN.md).

## Heterogeneous pooling

A typed pool fixes its element type at construction. A **blind pool** moves
that decision to the call site: one pool object accepts values of any type, and
the type parameter travels with the allocation instead of with the pool.

```rust
let pool = BlindPool::new();
let widget = pool.alloc_box(Widget::new());    // Box<Widget>
let count  = pool.alloc_arc(0_u64);            // Arc<u64>
let name   = pool.alloc_rc(String::new());     // Rc<String>
```

Everything the typed pool promises about a handle continues to hold: address
stability, detachable lifetime, one-pointer width for sized values, and
coercion to trait objects and slices ([handles](./handles.md)). A blind pool is
what lets a single pool back a heterogeneous working set — a scheduler holding
many distinct future types, a scene graph of unrelated node types, or any
collection stored as `Box<dyn Trait>` where the concrete types differ.

## The router and the layout pools

A blind pool owns a set of **layout pools**. A layout pool is the same
machinery as a typed pool with the element type replaced by a layout fixed at
construction: same chunks, same slots, same free list, same pair of reference
counts. The blind pool itself holds no slots and no values — it is a directory
that maps a layout to the pool serving it.

```text
   alloc_box(widget)
        │  Layout::new::<Widget>()  →  (size 24, align 8)
        ▼
   ┌───────────────────────────────┐
   │  blind pool: layout directory │
   │   (0, 1)  →  layout pool ●    │
   │   (8, 8)  →  layout pool ●    │
   │  (24, 8)  →  layout pool ●────┼──► chunks · slots · free list
   └───────────────────────────────┘
        │
        ▼
   Box<Widget>  — one pointer, no reference to the directory
```

Types that share a layout share a layout pool: `u64` and `i64` draw from the
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

This is why blind handles are byte-for-byte the typed handles — one pointer for
a sized value, with no layout key, no pool reference, and no per-handle
metadata. It is also why freeing costs exactly what it costs in a typed pool,
and why frees remain concurrent and lock-free while the directory stays
confined to the single allocator thread.

## Exact layouts, no size classes

A blind pool routes a value only to the pool whose layout is *exactly*
`Layout::new::<T>()`. It never rounds sizes up, buckets nearby layouts
together, or imposes size classes.

Allocation and reclamation also rely on a separate safety invariant: the
allocating pool and the reclaiming handle agree on **slot geometry** — the
stride and the offsets of the counter and index. The reclaiming handle
recomputes those from the value it holds, so the geometry must match the slot
that was allocated for that value.

Geometry is a pure function of layout, but not an injective one. Padding a
value out to make room for the counter and index collapses neighboring
layouts together: `u8`, `u16`, `u32` and `[u8; 4]` are four distinct layouts
with one geometry between them. The directory still uses the exact Rust
`Layout` as its key. **Pool identity follows layout, not geometry**: those four
types get four layout pools, not one, and each has its own chunks, its own free
list, its own statistics, and its own share of any cap. Geometry appears only
in the safety invariant above; it never partitions anything.

The rule pays for itself: a blind pool has no internal fragmentation from
rounding, and a value occupies exactly the space a typed pool would give it.
What it costs is one layout pool per distinct layout — bounded in practice by
the set of types a program instantiates.

Values of zero size are ordinary participants. Their slots carry only the
counter and the index, which is what a typed pool already does for a
zero-sized element type.

Over-aligned values are ordinary participants too, and pay the same overhead a
typed pool charges them: a slot is padded to the value's alignment, so a value
whose alignment exceeds its size is stored in a slot larger than itself, and a
chunk reserves alignment-sized padding ahead of its first slot. This matches
the typed pool, but a blind pool is likelier to meet such types, so it is worth
stating.

## Sizing chunks by bytes

A typed pool sizes chunks in **slots**, because it knows how big a slot is. A
blind pool serves layouts spanning several orders of magnitude, so a uniform
slot count would make chunk sizes just as uneven — a few hundred bytes for a
pool of small integers and many megabytes for a pool of large aggregates.

A blind pool therefore sizes chunks by a **byte target**. Each layout pool
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

The chunk size and chunk cap a blind pool uses for a layout are derived values,
not the configured ones. A single configuration meets many layouts, so a blind
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

The chunk cap is **per layout pool**, and because routing is by exact layout,
that means per distinct layout — not per geometry. Types that share a geometry
but differ in layout hold separate pools and separate allowances. Each layout
pool independently refuses to grow past the cap and reports capacity
exhaustion, exactly as a typed pool does.

A per-layout cap alone does not bound the pool, because the number of distinct
layouts is a property of the whole program's type set — including types from
dependencies — and is not something a caller can enumerate. A blind pool
therefore also accepts a **cap on the number of layouts**. Once reached, a
request for an unseen layout reports capacity exhaustion rather than creating a
pool for it. Like the typed pool's chunk cap, it is optional: growth in both
dimensions is unbounded unless the caller says otherwise.

When both caps are set, they bound the pool's memory as layouts times chunks
times the size of a chunk; with either left open, memory is unbounded in that
dimension. That last factor tracks the byte target only for layouts whose
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

Because the cap is per layout, the *first* allocation of a previously unseen
layout does not compete for capacity with layouts already in the pool: it fails
only against the layout cap, never against another layout's chunk cap.

## Memory is monotonic per layout

A layout pool is never retired. Once created it lives until the blind pool is
dropped, even after every value of its layout has been freed, and chunk memory
is never returned to the allocator. A blind pool's memory is therefore
monotonic per layout: the capacity and the chunk count reported for a layout
are a high-water mark over the pool's lifetime, not a measure of what is in use.

Heterogeneity makes this far more visible than it is in a typed pool. Chunks
hold a minimum number of slots regardless of how large the layout is, so a
single value of a large layout commits that chunk for as long as the pool
lives, and a program that presents many transient layouts accumulates a layout
pool for each one it touches. A program of that shape should scope its blind
pool to the phase that uses those layouts, so that the memory is released when
the pool is dropped.

## Lifetimes and teardown

A blind pool holds one unit of the pool-level reference count on each of its
layout pools, in the same way a typed pool object holds one unit of its own.
Dropping the blind pool drops the directory, releasing one unit from every
layout pool it created. Any layout pool with outstanding detachable handles
survives and tears down when its last handle departs, on whichever thread that
happens to be.

Handles from a blind pool therefore outlive it individually and independently:
two values of different layouts, allocated from the same blind pool, may keep
two separate layout pools alive for different durations.

## Concurrency

The blind pool follows the same single allocator, multiple reclaimers
discipline as the typed pool ([concurrency](./concurrency.md)), and for the
same reason:
allocation mutates the directory and must not overlap with itself, while frees
never touch the directory at all. The pool object may be moved between threads
but not shared, and its handles carry their own thread-mobility rules.

A blind pool is `Send` whenever its allocator is, on the same terms as a typed
pool and for the same reasons. The only bound specific to the blind pool is on
the allocator: because each layout pool owns its own clone of it, and because
layout pools tear down independently, two clones may be in use on two threads
at once — one tearing down a layout pool whose last handle just departed,
another serving the pool object elsewhere. This is exactly what `Send` plus
`Clone` already licenses for any type, so it imposes no new bound; it is stated
because per-layout cloning makes the situation reachable in a way a typed
pool's single allocator instance never is.

Because reclamation never enters the directory, a destructor running on a
pooled value may freely allocate from, or free into, the same blind pool. There
is no lock to re-enter and no directory borrow held across user code. The same
freedom extends to construction closures and to `Clone::clone` on the blind
pool's allocator. It does not extend to `allocate` or `deallocate` on the
pool's allocator or the global allocator, which the
[invariant list](../DESIGN.md#design-invariants-at-a-glance) requires not to
re-enter a plurality pool; a blind pool reaches both at more points than a
typed pool does.

## Allocation surface

The blind pool mirrors the typed pool's allocation surface method for method
([allocation](./allocation.md)). Every handle flavor offers the by-value,
closure, and uninitialized-then-initialize forms, each with a panicking and a
fallible variant, and the shared flavors additionally offer their pinned
constructors. The only change is where the type parameter sits:

```rust
typed.alloc_box(value);          // Pool<Widget>  — type fixed by the pool
blind.alloc_box(value);          // BlindPool     — type inferred from the value
blind.alloc_uninit_box::<Widget>();  // named where it cannot be inferred
let erased: plurality::Box<dyn core::fmt::Display> =
    plurality::Box::unsize(
        blind.alloc_box(1.5_f64),
        plurality::coerce!(dyn core::fmt::Display),
    );
```

Introspection splits into two tiers, because a blind pool has no single slot
size. Aggregate queries report the whole pool — the count of live detachable
allocations (`Box`, `Arc`, and `Rc`), the chunks held, and the distinct layouts
in play. Lifetime-bound `Alloc` handles can occupy slots, but they do not hold
a pool-level reference and do not contribute to `len`; `is_empty` uses the same
definition, so it is not a physical occupancy test. Per-layout queries are
named for a type and report that type's layout pool: its capacity, its
detachable-allocation count, and the effective chunk size and chunk cap in
force for that layout. Per-layout queries never create a layout pool, so asking
about a type the pool has not yet seen simply reports an empty pool at that
layout's effective sizing.

Aggregate queries cost time proportional to the number of layouts, since they
sum over the layout pools. They inherit the typed pool's imprecision under
concurrent frees, and compound it: a sum of independently read counters may
describe a state the pool was never in. They are reporting instruments, not
control-flow inputs.

## How the blind pool differs from a typed pool

| Aspect | Typed pool | Blind pool |
|---|---|---|
| Type parameter | On the pool | On the allocation |
| Handles | Four flavors | The same four |
| Chunk sizing | Slot count | Byte target, or a slot count, clamped per layout |
| Chunk cap | Per pool | Per layout, clamped, plus a cap on layouts |
| Capacity queries | Single tier, constant time | Two tiers; aggregates scale with layouts |
| Allocator | Held once | Cloned per layout, so it must be cloneable |
| First use of a layout | — | Cold path that allocates pool metadata |

Everything else is common: the handle types and their guarantees, coercion to
unsized values, pinning rules, the uninitialized tiers, the error currency, the
panic-versus-`Result` split, and the `Send` rules for both the pool object and
its handles.

Two capabilities are deliberately absent, matching the typed pool. There is no
iteration over pool contents — handles are the only way to reach a value. And
there is no type identity or downcasting: the pool records a layout, not a
type, so a value's concrete type is recovered from its handle or not at all.

Converting a blind pool into a typed pool for one of its layouts, or the
reverse, is likewise not offered. The obstacle is not geometry — both forms
derive the same slot geometry from the same layout, which allocation and
reclamation safety require. It is that a typed pool and a layout pool are
distinct instantiations with their own teardown hooks and their own sizing and
capping policies, so neither can adopt the other's slots without adopting its
configuration and its pool-metadata teardown as well.

## Failure

The blind pool reports the same two failures as the typed pool, and adds no
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

The reference design for this feature is `infinity_pool`'s blind pool family.
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
pool runs. Allocator `allocate` and `deallocate` callbacks are governed
separately, by the
[invariant list](../DESIGN.md#design-invariants-at-a-glance).

The single-word handle and the lookup-free drop are direct consequences of the
pointer-recovery architecture the typed pool already rests on. Because a handle
can find its own pool, it does not need to carry a key to it, and the directory
never has to be reachable — or lockable — from a destructor. That is also what
makes externalising the lock viable: a design whose free path needed the
directory could not let the caller hold the lock, because every drop would have
to reacquire it.

The router, the layout pools, and the ordering that keeps the directory
consistent are described in
[`implementation/blind-pool.md`](../implementation/blind-pool.md).
