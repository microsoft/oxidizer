# Changelog

## [0.4.0] - 2026-05-18

- ⚠️ Breaking

  - update cache thread-awareness dependencies to `thread_aware` 0.7-compatible crate versions

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
