# Changelog

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
