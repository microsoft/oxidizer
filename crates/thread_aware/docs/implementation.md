# Thread-aware implementation

The user-visible behavior and design tenets of `Arc` and `Storage` are described
in [design.md](design.md); this document covers how that behavior is implemented.

## Storage

An `Arc<T, S>` owns a handle to a slot table shared by every clone of that
value. The `Strategy` type parameter maps an `Affinity` to a slot index and to
the number of slots the table needs, so `PerCore`, `PerNuma` and `PerProcess`
differ only in that mapping.

Each slot holds the value materialized for it and is guarded by its own
reader-writer lock. The strategy maps affinities to slots: `PerCore` gives each
processor its own slot, while `PerNuma` and `PerProcess` fold several affinities
onto one shared slot. Relocations into different slots touch different locks and
never contend, so under `PerCore` a fanout across cores spreads across slots; a
relocation only ever synchronizes with other relocations into the *same* slot.

Slots are cache-line padded to curb false sharing between neighboring locks. The
padding follows a target-specific alignment estimate; it reduces the chance that
two locks share a line on the architectures the crate targets, rather than
guaranteeing physical isolation on every machine.

The table is sized once, on first use, to the slot count the strategy reports.
Because the whole table is fixed from then on, the design assumes the strategy
reports the same count for every affinity that shares it — the built-in strategies
do, since the processor and memory-region counts are properties of the machine. A
strategy that breaks that assumption can produce an index past the table's end; the
lookup then falls back to the first slot rather than reaching out of bounds, and
debug builds trap the anomaly. There is no growth path and no table-wide lock
guarding the array. After that first
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
`Storage::insert`, `Storage::get` and `Arc::from_storage` all traffic in
`sync::Arc<T>`, because an unsized `T` cannot be passed or stored by value and
must already sit behind a shared pointer.

Storing `sync::Arc<T>` is also what preserves `T: ?Sized` (so `Arc<dyn Trait, S>`
works). A `sync::Arc<T>` is a sized value for any `T` — thin when `T` is sized, a
fat pointer when it is not — and the slot's `RwLock` holds it as an opaque value
without inspecting its shape.

`Storage` is constructible and populatable from outside the crate: a caller can
build one with `Storage::new`, seed affinities with `Storage::insert`, and then
pass it to `Arc::from_storage` to obtain an `Arc` backed by those prepared
values. The internal slot layout stays hidden — only the affinity-keyed
insert/get surface is exposed.

### Why a reader-writer lock and not an atomic swap

An earlier design considered replacing each slot's `RwLock<Option<Arc<T>>>` with
an atomic pointer swap (an `arc_swap`-style cell), which would turn a slot read
into a lock-free load. It is not adopted, for two reasons.

A cell like that stores the live handle as a single machine-word pointer it loads
and stores atomically. That works directly only when `T` is sized, where `Arc<T>`
is a thin pointer; for an unsized `T` — `dyn Trait`, `[u8]` — `Arc<T>` is a fat
pointer that does not fit one atomic word, so a direct swap would force
`T: Sized`. The reader-writer lock keeps `?Sized` for free, because it guards the
`Arc<T>` as an opaque value and never decomposes it into a pointer. `?Sized` could
still be kept under a swap by adding a thin indirection — swapping an `Arc<Arc<T>>`
or `Arc<Box<T>>`, whose outer handle is thin whatever `T` is — but that adds an
allocation and an extra indirection to every stored value.

The decisive reason is that the swap would buy almost nothing here.
Dereferencing an `Arc<T, S>` — the steady state — never touches a slot: the holder
carries its current value in its own `value` field and derefs through that with no
synchronization. A slot lock is taken on relocation (and by the storage
accessors), and on relocation's common hit path only its shared side. Because each
slot has its own lock and the workloads that matter relocate into distinct slots,
that acquisition is essentially uncontended, so replacing it with a lock-free load
would shave a few instructions off a path that is not the hot one while adding
indirection to every stored value. Keeping the lock is the better trade.

## Relocation locking

`ThreadAware::relocate` moves a clone of an `Arc` into a destination affinity. It
locks only the destination affinity's slot, and acquires it in two stages.

