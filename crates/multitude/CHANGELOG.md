# Changelog

## [0.6.1] - 2026-07-07

- 🔧 Maintenance

  - Now requires `0.6.0` of `bytesbuf`

- ✨ Features

  - Adopt custom AllocError type for Ralf-compatibility ([#536](https://github.com/microsoft/oxidizer/pull/536))
  - Introduce Alloc<T> and reintroduce Rc<T> ([#521](https://github.com/microsoft/oxidizer/pull/521))
  - add arena-backed hashbrown HashMap/HashSet support ([#517](https://github.com/microsoft/oxidizer/pull/517))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - re-release all packages with LFS-free tarballs ([#531](https://github.com/microsoft/oxidizer/pull/531))

## [0.5.1] - 2026-06-26

- 🔧 Maintenance

  - Now requires `0.5.6` of `bytesbuf`

- ✨ Features

  - add arena-backed hashbrown HashMap/HashSet support ([#517](https://github.com/microsoft/oxidizer/pull/517))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

## [0.3.1] - 2026-06-18

- 🔧 Maintenance

  - Now requires `0.5.5` of `bytesbuf`

## [0.2.0] - 2026-06-11

- 🔧 Maintenance

  - Now requires `0.5.4` of `bytesbuf`
  - Now requires `0.7.4` of `thread_aware`
  - Now requires `0.7.4` of `thread_aware_macros`
  - Now requires `0.7.3` of `thread_aware_macros_impl`

- ✨ Features

  - Rewrite the multitude crate ([#471](https://github.com/microsoft/oxidizer/pull/471))

## [0.1.3] - 2026-06-04

- 🔧 Maintenance

  - bump `thread_aware` to 0.7.3 (includes derive macro updates via `thread_aware_macros_impl` 0.7.2)

## [0.1.2] - 2026-06-02

- 🔧 Maintenance

  - Now requires `0.5.2` of `bytesbuf`
  - Now requires `0.7.2` of `thread_aware`
  - Now requires `0.7.2` of `thread_aware_macros`
  - Now requires `0.7.1` of `thread_aware_macros_impl`

- ✨ Features

  - release all packages for MSRV increment ([#463](https://github.com/microsoft/oxidizer/pull/463))

- 🐛 Bug Fixes

  - gate gungraun to linux ([#456](https://github.com/microsoft/oxidizer/pull/456))
  - examples collision ([#455](https://github.com/microsoft/oxidizer/pull/455))
  - tighten allocator safety proofs and docs ([#443](https://github.com/microsoft/oxidizer/pull/443))
  - ensure that `cargo test` passes on a clean checkout ([#441](https://github.com/microsoft/oxidizer/pull/441))

- ⚡ Performance

  - split allocator hot paths from cold refill/oversized… ([#442](https://github.com/microsoft/oxidizer/pull/442))

- ✔️ Tasks

  - Tidy cargo dependencies to unbreak publishing ([#466](https://github.com/microsoft/oxidizer/pull/466))
  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - bump templated_uri version ([#444](https://github.com/microsoft/oxidizer/pull/444))

- ♻️ Code Refactoring

  - consolidate unsafe idioms behind shared helpers ([#447](https://github.com/microsoft/oxidizer/pull/447))

## [0.1.1] - 2026-06-01

- 🔧 Maintenance

  - Now requires `0.7.1` of `thread_aware`
  - Now requires `0.7.1` of `thread_aware_macros`

- 🐛 Bug Fixes

  - gate gungraun to linux ([#456](https://github.com/microsoft/oxidizer/pull/456))
  - examples collision ([#455](https://github.com/microsoft/oxidizer/pull/455))
  - tighten allocator safety proofs and docs ([#443](https://github.com/microsoft/oxidizer/pull/443))
  - ensure that `cargo test` passes on a clean checkout ([#441](https://github.com/microsoft/oxidizer/pull/441))

- ⚡ Performance

  - split allocator hot paths from cold refill/oversized… ([#442](https://github.com/microsoft/oxidizer/pull/442))

- ✔️ Tasks

  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - bump templated_uri version ([#444](https://github.com/microsoft/oxidizer/pull/444))

- ♻️ Code Refactoring

  - consolidate unsafe idioms behind shared helpers ([#447](https://github.com/microsoft/oxidizer/pull/447))

## [0.1.0] - 2026-05-21

- ✨ Features

  - Initial release of the `multitude` arena allocator.
