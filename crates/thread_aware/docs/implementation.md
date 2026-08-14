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

| Group       | What it measures                                                     |
| ----------- | -------------------------------------------------------------------- |
| `hit_path`  | Relocation into an already-populated slot, single-threaded.           |
| `miss_path` | Relocation that has to materialize a slot, single-threaded.           |
| `storm`     | Many threads relocating into distinct affinities at the same time.    |

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
isolation of a single relocation. `storm` measures only the tree: at one
acquisition per message the bare subject collides too rarely for its run-to-run
spread to resolve any difference between locking policies.

`storm` covers one thread, one thread per processor, and
`STORM_OVERSUBSCRIPTION` threads per processor. The oversubscribed shape is the
one that exposes a thread preempted while holding an exclusive lock, since only
then are there runnable threads queued behind it. Thread counts far above the
processor count are deliberately *not* used: barrier release costs roughly a
millisecond per few threads and lands inside the measured round, so by a few
hundred threads the shape measures thread wake-up instead of relocation.

Processor count here means logical processors, so on a machine with simultaneous
multithreading the saturated shape runs two workers per physical core. The
affinities the workers relocate between are fabricated values used to select
slots; no thread is pinned.

A shape whose workers do not actually run at the same time quietly reports
uncontended timings under a contended name, so `storm` both arranges for the
overlap and then checks it. Each worker relocates untimed until every worker is
awake, and again after closing its own timing window until every worker has
closed one, so the timed region is bracketed by full load rather than by the
ragged edges of barrier release. Each round then asserts that the windows really
did overlap. The assertion is skipped for rounds too short to outlast the spread
in start times that remains after the lead-in, which is a condition only
Criterion's first ramp-up rounds meet.

`storm` is Criterion-only. Callgrind counts instructions on a serialized
execution, so it cannot observe lock contention at all. It also cannot show the
benefit of the shared-lock probe, because an uncontended shared acquisition costs
about as many instructions as an uncontended exclusive one; its role is to catch
regressions in either branch.

`storm` reports the median over its workers rather than the elapsed time of the
round. Timing the round from the controller would measure how long the operating
system took to wake the workers, and a mean would let one stalled worker move the
whole sample.
