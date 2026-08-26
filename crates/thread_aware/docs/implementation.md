# Thread-aware implementation

The user-visible behavior and design tenets of `Arc` and `Storage` are described
in [design.md](design.md); this document covers how that behavior is implemented.

## Storage

An `Arc<T, S>` owns a handle to a slot table shared by every clone of that
value. The `Strategy` type parameter maps an `Affinity` to a slot index and to
the number of slots the table needs, so `PerCore`, `PerNuma` and `PerProcess`
differ only in that mapping.

Each slot is a write-once cell — a `OnceLock` — published the first time a clone
is relocated into it and read by every relocation thereafter. The strategy maps
affinities to slots: `PerCore` gives each processor its own slot, while `PerNuma`
and `PerProcess` fold several affinities onto one shared slot. A published slot is
read with a plain acquire load and is never mutated again, so relocations into
already-populated slots carry no slot or table lock-word contention. Adopting the
stored `sync::Arc` still updates its strong count, so callers that share a partition
can contend on that allocation's reference count. Under `PerCore` a fanout across
cores spreads across slots that are all read-only in the steady state.

The table is sized once, on first use, to the slot count the strategy reports —
`Strategy::count` returns a `NonZero<usize>`, so the table always has at least one
slot. Because the whole table is fixed from then on, the design assumes the strategy
reports the same count for every affinity that shares it — the built-in strategies
do, since the processor and memory-region counts are properties of the machine. A
strategy that breaks that assumption can produce an index past the table's end; such
an affinity has no slot of its own. The relocation path treats it as unreachable: a
relocation into such a destination is a no-op — the `Arc` keeps the value it already
carries rather than reaching into an unrelated slot — and records the
`thread_aware_arc_oob` metric so the condition is observable in a running process.
The direct storage accessors do not degrade this way: `Storage::insert` and
`Storage::get` panic on an out-of-range affinity, because a caller preparing storage
by hand controls its own affinities, so mixing coordinate spaces there is a
programming error rather than the tolerated relocation fallback. There is no growth
path and no table-wide lock guarding the array. After that first
initialization the array and the pointer to it are immutable, so reaching a slot
is a plain atomic load that carries no further synchronization; how well it stays
in cache is left to the hardware. Slots are filled lazily: a slot is populated the
first time a clone is relocated into it, and stays populated for the lifetime of
the shared table.

A slot stores a `sync::Arc<T>` — the same shared handle the `Arc<T, S>` derefs
through — not a bare `T`. The `Arc` is the crate's primary abstraction: an
`Arc<T, S>` keeps its current-affinity value in a `value: sync::Arc<T>` field and
derefs through it with no locking, while the slot table is a secondary,
lazily-filled cache recording which `sync::Arc<T>` was materialized for each
affinity, so a later relocation can restore the same shared handle. The choice to
store an `Arc` is therefore made at the `Storage` layer, not baked into
`SlotTable`: `SlotTable` is a generic, value-agnostic partitioned table
(unit-tested with a plain value type), and `Storage` is the adapter that fixes its
element type to `sync::Arc<T>`. The public boundary makes the same split visible —
`Storage::insert`, `Storage::get` and `Arc::from_storage` all pass and store
`sync::Arc<T>`, because an unsized `T` cannot be passed or stored by value and
must already sit behind a shared pointer.

Storing `sync::Arc<T>` is also what preserves `T: ?Sized` (so `Arc<dyn Trait, S>`
works). A `sync::Arc<T>` is a sized value for any `T` — thin when `T` is sized, a
fat pointer when it is not — and the slot's `OnceLock` holds it as an opaque value
without inspecting its shape.

`Storage` is constructible and populatable from outside the crate: a caller can
build one with `Storage::new`, seed affinities with `Storage::insert`, and then
pass it to `Arc::from_storage` to obtain an `Arc` backed by those prepared
values. The internal slot layout stays hidden — only the affinity-keyed
insert/get surface is exposed.

### Why a write-once cell and not an atomic swap

Each slot is published exactly once and then only read, so it is a `OnceLock`
rather than a mutable cell. A read is a plain acquire load with no lock word to
contend on — the property an `arc_swap`-style atomic pointer cell would also give —
but `OnceLock` reaches it without the two costs such a cell carries here.

