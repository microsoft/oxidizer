# `thread_aware` design

This document describes the user-visible behavior and design tenets of
`thread_aware::Arc` and its companion `storage::Storage` handle. The
`ThreadAware` trait and the relocation model these build on are introduced in
the crate root documentation; this document focuses on the affinity-partitioned
`Arc`. The internal mechanism — per-affinity slot locking, the two-stage
relocation, and its poison-freedom — is documented separately in
[implementation.md](implementation.md).

## 1. Purpose

`thread_aware::Arc<T, S>` is a shared pointer, like `std::sync::Arc`, that
additionally keeps a distinct value per memory affinity. It lets a type that is
not itself `ThreadAware` — a third-party client, a connection pool, any value
that benefits from being local to the core or NUMA node using it — be shared
through a handle that reacts to relocation by adopting, or lazily creating, a
value belonging to the destination affinity.

Cloning an `Arc<T, S>` is cheap and shares state. Dereferencing it yields the
value for the affinity its holder currently belongs to. The `Strategy` type
parameter `S` decides what "affinity" means.

## 2. Affinities and strategies

An affinity identifies the placement of the code that holds the `Arc`. The
strategy maps that affinity to the value the holder sees:

- `PerProcess` maps every affinity to one slot, so a clone family shares a single
  materialized value process-wide. For values whose clones share their state this
  approximates a plain `sync::Arc<T>`, but it is not identical: the first
  relocation still materializes the shared value (see §3), so a holder that never
  relocates keeps the value it was created with.
- `PerCore` keeps a value per processor. A holder that relocates to another
  processor observes that processor's own value.
- `PerNuma` keeps a value per memory region, so holders on cores of the same NUMA
  node share a value and holders on different nodes do not.

Custom strategies are possible; a strategy must report the same slot count for
every affinity that shares one `Arc` (see [implementation.md](implementation.md)).

## 3. Per-affinity values

An `Arc<T, S>` always carries a current value in hand and derefs to it directly,
without synchronization. It additionally keeps, per slot, the value materialized
for that slot; a holder that relocates into a slot adopts the slot's value, and
clones that have adopted the same slot share one underlying `sync::Arc<T>`.

Constructors create the initial carried value eagerly. The per-slot values are
then produced lazily — a slot is materialized the first time a holder relocates
into it while it is still empty — and how a value is produced depends on the
constructor used:

- `new` / `new_boxed` run a constructor function once per slot, giving each slot a
  freshly built, independent value. Neither requires `T: Clone` or
  `T: ThreadAware`.
- `new_with` runs a closure that may capture other `ThreadAware` state, which is
  itself relocated for the destination before the value is built.
- `from_unaware` takes one value and clones it for each slot.
- `with_clone_fn` takes a concrete value plus a clone function, so trait-object
  values can be reproduced per slot without an object-safe `Clone`.

Relocation is a cooperative performance optimization, not a guarantee: consistent
with the crate-wide contract for `ThreadAware`, a holder that reaches a new
affinity without a relocation call still functions correctly, keeping the value it
already carries rather than switching. Dereferencing never blocks.

`Arc::strong_count` estimates how many strong references to the holder's current
value are held outside the shared storage: it is that value's raw `sync::Arc`
strong count minus the references the storage's slots hold. It samples those two
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

`storage::Storage<T, S>` is the per-affinity table an `Arc` shares across its
clones, exposed as a handle a caller can build directly. This serves the case
where the per-affinity values are known in advance rather than materialized lazily
on relocation:

1. Build an empty table with `Storage::new`.
2. Publish a `sync::Arc<T>` for each affinity that should carry one with
   `Storage::insert`; read one back with `Storage::get`.
3. Hand the table to `Arc::from_storage` together with the current affinity to
   obtain an `Arc` backed by those values.

`from_storage` requires that the table already hold a value for the current
affinity. An `Arc` built this way that later relocates into an affinity the table
left empty behaves like a plain `sync::Arc`, keeping the value it carries. The
handle exposes only the affinity-keyed insert/get surface; the slot layout behind
it is not part of the contract.

## 6. Design tenets

- **Performance first, correctness always.** The type exists to reduce
  cross-affinity contention, but relocation is advisory: skipping it degrades
  performance, never correctness.
- **Cheap, lock-free reads.** Dereferencing yields the carried value directly and
  does not synchronize; synchronization is confined to relocation.
- **Per-slot values, in-slot sharing.** Lazily materialized values are kept per
  slot and are not shared across slots, while holders that have adopted one slot
  share its value. Prepared storage is under the caller's control and can
  deliberately place the same value in several slots.
- **Unsized support is a feature.** Trait-object and slice values are a retained
  capability, weighed against representations that would trade `?Sized` away.
- **Storage is usable, its shape is not exposed.** Callers can construct and fill
  a `Storage`, but only through an affinity-keyed contract that leaves the
  representation free to change.
