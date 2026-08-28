# Changelog

## [0.2.4] - 2026-08-28

- ✨ Features

  - add socket tuning and HTTP/2 stream window options ([#637](https://github.com/microsoft/oxidizer/pull/637))
  - add REST transcoding for gRPC services ([#600](https://github.com/microsoft/oxidizer/pull/600))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - re-release all packages with LFS-free tarballs ([#531](https://github.com/microsoft/oxidizer/pull/531))

- 🏗️ Build System

  - adopt cargo-anvil check catalog (github backend) ([#534](https://github.com/microsoft/oxidizer/pull/534))

## [0.2.3] - 2026-06-26

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - release all packages for MSRV 1.93 ([#492](https://github.com/microsoft/oxidizer/pull/492))

## [0.2.1] - 2026-06-05

- 🔧 Maintenance

  - technical release

## [0.2.0] - 2026-06-04

- ✨ Features

  - introduce fetch_options
