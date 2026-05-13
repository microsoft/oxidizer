# Changelog

## Unreleased

- 💥 Breaking Changes

  - Simplified `Cache<K, V, CT>` to `Cache<K, V>`. All builders now return the same type, making it easy to store caches without naming internal tier types.
  - Removed `Cache::inner()`. The underlying storage tier is no longer directly accessible.

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
