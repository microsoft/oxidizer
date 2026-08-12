# Changelog

## [0.2.0] - 2026-08-09

- 🔧 Maintenance

  - Now requires Rust `1.93.1` ([#629](https://github.com/microsoft/oxidizer/pull/629))

- ⚠️ Breaking

  - hide documentation modules from public API ([#635](https://github.com/microsoft/oxidizer/pull/635))
  - Now requires `0.4.0` of `ohno`

- 🏗️ Build System

  - adopt cargo-anvil check catalog (github backend) ([#534](https://github.com/microsoft/oxidizer/pull/534))

## [0.1.7] - 2026-06-26

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - release all packages for MSRV 1.93 ([#492](https://github.com/microsoft/oxidizer/pull/492))

## [0.1.5] - 2026-06-05

- 🔧 Maintenance

  - technical release

## [0.1.4] - 2026-06-02

- ✨ Features

  - release all packages for MSRV increment ([#463](https://github.com/microsoft/oxidizer/pull/463))
  - Improve thread_aware APIs and anyspawn rt compat. ([#403](https://github.com/microsoft/oxidizer/pull/403))
  - add recipes docs ([#381](https://github.com/microsoft/oxidizer/pull/381))

- ✔️ Tasks

  - Tidy cargo dependencies to unbreak publishing ([#466](https://github.com/microsoft/oxidizer/pull/466))
  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - enforce nightly formatting ([#407](https://github.com/microsoft/oxidizer/pull/407))

- 🧩 Miscellaneous

  - Update tool versions ([#462](https://github.com/microsoft/oxidizer/pull/462))

## [0.1.3] - 2026-06-01

- ✨ Features

  - Improve thread_aware APIs and anyspawn rt compat. ([#403](https://github.com/microsoft/oxidizer/pull/403))
  - add recipes docs ([#381](https://github.com/microsoft/oxidizer/pull/381))

- ✔️ Tasks

  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - enforce nightly formatting ([#407](https://github.com/microsoft/oxidizer/pull/407))

## [0.1.2] - 2026-04-12

- ✨ Features

  - implement `From<std::io::error::ErrorKind>` for `RecoveryInfo`

## [0.1.1] - 2026-03-03

- ✨ Features

  - add RecoveryKind::as_str for static string representation

## [0.1.0] - 2025-12-30

- ✨ Features

  - introduce the recoverable crate
