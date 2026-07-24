# Changelog

## [0.3.5] - 2026-07-24

- 🔧 Maintenance

  - Now requires `0.3.5` of `fundle_macros`

- ✨ Features

  - add REST transcoding for gRPC services ([#600](https://github.com/microsoft/oxidizer/pull/600))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - re-release all packages with LFS-free tarballs ([#531](https://github.com/microsoft/oxidizer/pull/531))

- 🏗️ Build System

  - adopt cargo-anvil check catalog (github backend) ([#534](https://github.com/microsoft/oxidizer/pull/534))

## [0.3.4] - 2026-06-26

- 🔧 Maintenance

  - Now requires `0.3.4` of `fundle_macros`

- ✨ Features

  - enable and enforce unreachable_pub lint ([#493](https://github.com/microsoft/oxidizer/pull/493))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - release all packages for MSRV 1.93 ([#492](https://github.com/microsoft/oxidizer/pull/492))

## [0.3.3] - 2026-06-11

- 🔧 Maintenance

  - Now requires `0.3.3` of `fundle_macros`
  - Now requires `0.3.3` of `fundle_macros_impl`

## [0.3.2] - 2026-06-02

- 🔧 Maintenance

  - Now requires `0.3.2` of `fundle_macros`
  - Now requires `0.3.2` of `fundle_macros_impl`

- ✨ Features

  - release all packages for MSRV increment ([#463](https://github.com/microsoft/oxidizer/pull/463))
  - Introduce the thread_aware crate ([#72](https://github.com/microsoft/oxidizer/pull/72))
  - Improve builder UX. ([#57](https://github.com/microsoft/oxidizer/pull/57))

- 🐛 Bug Fixes

  - Replace removed doc_auto_cfg feature with doc_cfg ([#178](https://github.com/microsoft/oxidizer/pull/178))

- 📚 Documentation

  - Normalize feature handling for docs.rs ([#153](https://github.com/microsoft/oxidizer/pull/153))
  - Fix the CI badge ([#154](https://github.com/microsoft/oxidizer/pull/154))
  - A few doc-related fixes ([#80](https://github.com/microsoft/oxidizer/pull/80))

- ✔️ Tasks

  - Tidy cargo dependencies to unbreak publishing ([#466](https://github.com/microsoft/oxidizer/pull/466))
  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - switch to cargo-heather ([#440](https://github.com/microsoft/oxidizer/pull/440))
  - Improve our crate's repository property. ([#246](https://github.com/microsoft/oxidizer/pull/246))
  - Replace cargo-rdme by cargo-doc2readme ([#148](https://github.com/microsoft/oxidizer/pull/148))
  - Enable the missing_docs compiler lint. ([#102](https://github.com/microsoft/oxidizer/pull/102))

- 🔄 Continuous Integration

  - Linting for Cargo.toml files ([#110](https://github.com/microsoft/oxidizer/pull/110))
  - Add license check for dependencies ([#105](https://github.com/microsoft/oxidizer/pull/105))

- 🧩 Miscellaneous

  - Enable the allow_attribute lint and fix warnings. ([#111](https://github.com/microsoft/oxidizer/pull/111))

## [0.3.1] - 2026-06-01

- 🔧 Maintenance

  - Now requires `0.3.1` of `fundle_macros`
  - Now requires `0.3.1` of `fundle_macros_impl`

- ✨ Features

  - Introduce the thread_aware crate ([#72](https://github.com/microsoft/oxidizer/pull/72))
  - Improve builder UX. ([#57](https://github.com/microsoft/oxidizer/pull/57))

- 🐛 Bug Fixes

  - Replace removed doc_auto_cfg feature with doc_cfg ([#178](https://github.com/microsoft/oxidizer/pull/178))

- 📚 Documentation

  - Normalize feature handling for docs.rs ([#153](https://github.com/microsoft/oxidizer/pull/153))
  - Fix the CI badge ([#154](https://github.com/microsoft/oxidizer/pull/154))
  - A few doc-related fixes ([#80](https://github.com/microsoft/oxidizer/pull/80))

- ✔️ Tasks

  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - switch to cargo-heather ([#440](https://github.com/microsoft/oxidizer/pull/440))
  - Improve our crate's repository property. ([#246](https://github.com/microsoft/oxidizer/pull/246))
  - Replace cargo-rdme by cargo-doc2readme ([#148](https://github.com/microsoft/oxidizer/pull/148))
  - Enable the missing_docs compiler lint. ([#102](https://github.com/microsoft/oxidizer/pull/102))

- 🔄 Continuous Integration

  - Linting for Cargo.toml files ([#110](https://github.com/microsoft/oxidizer/pull/110))
  - Add license check for dependencies ([#105](https://github.com/microsoft/oxidizer/pull/105))

- 🧩 Miscellaneous

  - Enable the allow_attribute lint and fix warnings. ([#111](https://github.com/microsoft/oxidizer/pull/111))

## [0.3.0] - 2025-11-20

- 🧩 Miscellaneous

  - Format.
  - Fix tests.
  - Move generated builder into submodule to hide fields.
  - Add getters.
  - More type magic.

## [0.2.1] - 2025-11-14

- 📚 Documentation

  - Add logos and favicons to our crate docs ([#44](https://github.com/microsoft/oxidizer/pull/44))
  - Added missing entries in README TOC ([#42](https://github.com/microsoft/oxidizer/pull/42))
  - More fundle docs ([#41](https://github.com/microsoft/oxidizer/pull/41))

- 🧩 Miscellaneous

  - Bump version. ([#49](https://github.com/microsoft/oxidizer/pull/49))

## [0.2.0] - 2025-10-21

- ✨ Features

  - Add fundle ([#39](https://github.com/microsoft/oxidizer/pull/39))

- ✔️ Tasks

  - Add logo files and other readme cleanup ([#40](https://github.com/microsoft/oxidizer/pull/40))

