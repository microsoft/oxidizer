# Changelog

## [0.4.0] - 2025-12-03

- ♻️ Code Refactoring

  - Rework `MemoryAffinity` to expose an `Unknown` kind and rework `ThreadAware` trait accordingly ([#85](https://github.com/microsoft/oxidizer/pull/85))
  - Introduce `Arc<T, S>` type as a replacement for `PerCore<T>` and `PerNuma<T>`. ([#96](https://github.com/microsoft/oxidizer/pull/96))

## [0.3.0] - 2025-11-26

- 📚 Documentation

  - A few doc-related fixes

- ♻️ Code Refactoring

  - Clean up Unaware type ([#78](https://github.com/microsoft/oxidizer/pull/78))

## [0.2.0] - 2025-11-26

- ✨ Features

  - Introduce the thread_aware crate ([#72](https://github.com/microsoft/oxidizer/pull/72))

