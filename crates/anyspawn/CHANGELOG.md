# Changelog

## Unreleased

## [0.5.0] - 2026-05-12

- ✨ Features

  - add `Spawner::spawn_blocking` for running synchronous, CPU-bound or blocking work on a dedicated thread (Tokio uses [`tokio::task::spawn_blocking`]).
  - extend the `SpawnCustom` trait with a `spawn_blocking` method, allowing custom runtimes to plug in their own blocking-task execution strategy.

- ⚠️ Breaking

  - `SpawnCustom` now requires a `spawn_blocking` method. Existing implementors must add this method to compile.
  - `CustomSpawnerBuilder::layer` now takes two closures — one for futures and one for blocking tasks. Pass an identity closure (`|t| t`) for either side to leave that task kind unchanged.

## [0.4.0] - 2026-05-07

- ✨ Features

  - add `Spawner::spawn_anywhere` for spawning futures built from [`ThreadAware`](thread_aware::ThreadAware) data, with the data relocated before the future is constructed ([#403](https://github.com/microsoft/oxidizer/pull/403)).
  - publicly export the `SpawnCustom` trait so custom runtimes can be implemented as named types instead of closures ([#403](https://github.com/microsoft/oxidizer/pull/403)).
  - re-export `thread_aware::closure::ThreadAwareAsyncFnOnce` for ergonomic use alongside `Spawner` ([#403](https://github.com/microsoft/oxidizer/pull/403)).

- ⚠️ Breaking

  - `Spawner::new_custom` now takes a `T: SpawnCustom + Clone` implementation instead of a `Fn(BoxedFuture)` closure. Wrap existing closures in a small struct that implements `SpawnCustom` ([#403](https://github.com/microsoft/oxidizer/pull/403)).
  - remove `Spawner::new_thread_aware`. Per-core isolation is now expressed by implementing `SpawnCustom` on a `ThreadAware` type and using `Spawner::new_custom`, or via `CustomSpawnerBuilder` ([#403](https://github.com/microsoft/oxidizer/pull/403)).
  - the set of `allowed_external_types` changed: `thread_aware::affinity::{MemoryAffinity, PinnedAffinity}` are no longer part of the public surface; `thread_aware::affinity::Affinity` and `thread_aware::closure::ThreadAwareAsyncFnOnce` are now exposed instead ([#403](https://github.com/microsoft/oxidizer/pull/403)).

## [0.3.0] - 2026-03-27

- ✨ Features

  - make Spawner usable without features ([#343](https://github.com/microsoft/oxidizer/pull/343))
  - allow creating spawner using tokio handle ([#341](https://github.com/microsoft/oxidizer/pull/341))
  - the Spawner is now thread aware ([#330](https://github.com/microsoft/oxidizer/pull/330))

- ⚠️ Breaking

  - the crate does not have any default features enabled anymore
  - the custom spawner is now always available

## [0.2.0] - 2026-03-17

- ⚠️ Breaking

  - add `CustomSpawnerBuilder` for composing multi-layer spawners ([#308](https://github.com/microsoft/oxidizer/pull/308))

## 0.1.0

Initial release.

- `Spawner` trait for abstracting async task spawning across runtimes
- `TokioSpawner` implementation for the Tokio runtime (requires `tokio` feature)