```text
    relocate(source, destination)
             |
      [ shared lock: slot[destination] ]
       read slot[destination]
             |
      populated? --- yes ---> adopt value, done
             |
             no
             |
    [ exclusive lock: slot[destination] ]
       read slot[destination]        (re-probe)
             |
      populated? --- yes ---> adopt value, done
             |
             no
             |
      same slot as source? --- yes ---> keep carried value,
             |                            seed slot[destination], done
             no
             |
       materialize value
       publish to slot[destination]
             |
      [ release slot[destination] ]
             |
      [ exclusive lock: slot[source] ]
       record source value in slot[source] if empty
```

The first stage carries the throughput. A slot is never emptied once populated,
so a relocation into an already-populated affinity only clones a reference out of
its slot. Because each slot owns its own lock, these reads scale with the number
of slots: under `PerCore` a fanout that hands work to every core relocates into a
different slot per core, and the cores do not contend.

The second stage handles a miss: the destination slot is empty. If the source
resolves to that same slot, the value the `Arc` carries already belongs there, so
it is kept and the slot is seeded with it — this is every relocation under
`PerProcess`, and any relocation whose source and destination share a slot.
Otherwise the value is materialized by running the factory and published under the
exclusive lock, on that slot only, so a miss on one slot never blocks hits on
another. The re-probe before either is required for correctness, not an
optimization: the lock is dropped between the stages, so another thread may have
populated the slot in between, and without re-checking, two threads would
materialize competing values for the same slot.

A cross-slot miss also preserves the value it moves away from. The `Arc` is
carrying the source slot's value, so that value is written into the source slot
when it is still empty — the case of an `Arc` leaving a slot that nothing had
recorded yet. This write happens after the destination lock is released, never
with both locks held: two threads relocating in opposite directions (`X → Y` and
`Y → X`) would otherwise deadlock, each waiting for the lock the other holds. The
same-slot case needs none of this, having already seeded the one slot involved.

A slot lock is never left poisoned, so acquiring one never has to handle a poison
error. Poisoning would require a panic while the lock is held, and the crate
never panics there. The operations run under a slot lock — cloning, storing and
comparing the reference-counted handle it holds — cannot unwind. The one
exception is materializing a value on the miss path, which runs the caller's
factory; `relocate` runs that under `catch_unwind`, and on a panic it drops the
destination guard before resuming the unwind, so the lock is released cleanly
rather than poisoned. `strong_count` likewise clones each handle out from under
its lock and applies its predicate afterwards, so no caller code runs while a
lock is held.

## Benchmarks

`benches/thread_aware_relocate.rs` and its Callgrind counterpart
`benches/thread_aware_relocate_cg.rs` measure relocation. The suite separates
the two stages, because they have opposite cost profiles and only the first one
is on the hot path.

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

The two subjects do very different amounts of lock work per message. Each
thread-aware node owns a separate slot table with its own lock, and the derived
walk visits fields in sequence, so relocating the tree takes roughly one slot
acquisition per layer instead of one in total. Each message therefore pays the
per-slot cost several times over, which makes that cost easier to resolve.

`hit_path` and `miss_path` measure both subjects, the bare one being a meaningful
isolation of a single relocation. `concurrent` measures only the tree: at one
acquisition per message the bare subject does too little lock work to resolve the
effect it is looking for.

### The miss benchmark

`miss_path` measures a cross-slot miss: the destination slot is empty, so
relocation escalates to that slot's exclusive lock, runs the factory to
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
touch the same slot lock. What the benchmark isolates is that hit path with no
shared lock to serialize on: adding workers introduces no lock hand-off between
them, so the per-relocation cost does not degrade the way it did when a single
lock guarded the whole table. The before/after comparison against that former
design is reported in the pull request, not established by this run alone.

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
slots, the oversubscribed shape does not add lock contention; it adds scheduler
pressure, exposing how the hit path behaves when more workers than processors are
runnable. The uncontended single-worker cost is `hit_path`'s job and is
deliberately absent here, where one worker would only add the pool's
thread-handoff overhead to the same number.

Processor count means logical processors, so on a machine with simultaneous
multithreading the saturated shape runs two workers per physical core. The
affinities the workers relocate between are fabricated values used to select
slots; no thread is pinned.

By construction `concurrent` does not measure the miss path or contention on a
shared destination; the miss path — including the source-slot write that follows
it — is `miss_path`'s single-threaded job, and shared-destination contention is
out of scope for the whole suite. It is Criterion-only: Callgrind counts
instructions on a serialized execution and cannot observe scaling across threads
at all.
