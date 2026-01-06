# Changelog

## [0.7.0] - 2026-01-06

- ✨ Features

  - Make std::sync::Arc not implement ThreadAware ([#149](https://github.com/microsoft/oxidizer/pull/149))

- 🐛 Bug Fixes

  - Replace removed doc_auto_cfg feature with doc_cfg ([#178](https://github.com/microsoft/oxidizer/pull/178))

- 📚 Documentation

  - Normalize feature handling for docs.rs ([#153](https://github.com/microsoft/oxidizer/pull/153))
  - Fix the CI badge ([#154](https://github.com/microsoft/oxidizer/pull/154))

- ✔️ Tasks

  - Replace cargo-rdme by cargo-doc2readme ([#148](https://github.com/microsoft/oxidizer/pull/148))

- 🔄 Continuous Integration

  - Add spell checker ([#158](https://github.com/microsoft/oxidizer/pull/158))

- 🧩 Miscellaneous

  - remove non-existing feature ([#159](https://github.com/microsoft/oxidizer/pull/159))

## [0.6.0] - 2025-12-15

- ✔️ Tasks

  - thread_aware does not need to depend on mutants ([#129](https://github.com/microsoft/oxidizer/pull/129))
  - Add tests for missing mutants ([#126](https://github.com/microsoft/oxidizer/pull/126))

- 🧩 Miscellaneous

  - Shouldn't have been renamed due to stuttering in re-exports. ([#125](https://github.com/microsoft/oxidizer/pull/125))

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

