# Changelog

## [0.3.6] - 2026-07-24

- ✨ Features

  - add REST transcoding for gRPC services ([#600](https://github.com/microsoft/oxidizer/pull/600))
  - support type erasure of pooled handles to unsized types ([#585](https://github.com/microsoft/oxidizer/pull/585))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - upgrade alloc_tracker from 0.5.25 to 0.6.0 ([#513](https://github.com/microsoft/oxidizer/pull/513))
  - re-release all packages with LFS-free tarballs ([#531](https://github.com/microsoft/oxidizer/pull/531))

- 🏗️ Build System

  - adopt cargo-anvil check catalog (github backend) ([#534](https://github.com/microsoft/oxidizer/pull/534))

## [0.3.5] - 2026-06-26

- ✨ Features

  - enable and enforce unreachable_pub lint ([#493](https://github.com/microsoft/oxidizer/pull/493))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - release all packages for MSRV 1.93 ([#492](https://github.com/microsoft/oxidizer/pull/492))

## [0.3.3] - 2026-06-10

- ⚡ Performance

  - dynamic service does not allocate anymore ([#480](https://github.com/microsoft/oxidizer/pull/480))

## [0.3.2] - 2026-06-02

- ✨ Features

  - release all packages for MSRV increment ([#463](https://github.com/microsoft/oxidizer/pull/463))
  - improve ergonomics of BytesView::as_read() ([#272](https://github.com/microsoft/oxidizer/pull/272))

- ✔️ Tasks

  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - Improve our crate's repository property. ([#246](https://github.com/microsoft/oxidizer/pull/246))

## [0.3.1] - 2026-06-01

- ✨ Features

  - improve ergonomics of BytesView::as_read() ([#272](https://github.com/microsoft/oxidizer/pull/272))

- ✔️ Tasks

  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - Improve our crate's repository property. ([#246](https://github.com/microsoft/oxidizer/pull/246))

## [0.3.0] - 2026-01-22

- ✨ Features

  - rename Stack::build into Stack::into_service
  - documentation improvements

## [0.2.0] - 2026-01-21

- ✨ Features

  - add typed InterceptFuture for tower Service impl ([#207](https://github.com/microsoft/oxidizer/pull/207))

## [0.1.0] - 2026-01-12

- ✨ Features

  - introduce layered crate ([#189](https://github.com/microsoft/oxidizer/pull/189))
