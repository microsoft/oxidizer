# Changelog

## [0.6.5] - 2026-06-05

- 🔧 Maintenance

  - bump `recoverable` to 0.1.5

## [0.6.4] - 2026-06-04

- 🔧 Maintenance

  - bump `ohno` to 0.3.5 (transitively updates `ohno_macros` to 0.3.3)

## [0.6.3] - 2026-06-04

- 🔧 Maintenance

  - bump `thread_aware` to 0.7.3 (includes derive macro updates via `thread_aware_macros_impl` 0.7.2)

## [0.6.2] - 2026-06-02

- 🔧 Maintenance

  - Now requires `0.3.2` of `layered`

- ✨ Features

  - release all packages for MSRV increment ([#463](https://github.com/microsoft/oxidizer/pull/463))

- ✔️ Tasks

  - Release all packages again to unbreak GitHub publishing (part N+1) ([#467](https://github.com/microsoft/oxidizer/pull/467))
  - Tidy cargo dependencies to unbreak publishing ([#466](https://github.com/microsoft/oxidizer/pull/466))

- 🧩 Miscellaneous

  - Update tool versions ([#462](https://github.com/microsoft/oxidizer/pull/462))

## [0.6.1] - 2026-06-02

- 🔧 Maintenance

  - Now requires `0.5.2` of `anyspawn`
  - Now requires `0.5.2` of `bytesbuf`
  - Now requires `0.3.1` of `cachet_memory`
  - Now requires `0.2.1` of `cachet_tier`
  - Now requires `0.3.4` of `ohno`
  - Now requires `0.3.2` of `ohno_macros`
  - Now requires `0.1.4` of `recoverable`
  - Now requires `0.7.2` of `thread_aware`
  - Now requires `0.7.2` of `thread_aware_macros`
  - Now requires `0.7.1` of `thread_aware_macros_impl`
  - Now requires `0.3.2` of `tick`
  - Now requires `0.2.2` of `uniflight`

- ✨ Features

  - release all packages for MSRV increment ([#463](https://github.com/microsoft/oxidizer/pull/463))

- ✔️ Tasks

  - Tidy cargo dependencies to unbreak publishing ([#466](https://github.com/microsoft/oxidizer/pull/466))

- 🧩 Miscellaneous

  - Update tool versions ([#462](https://github.com/microsoft/oxidizer/pull/462))

## [0.6.0] - 2026-06-01

- ⚠️ Breaking

  - Now requires `0.5.1` of `anyspawn`
  - Now requires `0.2.0` of `cachet_service`
  - Now requires `0.2.0` of `cachet_tier`
  - Now requires `0.3.1` of `layered`
  - Now requires `0.3.3` of `ohno`
  - Now requires `0.3.1` of `ohno_macros`
  - Now requires `0.1.3` of `recoverable`
  - Now requires `0.7.1` of `thread_aware`
  - Now requires `0.7.1` of `thread_aware_macros`
  - Now requires `0.3.1` of `tick`

- ✨ Features

  - add configurable ttl on stampede protected cache, eviction telemetry ([#454](https://github.com/microsoft/oxidizer/pull/454))

- ✔️ Tasks

  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))

## [0.5.1] - 2026-05-21

- ✨ Features

  - Add `get_or_insert_with` and `try_get_or_insert_with` methods that accept closures returning `CacheEntry<V>`, enabling per-entry TTL control on cache-miss computations.
  - Add eviction telemetry via `cache.eviction` and `cache.expired`, opt-in through `InMemoryCacheBuilder::with_eviction_telemetry` together with the new `CacheBuilder::memory_with` helper.

## [0.5.0] - 2026-05-19

- ✔️ Tasks

  - release HTTP and bytesbuf dependents
  - release thread-aware-dependent crates

## [0.4.0] - 2026-05-18

- 🔧 Maintenance

  - bump `cachet_memory` to 0.1.1

- ⚠️ Breaking

  - Simplify cachet builder return type ([#422](https://github.com/microsoft/oxidizer/pull/422))
  - Make cachet telemetry more user-friendly ([#420](https://github.com/microsoft/oxidizer/pull/420))

- ✨ Features

  - introduce a new "routing" module ([#389](https://github.com/microsoft/oxidizer/pull/389))

## [0.3.0] - 2026-05-14

- ⚠️ Breaking

  - update the `metrics` and `logs` feature APIs to use OpenTelemetry 0.32 types ([#417](https://github.com/microsoft/oxidizer/pull/417))

- ✨ Features

  - add serialization support with PostcardEncoder/PostcardCodec ([#377](https://github.com/microsoft/oxidizer/pull/377))

- ✔️ Tasks

  - enforce nightly formatting ([#407](https://github.com/microsoft/oxidizer/pull/407))
  - upgrade opentelemetry crates to 0.32.0 ([#417](https://github.com/microsoft/oxidizer/pull/417))

## [0.2.0] - 2026-05-06

- ✔️ Tasks

  - release a new version of tick crate ([#387](https://github.com/microsoft/oxidizer/pull/387))

- ♻️ Code Refactoring

  - Rename FallbackPromotionPolicy to InsertPolicy and move it to CacheWrapper instead of on the FallbackCache ([#397](https://github.com/microsoft/oxidizer/pull/397))

## [0.1.1] - 2026-04-22

- 🔧 Maintenance

  - bump `tick` to 0.2.2

## [0.1.0]

Initial release.