An atomic pointer cell stores the live handle as a single machine-word pointer it
loads and stores atomically. That works directly only when `T` is sized, where
`Arc<T>` is a thin pointer; for an unsized `T` — `dyn Trait`, `[u8]` — `Arc<T>` is
a fat pointer that does not fit one atomic word, so a direct swap would force
`T: Sized`. `?Sized` could be kept by adding a thin indirection — swapping an
`Arc<Arc<T>>` or `Arc<Box<T>>`, whose outer handle is thin whatever `T` is — but
that adds an allocation and an extra indirection to every stored value.
`OnceLock<Arc<T>>` holds the `Arc<T>` as an opaque value and never decomposes it
into a pointer, so it preserves `?Sized` at no extra cost.

The swap's remaining capability — replacing an already-published value — is one
this design never uses. A slot is materialized once and never emptied or
rewritten, so write-once is exactly the contract the slot needs. `OnceLock` encodes
that contract directly: `get_or_init` publishes the first materialized value and
hands every racer that one value, so the factory runs at most once per strategy partition
without carrying the machinery for stores that never happen. Dereferencing an
`Arc<T, S>` — the steady state — never touches a slot at all: the holder carries
its current value in its own `value` field and derefs through that with no
synchronization, so the only slot reads are on relocation, and on the common hit
path each is that cheap lock-free read.

## Relocation and publication

`ThreadAware::relocate` moves a clone of an `Arc` into a destination affinity. It
touches at most two slots — the destination and, only on a cross-slot miss, the
source. Hit-path reads and source-slot writes do not hold one cell while accessing
another, so they introduce no lock order. On a destination miss,
`OnceLock::get_or_init` keeps that cell in its initializing state while the factory
runs. The factory must not reenter the same cell or form a cycle among cell
initializations; absent such caller-created dependencies, opposite-direction
source recording has no lock order that can deadlock.

```text
    relocate(source, destination)
             |
       record known original source in factory
             |
       probe slot[destination]            (acquire load)
             |
      populated? --- yes ---> adopt value, done
             |
             no
             |
       reach cell slot[destination]
             |
      out of range? --- yes ---> no-op: keep carried value,
             |                     record thread_aware_arc_oob, done
             no
             |
      same slot as source? --- yes ---> get_or_init slot[destination]
             |                            with carried value, adopt, done
             no
             |
       get_or_init slot[destination]      (destination is initializing while
             |                             factory runs at most once;
             |                             adopt the published value)
             |
       record carried value in slot[source]   (write-once, if in range)
```

The probe carries the throughput. A slot is never emptied once populated, so a
relocation into an already-populated partition is a cheap lock-free read that clones
a reference out of the cell. Because each slot is an independent cell, these reads
scale with the number of slots: under `PerCore` a fanout that hands work to every
core relocates into a different slot per core, and the cores share no lock word to
contend on.

A miss reaches the destination cell to publish into it. If the source resolves to
that same slot, the value the `Arc` carries already belongs there, so
`get_or_init` seeds the empty cell with it — or adopts a racer's value if one
published first. This is every relocation under `PerProcess`, and any relocation
whose source and destination share a slot. Otherwise the destination value is
published through `get_or_init`, which runs the caller's factory to materialize the
value and serializes that materialization on the cell: the first racer to arrive
runs the factory, every other racer blocks and then adopts the one published value.
The factory therefore runs at most once per strategy partition, upholding that
contract even under a concurrent first relocation, and no racer's work is dropped.
The destination cell remains in its initializing state while the factory runs.
Caller code must not reenter that cell — directly or through another `Arc` backed
by the same storage — or create a cycle among initializing cells, because
write-once initialization is non-reentrant. A panic simply propagates and leaves
the cell empty for the next relocation to retry: there is no poisonable lock and
no partial published state to unwind.

At the start of relocation, a closure factory records the source affinity if it
has not already done so and the source is known. The update is deterministic and
runs before the hit, out-of-range, and same-slot fast paths, so a clone that first
takes any of those paths still reproduces the original transfer when a later
relocation materializes a new value. An unknown source records nothing, allowing a
later relocation with a known source to establish the original affinity.

A cross-slot miss also preserves the value it moves away from. The `Arc` is
carrying the source slot's value, so that value is recorded into the source slot
with a write-once `set`. An already-populated source slot is left untouched:
another thread may have recorded the same slot with the same value first, and
keeping the existing value is correct. The same-slot case needs none of this,
having already seeded the one slot involved.

An affinity whose slot index falls outside the sized table has no cell to reach. A
relocation into such a destination is a no-op — the `Arc` keeps the value it
already carries — and an out-of-range source is simply not recorded into. Either
out-of-range access is reported through the `thread_aware_arc_oob` metric. Ref:
"Storage".

Because a slot is only ever published or read, never mutated in place, no operation
on the relocation path can leave shared state poisoned or half-written. The one
piece of caller code that runs — the factory on the miss path — runs before
anything is published, so its unwinding cannot corrupt a slot. `strong_count`
likewise reads each handle out of its cell with an acquire load and applies its
predicate afterwards, so no caller code observes a slot mid-write.

