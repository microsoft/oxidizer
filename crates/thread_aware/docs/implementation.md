# Thread-aware implementation

## Storage

An `Arc<T, S>` owns a handle to a slot table shared by every clone of that
value. The `Strategy` type parameter maps an `Affinity` to a slot index and to
the number of slots the table needs, so `PerCore`, `PerNuma` and `PerProcess`
differ only in that mapping.

A slot holds the value materialized for one affinity. The table is filled
lazily: a slot is populated the first time a clone of the value is relocated
into the affinity that owns it, and it stays populated for the lifetime of the
shared table.

## Relocation locking

`ThreadAware::relocate` moves a clone of an `Arc` into a destination affinity.
The slot table is guarded by a reader-writer lock and relocation acquires it in
two stages.

```text
    relocate(source, destination)
             |
      [ shared lock ]
       read slot[destination]
             |
      populated? --- yes ---> adopt value, done
             |
             no
             |
    [ exclusive lock ]
       read slot[destination]        (re-probe)
             |
      populated? --- yes ---> adopt value, done
             |
             no
             |
       materialize value
       publish to slot[destination]
       restore old value to slot[source]
```

The first stage is the one that matters for throughput. Slots are populated
lazily but never emptied, so once a process has warmed up, essentially every
relocation into a given affinity finds the slot already populated and does
nothing but clone a reference out of it. Serving that case under an exclusive
lock would funnel every relocation of every clone of the value through a single
writer, turning a read-only lookup into a process-wide serialization point on
the hot path of cross-affinity work handoff.

The second stage exists for the cold case, where the destination slot has to be
materialized. Materialization runs the value's factory and publishes the result,
which requires exclusive access.

The re-probe at the start of the second stage is load-bearing rather than an
optimization. The lock is released between the stages, so several threads can
observe the same empty slot and queue up on the exclusive lock together. Only
the first of them may materialize; the rest must observe the published value and
adopt it, otherwise they would each materialize a competing value and overwrite
the one already published, handing different threads different values for the
same affinity.

Materialization holds the exclusive lock for the duration of the factory call.
This keeps the slot table and the published value consistent without a second
synchronization mechanism, at the cost of blocking concurrent relocations into
other affinities of the same value while a cold slot is being filled.

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
one relocation, and a five-layer object tree, which is what actually crosses
affinities in a consumer. Relocation is a graph walk, so a caller pays for every
thread-aware node reachable from the message rather than for a single call.

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
