# Changelog

## [0.2.0] - 2026-03-17

- ⚠️ Breaking

  - add `CustomSpawnerBuilder` for composing multilayered spawner ([#308](https://github.com/microsoft/oxidizer/pull/308))

- ✨ Features

  - improve ergonomics of BytesView::as_read() ([#272](https://github.com/microsoft/oxidizer/pull/272))

- ✔️ Tasks

  - Improve our crate's repository property. ([#246](https://github.com/microsoft/oxidizer/pull/246))

## 0.1.0

Initial release.

- `Spawner` trait for abstracting async task spawning across runtimes
- `TokioSpawner` implementation for the Tokio runtime (requires `tokio` feature)