## Benchmarks

The unpublished `thread_aware_benchmarking` package owns
`benches/thread_aware_relocate.rs` and its Callgrind counterpart
`benches/thread_aware_relocate_cg.rs`. The suite separates the two stages,
because they have opposite cost profiles and only the first one is on the hot
path.

| Group        | What it measures                                                    |
| ------------ | ------------------------------------------------------------------- |
| `hit_path`   | Relocation into an already-populated slot, single-threaded.          |
| `miss_path`  | Cross-slot miss into an already-sized table, single-threaded.        |
| `concurrent` | Hit-path relocation throughput with many workers relocating at once. |

The suite measures two subjects: a bare `Arc<Payload, PerCore>`, which isolates
one relocation, and a five-layer object tree, a larger object graph. Relocation
is a graph walk, so a caller pays for every thread-aware node reachable from the
message, not just for a single call; the two subjects cover both ends of that
range.

The two subjects do very different amounts of slot work per message. Each
thread-aware node owns a separate slot table, and the derived walk visits fields
in sequence, so relocating the tree touches roughly one cell per layer instead of
one in total. Each message therefore pays the per-slot cost several times over,
which makes that cost easier to resolve.

`hit_path` and `miss_path` measure both subjects, the bare one being a meaningful
isolation of a single relocation. `concurrent` measures only the tree: at one cell
read per message the bare subject does too little slot work to resolve the effect
it is looking for.

### The miss benchmark

`miss_path` measures a cross-slot miss: the destination slot is empty, so
relocation runs the factory to
materialize the value, publishes it, and records the value the `Arc` carried in
the still-empty source slot. A primer affinity relocates a throwaway clone before
timing, so the shared slot table is already allocated and both the source and
destination slots it later uses stay empty. That keeps the one-time table
allocation — an O(slot-count) cost paid once per `Arc` lineage, not once per
miss — out of the measurement.

### The concurrent benchmark

`concurrent` measures one thing: the per-relocation cost of the hit path while
many workers relocate at once. A pool of workers is created once and reused; each
worker owns a distinct destination slot that is materialized before timing, and
relocates its own clone of the subject into that slot. Every measured relocation
is therefore a hit, and because the destinations are distinct no two workers ever
read the same cell. What the benchmark isolates is that hit path with no shared
lock word to serialize on: each hit is an acquire load of a distinct write-once
cell, so adding workers introduces no hand-off between them and the per-relocation
cost does not degrade the way it did when a single lock guarded the whole table.
The before/after comparison against that former design is reported in the pull
request, not established by this run alone.

Each round hands every worker a batch of relocations, releases them together, and
measures the wall-clock time until the last worker finishes. The batch is what
makes the measurement possible: a relocation is a handful of nanoseconds while
releasing the workers and waking them onto their cores is tens of microseconds, so
a per-round timing at that ratio would measure the scheduler, not the work.
Criterion drives the batch size up until a sample fills its target time and fits
round duration against batch size, so the fixed release cost lands in the
regression intercept. The reported per-iteration time is then the batch makespan
divided by the batch size — the amortized cost of one relocation on the worker
that finishes last. Workers are synchronized once per batch, not per relocation.
Throughput is counted per worker, so the group also prints aggregate relocations
per second.

Readiness is proven before the clock starts: every worker parks on a barrier, the
controller waits there too, and only once all have arrived does it start timing
and release them. Timing therefore excludes barrier arrival skew, and there is no
per-operation synchronization inside the batch to distort the measurement.

The group sweeps one worker per processor and `CONCURRENT_OVERSUBSCRIPTION`
workers per processor. Because the measured relocations are all hits into distinct
slots, the oversubscribed shape does not add cell contention; it adds scheduler
pressure, exposing how the hit path behaves when more workers than processors are
runnable. The uncontended single-worker cost belongs to `hit_path` and is
deliberately absent here, where one worker would only add the pool's
thread-handoff overhead to the same number.

Processor count means logical processors, so on a machine with simultaneous
multithreading the saturated shape runs two workers per physical core. The
affinities the workers relocate between are fabricated values used to select
slots; no thread is pinned.

By construction `concurrent` does not measure the miss path or contention on a
shared destination; the miss path — including the source-slot write that follows
it — is measured single-threaded by `miss_path`, and shared-destination contention
is out of scope for the whole suite. It is Criterion-only: Callgrind counts
instructions on a serialized execution and cannot observe scaling across threads
at all.
