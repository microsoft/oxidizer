# Changelog

## [0.3.0] - 2026-06-01

- ⚠️ Breaking

  - Now requires `0.2.0` of `cachet_tier`
  - Now requires `0.3.3` of `ohno`
  - Now requires `0.3.1` of `ohno_macros`
  - Now requires `0.1.3` of `recoverable`
  - Now requires `0.7.1` of `thread_aware`
  - Now requires `0.7.1` of `thread_aware_macros`

- ✨ Features

  - add configurable ttl on stampede protected cache, eviction telemetry ([#454](https://github.com/microsoft/oxidizer/pull/454))

- ✔️ Tasks

  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))

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
