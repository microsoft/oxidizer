# Changelog

## [0.2.6] - 2026-07-24

- 🔧 Maintenance

  - Now requires `0.3.9` of `ohno`

- ✨ Features

  - add REST transcoding for gRPC services ([#600](https://github.com/microsoft/oxidizer/pull/600))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - re-release all packages with LFS-free tarballs ([#531](https://github.com/microsoft/oxidizer/pull/531))

- 🏗️ Build System

  - adopt cargo-anvil check catalog (github backend) ([#534](https://github.com/microsoft/oxidizer/pull/534))

## [0.2.5] - 2026-06-26

- 🔧 Maintenance

  - Now requires `0.3.8` of `ohno`

- ✨ Features

  - enable and enforce unreachable_pub lint ([#493](https://github.com/microsoft/oxidizer/pull/493))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - release ohno 0.3.7 and cascade dependents ([#524](https://github.com/microsoft/oxidizer/pull/524))
  - release all packages for MSRV 1.93 ([#492](https://github.com/microsoft/oxidizer/pull/492))

## [0.2.4] - 2026-06-24

- 🔧 Maintenance

  - Now requires `0.3.7` of `ohno`

- ✨ Features

  - enable and enforce unreachable_pub lint ([#493](https://github.com/microsoft/oxidizer/pull/493))

- ✔️ Tasks

  - release all packages for MSRV 1.93 ([#492](https://github.com/microsoft/oxidizer/pull/492))

## [0.2.3] - 2026-06-11

- 🔧 Maintenance

  - Now requires `0.3.6` of `ohno`
  - Now requires `0.3.4` of `ohno_macros`

## [0.2.2] - 2026-06-04

- 🔧 Maintenance

  - bump `ohno` to 0.3.5 (transitively updates `ohno_macros` to 0.3.3)

## [0.2.1] - 2026-06-04

- 🧩 Miscellaneous

  - maintenance version bump

## [0.2.0] - 2026-06-02

- ✨ Features

  - release new `fetch_tls` crate
