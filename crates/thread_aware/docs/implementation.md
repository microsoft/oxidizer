# Thread-aware implementation

## Storage

An `Arc<T, S>` owns a handle to a slot table shared by every clone of that
value. The `Strategy` type parameter maps an `Affinity` to a slot index and to
the number of slots the table needs, so `PerCore`, `PerNuma` and `PerProcess`
differ only in that mapping.

Each slot holds the value materialized for one affinity and is guarded by its
own reader-writer lock, on its own cache line. Relocations targeting different
affinities therefore touch different locks on different lines and never contend;
a relocation only ever synchronizes with other relocations into the *same*
affinity.

The table is sized once, on first use, to the slot count the strategy reports —
a value fixed for the process lifetime — so there is no growth path and no
table-wide lock guarding the array. After that first initialization, reaching a
slot is a plain atomic load of a pointer that stays resident and shared in every
core's cache, generating no coherence traffic. Slots are filled lazily: a slot
is populated the first time a clone is relocated into the affinity that owns it,
and stays populated for the lifetime of the shared table.

A slot stores an `Arc<T>` rather than a `T`. An `Arc<T>` is a sized value even
when `T` is not, which is what lets `Arc<T, S>` keep `T: ?Sized` (so `Arc<dyn
Trait, S>` works). The public handle is `storage::Storage`; the raw table behind
it is `SlotTable`, kept separate so it can be unit-tested with a plain value type
while the handle pins the stored type to `Arc<T>`.

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
its slot. Because each affinity owns its own lock, these reads scale with the
number of affinities: a fanout that hands work to every core relocates into a
different slot per core, and the cores do not contend.

The second stage handles a miss: the destination slot is empty, so its value is
materialized by running the factory and published under the exclusive lock — on
that slot only, so a miss on one affinity never blocks hits on another. The
re-probe before materializing is required for correctness, not an optimization:
the lock is dropped between the stages, so another thread may have populated the
slot in between, and without re-checking the two would materialize competing
values for the same affinity.

A relocation also preserves the value it moves away from. The `Arc` is carrying
the source affinity's value, so on a miss that value is written into the source
slot when the slot is still empty — the case of an `Arc` leaving an affinity that
nothing had recorded yet. This write happens after the destination lock is
released, never with both locks held: two threads relocating in opposite
directions (`X → Y` and `Y → X`) would otherwise deadlock, each waiting for the
lock the other holds.

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
| `miss_path`  | Relocation that has to materialize a slot, single-threaded.          |
| `concurrent` | Cost of one relocation while every core relocates at once.           |

The suite measures two subjects: a bare `Arc<Payload, PerCore>`, which isolates
one relocation, and a five-layer object tree, a larger object graph. Relocation
is a graph walk, so a caller pays for every thread-aware node reachable from the
message, not just for a single call; the two subjects cover both ends of that
range.

The two subjects exercise the locking policy at very different rates. Each
thread-aware node owns a separate slot table with its own lock, and the derived
walk visits fields in sequence rather than nested, so the tree does not hold any
one lock for longer — it takes roughly one acquisition per layer instead of one
in total. That multiplies how often a message collides with another thread.

`hit_path` and `miss_path` measure both subjects, the bare one being a meaningful
isolation of a single relocation. `concurrent` measures only the tree: at one
acquisition per message the bare subject collides too rarely to resolve any
difference between locking policies.

### The concurrent benchmark

`concurrent` answers a single question: what does one relocation cost while every
core is relocating at once? A pool of workers is created once and reused. Each
round hands every worker a batch of relocations, releases them together, and
measures the wall-clock time until the last worker finishes.

The batch is the crux. A relocation is a handful of nanoseconds, while releasing
the workers and waking them onto their cores is tens of microseconds; a
per-operation timing at that ratio measures the scheduler, not the lock. Batching
many relocations behind one release amortizes the fixed cost to nothing. Criterion
drives the batch size up until a sample fills its target time and fits round
duration against batch size, so the fixed release cost falls into the regression
intercept and the reported per-iteration time is the slope — the cost of one
relocation under contention. Throughput is reported per worker, so the group also
prints aggregate relocations per second.

Readiness is proven before the clock starts: every worker parks on a barrier, the
controller waits there too, and only once all have arrived does it start timing
and release them. Timing therefore excludes barrier arrival skew, and there is no
per-operation synchronization inside the batch to distort the measurement.

The group sweeps contention with one worker per processor and
`CONCURRENT_OVERSUBSCRIPTION` workers per processor. The oversubscribed shape is
the one that exposes a worker preempted while holding an exclusive lock, since
only then are there runnable workers queued behind it. The uncontended cost is
`hit_path`'s job and is deliberately absent here, where a single worker would
measure no contention and only add the pool's thread-handoff overhead.

Processor count means logical processors, so on a machine with simultaneous
multithreading the saturated shape runs two workers per physical core. The
affinities the workers relocate between are fabricated values used to select
slots; no thread is pinned.

`concurrent` is Criterion-only. Callgrind counts instructions on a serialized
execution, so it cannot observe lock contention at all. It also cannot show the
benefit of the shared-lock probe, because an uncontended shared acquisition costs
about as many instructions as an uncontended exclusive one; its role is to catch
regressions in either branch.
