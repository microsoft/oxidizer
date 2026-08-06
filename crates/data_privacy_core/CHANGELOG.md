# Changelog

## [0.1.3] - 2026-08-06

- ✨ Features

  - add REST transcoding for gRPC services ([#600](https://github.com/microsoft/oxidizer/pull/600))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - re-release all packages with LFS-free tarballs ([#531](https://github.com/microsoft/oxidizer/pull/531))

- 🏗️ Build System

  - adopt cargo-anvil check catalog (github backend) ([#534](https://github.com/microsoft/oxidizer/pull/534))

- 🔄 Continuous Integration

  - add cargo-machete for workspace-wide unused-dependency detection ([#578](https://github.com/microsoft/oxidizer/pull/578))

## [0.1.2] - 2026-06-26

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - release all packages for MSRV 1.93 ([#492](https://github.com/microsoft/oxidizer/pull/492))
  - bump MSRV to 1.93 and adopt new stdlib helpers ([#474](https://github.com/microsoft/oxidizer/pull/474))

## [0.1.1] - 2026-06-11

- ✔️ Tasks

  - bump MSRV to 1.93 and adopt new stdlib helpers ([#474](https://github.com/microsoft/oxidizer/pull/474))

## [0.1.0] - 2026-05-28

- ✨ Features

  - Initial release.
  - Core data classification types and traits extracted from `data_privacy`: `Classified`, `DataClass`, `IntoDataClass`, `RedactedDebug`, `RedactedDisplay`, `RedactedToString`, `Redactor`.
