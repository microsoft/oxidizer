# Changelog

## [0.2.1]

- ✨ Features

  - Add `InMemoryCacheBuilder::on_eviction` for observing entry removals, along with the new public [`RemovalCause`] enum.
  - Add `InMemoryCacheBuilder::with_eviction_telemetry` as a marker for the `cachet` host crate to install built-in eviction telemetry via `CacheBuilder::memory_with`.

## [0.2.0] - 2026-05-19

- ✔️ Tasks

  - release HTTP and bytesbuf dependents
  - release thread-aware-dependent crates

## [0.1.1] - 2026-05-18

- ✨ Features

  - Improve thread_aware APIs and anyspawn rt compat. ([#403](https://github.com/microsoft/oxidizer/pull/403))
  - Add LRU eviction policy for in memory cache ([#369](https://github.com/microsoft/oxidizer/pull/369))

## [0.1.0]

Initial release.
