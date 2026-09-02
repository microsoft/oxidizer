# `thread_aware::Arc` implementation

This document records the implementation choices behind `thread_aware::Arc`.

## Representation

Each holder contains:

- `storage: std::sync::Arc<Storage<T, S>>`
- `value: std::sync::Arc<T>`
- a cloneable factory describing how to create a value for a new partition

`Storage<T, S>` contains a `DashMap<S::Key, std::sync::Arc<T>>` initialized with
capacity 32. The strategy key is derived from a borrowed `Thread`.

The carried `value` makes `Deref` independent of shared storage. Reads through
the holder perform no map lookup and take no lock.

## Strategy keys

`PerThread` uses `Thread::id`. `PerNumaNode` clones `Thread::numa_node`.
`PerProcess` uses `()`. Strategy is sealed because the storage and relocation
protocol rely on the built-in semantic guarantees, especially the
`SINGLE_PARTITION` marker used by `PerProcess`.

## Relocation

Relocation follows this order:

1. Bind storage to the source owner when the first known relocation crosses an
   owner boundary, then return without changing factory state, partition
   contents, or the carried value.
2. Bind unowned storage to the destination owner. If storage already belongs to
   another owner, return without changing the carried value.
3. Record the first known source for closure factories.
4. Look up the destination key. On a hit, adopt its value.
5. If source and destination have the same key, publish or adopt the carried
   value for that key.
6. Otherwise materialize the destination entry through DashMap's entry API.
7. Record the previous carried value under the known source key if that key was
   still empty.

`PerProcess` treats an unknown source as the same partition because every
thread necessarily maps to its one key. Other strategies cannot infer that an
unknown source belongs to the destination key.

## Publication and reentrancy

The vacant DashMap entry remains held while a factory runs. This makes
publication atomic for one key: racing relocations execute one successful
factory and all adopt the same value.

Factory code must not touch the same storage. Re-entry can deadlock on the map
shard. Different keys may share a shard, so long-running or blocking factories
also delay unrelated relocations. Constructors and clone functions are expected
to be short and non-blocking.

A factory panic leaves the entry vacant and propagates to the caller. A later
relocation may retry.

## Factory variants

- `Closure` stores erased cloneable `ThreadAwareFnOnce` state and the first
  known source coordinate.
- `Data` clones the carried value and may relocate the clone.
- `ErasedCloneFn` supports unsized targets and trait objects.
- `Manual` clones the carried value for storage supplied by the caller.

Factory state is local to each `Arc` holder. Shared storage deduplicates the
values produced by holders that race into one partition.

## Strong counts

`Arc::strong_count` reads the raw strong count of the carried allocation and
subtracts the number of storage entries pointing at that allocation. The two
samples are not one atomic snapshot, so the result is approximate under
concurrent relocation and uses saturating subtraction.

## Safety and correctness boundary

No unsafe code is needed for the storage protocol. `ThreadAware` requires
`Send`, while the Arc implementation additionally requires the shared value,
strategy, and key operations to satisfy the bounds needed by `DashMap`.

Relocation is advisory. Missing, repeated, same-key, unknown-source, and
cross-owner calls must all leave the holder usable. The cross-owner no-op is
especially important for runtime-bound resources: retaining a working remote
resource is preferable to replacing it with state for an unrelated runtime.
Owner binding also prevents that retained resource from becoming a canonical
partition value if the holder later moves again inside the foreign runtime.
Each holder separately records the owner of its carried value because another
clone may have already bound the shared storage to a different owner.

Partition entries are retained until their shared storage is dropped. This
matches stable thread-per-core worker sets but means `PerThread` storage should
not outlive an unbounded sequence of transient threads.
