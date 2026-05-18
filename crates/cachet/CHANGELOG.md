# Changelog

## Unreleased

- ⚠️ Breaking Changes

  - Removed `metrics` feature and `enable_metrics()` builder method. Metrics will be reintroduced via a `tracing_subscriber::Layer` in a future release.
  - Removed `CacheOperation` and `CacheActivity` enums. Telemetry events are now identified by a single `cache.event` field instead of separate `cache.operation` and `cache.activity` fields.
 - The `opentelemetry` dependency is no longer pulled in by the `logs` feature.
 - Simplified `Cache<K, V, CT>` to `Cache<K, V>`. All builders now return the same type, making it easy to store caches without naming internal tier types.
 - Removed `Cache::inner()`. The underlying storage tier is no longer directly accessible.

- ✨ New Features

  - Added public `telemetry::attributes` module with constants for event names (`EVENT_HIT`, `EVENT_MISS`, etc.), field names (`FIELD_NAME`, `FIELD_EVENT`, `FIELD_DURATION_NS`), and the tracing target prefix (`TARGET`). Consumers can use these to build custom `tracing_subscriber::Layer` implementations.
  - Added `telemetry_subscriber` example demonstrating how to subscribe to cache events.
  - Each telemetry event now has a unique, self-descriptive name (e.g., `cache.get_error`, `cache.insert_rejected`) instead of reusing generic values like `cache.error` across operations.
>>>>>>> 7c8b6df240be400c109964c28e9b78fbbbcf0c71

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
