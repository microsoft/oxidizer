# Changelog

## [0.8.0] - 2026-07-07

- ✨ Features

  - add ProcessorCount::AtMost for capped processor count ([#541](https://github.com/microsoft/oxidizer/pull/541))
  - enable and enforce unreachable_pub lint ([#493](https://github.com/microsoft/oxidizer/pull/493))
  - add feature-gated ThreadAware impls for 3rd-party crate types ([#478](https://github.com/microsoft/oxidizer/pull/478))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - re-release all packages with LFS-free tarballs ([#531](https://github.com/microsoft/oxidizer/pull/531))
  - release all packages for MSRV 1.93 ([#492](https://github.com/microsoft/oxidizer/pull/492))

## [0.7.5] - 2026-06-26

- 🔧 Maintenance

  - Now requires `0.7.5` of `thread_aware_macros`

- ✨ Features

  - enable and enforce unreachable_pub lint ([#493](https://github.com/microsoft/oxidizer/pull/493))
  - add feature-gated ThreadAware impls for 3rd-party crate types ([#478](https://github.com/microsoft/oxidizer/pull/478))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - release all packages for MSRV 1.93 ([#492](https://github.com/microsoft/oxidizer/pull/492))

## [0.7.4] - 2026-06-11

- 🔧 Maintenance

  - Now requires `0.7.4` of `thread_aware_macros`
  - Now requires `0.7.3` of `thread_aware_macros_impl`

## [0.7.3] - 2026-06-04

- 🔧 Maintenance

  - bump `thread_aware_macros` to 0.7.3 (includes `thread_aware_macros_impl` 0.7.2)

## [0.7.2] - 2026-06-02

- 🔧 Maintenance

  - Now requires `0.7.2` of `thread_aware_macros`
  - Now requires `0.7.1` of `thread_aware_macros_impl`

- ✨ Features

  - release all packages for MSRV increment ([#463](https://github.com/microsoft/oxidizer/pull/463))

- ✔️ Tasks

  - Tidy cargo dependencies to unbreak publishing ([#466](https://github.com/microsoft/oxidizer/pull/466))
  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - enforce nightly formatting ([#407](https://github.com/microsoft/oxidizer/pull/407))

## [0.7.1] - 2026-06-01

- 🔧 Maintenance

  - Now requires `0.7.1` of `thread_aware_macros`

- ✔️ Tasks

  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - enforce nightly formatting ([#407](https://github.com/microsoft/oxidizer/pull/407))

## [0.7.0] - 2026-05-07

- ⚠️ Breaking Changes

  - `ThreadAware` now requires `Send` as a supertrait.
  - `ThreadAware::relocated` has been renamed to `ThreadAware::relocate`, now takes `&mut self` instead of `self`, and no longer returns `Self`.
  - `Arc<T, S>` now supports `T: ?Sized` via `new_boxed` constructor accepting `fn() -> Box<T>`.

## [0.6.2] - 2026-02-13

- ✨ Features

  - introduce thread_aware::Arc::strong_count ([#253](https://github.com/microsoft/oxidizer/pull/253))

## [0.6.1] - 2026-01-20

- ✨ Features

  - Add `__private` module for selective trait reexports

## [0.6.0] - 2025-12-12

- 🧩 Miscellaneous

  - Shouldn't have been renamed due to stuttering in re-exports.

## [0.5.0] - 2025-12-11

- ✔️ Tasks

  - Improve documentation and clean up thread_aware crate root. ([#119](https://github.com/microsoft/oxidizer/pull/119))
  - Add missing documentation on thread_aware related crates ([#103](https://github.com/microsoft/oxidizer/pull/103))
  - Enable the missing_docs compiler lint. ([#102](https://github.com/microsoft/oxidizer/pull/102))
  - enable docs.rs documentation for feature-gated code ([#99](https://github.com/microsoft/oxidizer/pull/99))

- 🔄 Continuous Integration

  - Linting for Cargo.toml files ([#110](https://github.com/microsoft/oxidizer/pull/110))
  - Add license check for dependencies ([#105](https://github.com/microsoft/oxidizer/pull/105))

- 🧩 Miscellaneous

  - Enable the allow_attribute lint and fix warnings. ([#111](https://github.com/microsoft/oxidizer/pull/111))

## [0.4.0] - 2025-12-03

- ✨ Features

  - Rename Trc to Arc, remove exiting PerCore/PerNuma wrappers ([#96](https://github.com/microsoft/oxidizer/pull/96))
  - Add unknown MemoryAffinity ([#85](https://github.com/microsoft/oxidizer/pull/85))

- 📚 Documentation

  - Missing logo and favicon links for the thread_aware ([#84](https://github.com/microsoft/oxidizer/pull/84))

- 🧩 Miscellaneous

  - thread_aware 0.4.0 release ([#97](https://github.com/microsoft/oxidizer/pull/97))

## [0.3.0] - 2025-11-27

- 📚 Documentation

  - A few doc-related fixes ([#80](https://github.com/microsoft/oxidizer/pull/80))

- ♻️ Code Refactoring

  - Clean up Unaware type ([#78](https://github.com/microsoft/oxidizer/pull/78))

## [0.2.0] - 2025-11-26

- ✨ Features

  - Introduce the thread_aware crate ([#72](https://github.com/microsoft/oxidizer/pull/72))
