# Changelog

## [0.2.7] - 2026-07-24

- 🔧 Maintenance

  - Now requires `0.3.9` of `ohno`

- ✨ Features

  - add REST transcoding for gRPC services ([#600](https://github.com/microsoft/oxidizer/pull/600))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ⚡ Performance

  - optimize the per-request URI hot path ([#556](https://github.com/microsoft/oxidizer/pull/556))

- ✔️ Tasks

  - re-release all packages with LFS-free tarballs ([#531](https://github.com/microsoft/oxidizer/pull/531))

- 🏗️ Build System

  - adopt cargo-anvil check catalog (github backend) ([#534](https://github.com/microsoft/oxidizer/pull/534))

## [0.2.6] - 2026-06-26

- 🔧 Maintenance

  - Now requires `0.3.8` of `ohno`

- ✨ Features

  - enable and enforce unreachable_pub lint ([#493](https://github.com/microsoft/oxidizer/pull/493))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - release ohno 0.3.7 and cascade dependents ([#524](https://github.com/microsoft/oxidizer/pull/524))
  - release all packages for MSRV 1.93 ([#492](https://github.com/microsoft/oxidizer/pull/492))

## [0.2.5] - 2026-06-24

- 🔧 Maintenance

  - Now requires `0.3.7` of `ohno`

- ✨ Features

  - enable and enforce unreachable_pub lint ([#493](https://github.com/microsoft/oxidizer/pull/493))

- ✔️ Tasks

  - release all packages for MSRV 1.93 ([#492](https://github.com/microsoft/oxidizer/pull/492))

## [0.2.4] - 2026-06-11

- 🔧 Maintenance

  - Now requires `0.3.6` of `ohno`
  - Now requires `0.3.4` of `ohno_macros`

## [0.2.3] - 2026-06-04

- 🔧 Maintenance

  - bump `ohno` to 0.3.5 (transitively updates `ohno_macros` to 0.3.3)

## [0.2.2] - 2026-06-02

- 🔧 Maintenance

  - Now requires `0.3.4` of `ohno`
  - Now requires `0.3.2` of `ohno_macros`

- ✨ Features

  - release all packages for MSRV increment ([#463](https://github.com/microsoft/oxidizer/pull/463))

- ✔️ Tasks

  - Tidy cargo dependencies to unbreak publishing ([#466](https://github.com/microsoft/oxidizer/pull/466))
  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))

## [0.2.1] - 2026-06-01

- 🔧 Maintenance

  - Now requires `0.3.3` of `ohno`
  - Now requires `0.3.1` of `ohno_macros`

- ✔️ Tasks

  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))

## [0.2.0] - 2026-05-11

- ⚠️ Breaking

  - API review and overall cleanup ([#391](https://github.com/microsoft/oxidizer/pull/391))

- ✨ Features

  - Support `Option<T>` fields in `#[templated]` structs for RFC 6570 undefined variable semantics. ([#408](https://github.com/microsoft/oxidizer/pull/408))

## [0.1.1] - 2026-04-16

- ✔️ Tasks

  - bump `ohno` dependency version

## [0.1.0] - 2026-03-18

- ✨ Features

  - Introduce the templated_uri crate family ([#265](https://github.com/microsoft/oxidizer/pull/265))
