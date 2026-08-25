# `thread_aware` design

This document describes the user-visible behavior and design tenets of
`thread_aware::Arc` and its companion `storage::Storage` handle. The
`ThreadAware` trait and the relocation model these build on are introduced in
the crate root documentation; this document focuses on the strategy-partitioned
`Arc`. The internal mechanism — the write-once partition slots and the
relocation protocol built on them — is documented separately in
[implementation.md](implementation.md).

## 1. Purpose

`thread_aware::Arc<T, S>` is a shared pointer, like `std::sync::Arc`, that
additionally keeps a distinct value per strategy partition. It lets a type that
is not itself `ThreadAware` — a third-party client, a connection pool, any value
that benefits from being local to the core or NUMA node using it — be shared
through a handle that reacts to relocation by adopting, or lazily creating, the
value belonging to the destination affinity's partition.

Cloning an `Arc<T, S>` is cheap and shares state. Dereferencing it yields the
value carried by that holder. The `Strategy` type parameter `S` decides how
affinities map to partitions.

## 2. Affinities and strategies

An affinity identifies the placement of the code that holds the `Arc`. The
strategy maps that affinity to the partition whose value the holder sees:

- `PerProcess` maps every affinity to one partition, so all clones share a single value
  process-wide and relocation keeps that shared value: an `Arc<T, PerProcess>`
  behaves like a plain `sync::Arc<T>`.
- `PerCore` defines one partition per processor. A holder that relocates to another
  processor observes that processor partition's value.
- `PerNuma` defines one partition per memory region, so holders on cores of the
  same NUMA node share a value and holders on different nodes do not.

Custom strategies are possible; they are expected to report a partition count —
always at least one — that is consistent across the affinities that share one
`Arc` (see [implementation.md](implementation.md)).

## 3. Strategy-partitioned values

An `Arc<T, S>` always carries a current value in hand and derefs to it directly,
without synchronization. It additionally keeps the value materialized for each
strategy partition; a holder that relocates into a partition adopts that
partition's value, and clones in the same partition share one underlying
`sync::Arc<T>`.

Constructors create the initial carried value eagerly. Additional partition
values are then produced lazily — a partition is materialized the first time a
holder relocates into it across a partition boundary while it is still empty —
and how a value is produced depends on the constructor used:

- `new` / `new_boxed` run a constructor function once per partition, giving each
  partition a freshly built, independent value. Neither requires `T: Clone` or
  `T: ThreadAware`.
- `new_with` runs a closure that may capture other `ThreadAware` state, which is
  itself relocated for the destination before the value is built.
- `from_unaware` takes one value and clones it for each partition.
- `with_clone_fn` takes a concrete value plus a clone function, so trait-object
  values can be reproduced per partition without an object-safe `Clone`.

Materialization runs while the destination partition is being initialized.
Constructor functions, clone functions, and captured `ThreadAware` state must not
relocate an `Arc` backed by the same storage into that partition or form a cycle
among partition initializations. Write-once initialization is non-reentrant, so
such dependencies can deadlock.

A relocation whose source and destination resolve to the same partition is not a
cross-partition move: the holder keeps the value it is already carrying rather than
producing a new one. This is why every relocation under `PerProcess` — where all
affinities share one partition — preserves the shared value.

Relocation is a cooperative performance optimization, not a guarantee: consistent
with the crate-wide contract for `ThreadAware`, a holder that reaches a new
affinity without a relocation call still functions correctly, keeping the value it
already carries rather than switching. Dereferencing never blocks.

`Arc::strong_count` estimates how many strong references to the holder's current
value are held outside the shared storage: it is that value's raw `sync::Arc`
strong count minus the references the storage's partitions hold. It samples those two
counts separately, so under concurrent relocation the result is approximate, and
it saturates rather than underflowing.

## 4. Unsized values

`T` may be unsized, so `Arc<dyn Trait, S>` and other trait-object or slice values
are supported. Because an unsized value cannot be passed or held by value, the
unsized-capable entry points work through a pointer: `new_boxed` and
`with_clone_fn` produce `Box<T>`, and `from_storage` and `Storage::insert` accept
a ready `sync::Arc<T>`. This support is a deliberate, retained capability of the
type; see [implementation.md](implementation.md) for how it shapes the slot
representation.

## 5. Prepared storage

`storage::Storage<T, S>` is the strategy-partitioned table an `Arc` shares across
its clones, exposed as a handle a caller can build directly. This serves the case
where the partition values are known in advance rather than materialized lazily
on relocation:

1. Build an empty table with `Storage::new`.
2. Publish a `sync::Arc<T>` for each strategy partition by passing a representative
   affinity to `Storage::insert`; read one back with `Storage::get`.
3. Hand the table to `Arc::from_storage` together with the current affinity to
   obtain an `Arc` backed by those values.

`from_storage` requires that the table already hold a value for the current
affinity's partition. An `Arc` built this way that later relocates into an
affinity whose partition the table left empty behaves like a plain `sync::Arc`,
keeping the value it carries. The handle exposes only the affinity-keyed
insert/get surface; the partition layout behind it is not part of the contract.

`Storage::insert` and `Storage::get` require an affinity that maps into the table's
coordinate space — one within the partition count the strategy reports. A caller
building storage by hand controls its own affinities, so these accessors reject
an out-of-range affinity rather than tolerate it.

## 6. Design tenets

- **Performance first, correctness always.** The type exists to reduce
  cross-affinity contention, but relocation is advisory: skipping it degrades
  performance, never correctness.
- **Cheap, lock-free reads.** Dereferencing yields the carried value directly and
  does not synchronize; synchronization is confined to relocation.
- **Per-partition values, in-partition sharing.** Lazily materialized values are
  kept per partition and are not shared across partitions, while holders in one
  partition share its value. Prepared storage is under the caller's control and
  can deliberately place the same value in several partitions.
- **Unsized support is a feature.** Trait-object and slice values are a retained
  capability, weighed against representations that would trade `?Sized` away.
- **Storage is usable, its shape is not exposed.** Callers can construct and fill
  a `Storage`, but only through an affinity-keyed contract that leaves the
  representation free to change.
