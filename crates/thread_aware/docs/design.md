# `thread_aware` design

This document describes the user-visible behavior of `thread_aware::Arc` and
`storage::Storage`. The stable relocation contract is defined by
`thread_aware_core` and re-exported by `thread_aware`.

## 1. Core vocabulary

`thread_aware` re-exports `ThreadAware`, `Thread`, `Owner`, and `NumaNode` from
`thread_aware_core`. There is one authoritative relocation trait across the
package family.

Runtime integrations construct coordinates with `thread_aware::ThreadBuilder`.
`ThreadBuilder::default()` creates a unique runtime owner. Clones of one builder
retain that owner, so they can build coordinates for every worker in the same
runtime. `with_numa_node` selects the nearest NUMA node and `build` adds the
worker's `std::thread::ThreadId`.

The old coordinate and runtime registry modules do not exist. Processor
discovery, pinning, and worker enumeration belong to the runtime.

Implementations for third-party crate types also do not live here. Once
`thread_aware_core` is stable, third-party crates can implement its trait
natively. Until then, callers may use `Unaware` for inert foreign values.

## 2. Strategy-partitioned values

`thread_aware::Arc<T, S>` is a shared pointer that can keep one value per
strategy partition. It always carries a current `std::sync::Arc<T>` and
dereferences directly to that value without consulting shared storage.

The built-in strategies are:

- `PerThread`, keyed by `std::thread::ThreadId`.
- `PerNumaNode`, keyed by exact `NumaNode` coordinate identity.
- `PerProcess`, keyed by one constant process-wide value.

Strategies are sealed. The identifiers in `Thread` are opaque and not dense or
enumerable, so storage is keyed rather than indexed.

Partitioned storage intentionally uses the non-cryptographic Fx hasher. Its
sealed keys are trusted runtime-generated coordinate identifiers, not
attacker-controlled input, so randomized hash-flood resistance is unnecessary.

Constructors create the carried value eagerly. Additional values are produced
lazily when relocation first reaches a new key:

- `new` and `new_boxed` invoke a constructor once per reached partition.
- `new_with` relocates captured state before invoking its constructor.
- `from_unaware` clones an inert value.
- `with_clone_fn` supports unsized values and trait objects through an explicit
  clone function.

Clones entering the same partition converge on one published value. A holder
that reaches another thread without a relocation call remains correct and keeps
its carried value, although it may be less efficient.

## 3. Owner boundaries

An `Owner` identifies one runtime. When relocation has a known source and the
destination belongs to a different owner, `Arc::relocate` is a no-op. The holder
keeps its carried value and does not read, publish, or materialize a partition
for the foreign runtime. The holder records the source as the owner of its
carried value, but the shared storage remains unbound by that rejected move.

This preserves runtime-bound objects across relocation. They may continue to
use state associated with the original runtime and therefore operate less
efficiently, but relocation does not replace working state with a value built
for an unrelated owner.

Storage and holders bind independently and only once:

1. A newly constructed holder has an unbound carried value, and its storage is
   also unbound.
2. An accepted relocation or prepared insertion binds unowned storage to the
   destination owner.
3. Publishing or adopting a destination partition binds that holder's carried
   value to the destination owner.
4. A known cross-owner relocation instead binds an unbound holder to the
   source owner and leaves storage unchanged.

Once either binding is present, a different owner is rejected. Another clone
may already have bound the shared storage even while this holder still carries
an unbound or source-owned value, so both checks are required. When the source
is unknown, the source/destination owner relationship cannot be checked, but
the existing holder and storage bindings still are; an otherwise unbound move
may bind to the destination and follow the normal key lookup.

## 4. Concurrent storage

`storage::Storage<T, S>` uses `OnceLock<std::sync::Arc<T>>` for the
single-partition `PerProcess` strategy and
`DashMap<S::Key, std::sync::Arc<T>>` for partitioned strategies.

Storage is bound to the first runtime owner that populates or relocates it.
Threads belonging to another owner cannot read or publish its partitions.

`Storage::insert` publishes a value only when a key is empty and returns the
rejected value when another value was already published or the storage belongs
to another owner. Its public error type remains the rejected `Arc`; internally,
the insertion path distinguishes an occupied partition from an owner mismatch
so callers and tests do not lose that diagnostic. `Storage::get` clones the
value for a key.

Lazy materialization holds the destination map entry while the factory runs.
This guarantees one published value per key, but factory code must not re-enter
the same storage. An unrelated key that hashes to the same DashMap shard may
wait for the factory, so factories should remain short and non-blocking.

Published entries remain until the shared storage is dropped. `PerThread` is
therefore intended for runtimes with a stable worker set; runtimes that create
unbounded transient threads should use a coarser strategy or bound the lifetime
of the containing `Arc`.

## 5. Prepared storage and unsized values

Callers that know partition values in advance may populate `Storage` and pass it
to `Arc::from_storage` with the current `Thread`. The current key must already
have a value. Missing later keys retain the carried value when manual storage
mode materializes them.

`T` may be unsized. `Arc<dyn Trait, S>`, boxed constructors, explicit clone
functions, and prepared `std::sync::Arc<T>` values remain supported.

## 6. Design tenets

- One stable relocation contract comes from `thread_aware_core`.
- Relocation improves performance but never establishes correctness.
- Dereferencing is lock-free and does not consult the map.
- Partition values are published once per key.
- Crossing runtime owners keeps the working carried value.
- Runtime discovery and pinning are outside this crate.
- Third-party implementations belong with the third-party type.
