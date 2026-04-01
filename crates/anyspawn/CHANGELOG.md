# Changelog

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
