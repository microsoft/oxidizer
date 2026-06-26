# Changelog

## [0.5.5] - 2026-06-26

- 🔧 Maintenance

  - Now requires `0.7.4` of `thread_aware_macros_impl`

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - release all packages for MSRV 1.93 ([#492](https://github.com/microsoft/oxidizer/pull/492))

## [0.5.4] - 2026-06-11

- 🔧 Maintenance

  - Now requires `0.7.4` of `thread_aware`
  - Now requires `0.7.4` of `thread_aware_macros`
  - Now requires `0.7.3` of `thread_aware_macros_impl`

## [0.5.3] - 2026-06-04

- 🔧 Maintenance

  - bump `thread_aware` to 0.7.3 (includes derive macro updates via `thread_aware_macros_impl` 0.7.2)

## [0.5.2] - 2026-06-02

- 🔧 Maintenance

  - Now requires `0.7.2` of `thread_aware`
  - Now requires `0.7.2` of `thread_aware_macros`
  - Now requires `0.7.1` of `thread_aware_macros_impl`

- ✨ Features

  - release all packages for MSRV increment ([#463](https://github.com/microsoft/oxidizer/pull/463))

- ✔️ Tasks

  - Tidy cargo dependencies to unbreak publishing ([#466](https://github.com/microsoft/oxidizer/pull/466))
  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))

## [0.5.1] - 2026-06-01

- 🔧 Maintenance

  - Now requires `0.7.1` of `thread_aware`
  - Now requires `0.7.1` of `thread_aware_macros`

- ✔️ Tasks

  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))

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
