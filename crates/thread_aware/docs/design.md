# `thread_aware` design

This document describes the user-visible behavior of `thread_aware::Arc` and
`storage::Storage`. The stable relocation contract is defined by
`thread_aware_core` and re-exported by `thread_aware`.

## 1. Core vocabulary

`thread_aware` re-exports `ThreadAware`, `Thread`, `Owner`, and `NumaNode` from
`thread_aware_core`. There is one authoritative relocation trait across the
package family.

Runtime integrations construct coordinates with `thread::ThreadBuilder`.
`ThreadBuilder::default()` creates a unique runtime owner. Clones of one builder
retain that owner, so they can build coordinates for every worker in the same
runtime. `numa_node` selects the nearest NUMA node and `build` adds the
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
- `PerNumaNode`, keyed by `NumaNode`.
- `PerProcess`, keyed by one constant process-wide value.

Strategies are sealed. The identifiers in `Thread` are opaque and not dense or
enumerable, so storage is keyed rather than indexed.

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
for the foreign runtime. Storage records its runtime owner, while each holder
records the owner of its carried value. The per-holder owner prevents a retained
value from being published after another clone has already bound shared storage
to the foreign runtime.

This preserves runtime-bound objects across relocation. They may continue to
use state associated with the original runtime and therefore operate less
efficiently, but relocation does not replace working state with a value built
for an unrelated owner.

When the source is unknown, the owner relationship cannot be checked.
Relocation follows the normal destination-key lookup.

## 4. Concurrent storage

`storage::Storage<T, S>` uses `DashMap<S::Key, std::sync::Arc<T>>`. New
partitioned storage reserves capacity for 32 entries by default. This is an
explicit bounded-runtime heuristic: runtimes configured with at most 32 initial
partitions avoid map growth, while larger runtimes grow normally. It is not an
empirically established partition limit.

Storage is bound to the first runtime owner that populates or relocates it.
Threads belonging to another owner cannot read or publish its partitions.

`Storage::insert` publishes a value only when a key is empty and returns the
rejected value when another value was already published. `Storage::get` clones
the value for a key.

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
