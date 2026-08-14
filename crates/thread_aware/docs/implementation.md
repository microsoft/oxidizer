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
| `handoff`   | Two threads passing values to each other, relocating on receipt.      |

`storm` and `handoff` are Criterion-only. Callgrind counts instructions on a
serialized execution, so it cannot observe lock contention at all.
