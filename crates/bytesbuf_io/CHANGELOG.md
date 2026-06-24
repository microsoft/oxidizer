# Changelog

## [0.5.7] - 2026-06-24

- 🔧 Maintenance

  - Now requires `0.3.7` of `ohno`

## [0.5.6] - 2026-06-18

- 🔧 Maintenance

  - Now requires `0.5.5` of `bytesbuf`

## [0.5.5] - 2026-06-11

- 🔧 Maintenance

  - Now requires `0.5.4` of `bytesbuf`
  - Now requires `0.3.6` of `ohno`
  - Now requires `0.3.4` of `ohno_macros`
  - Now requires `0.7.4` of `thread_aware`
  - Now requires `0.7.4` of `thread_aware_macros`
  - Now requires `0.7.3` of `thread_aware_macros_impl`

## [0.5.4] - 2026-06-04

- 🔧 Maintenance

  - bump `ohno` to 0.3.5 (transitively updates `ohno_macros` to 0.3.3)

## [0.5.3] - 2026-06-04

- 🔧 Maintenance

  - bump `thread_aware` to 0.7.3 (includes derive macro updates via `thread_aware_macros_impl` 0.7.2)

## [0.5.2] - 2026-06-02

- 🔧 Maintenance

  - Now requires `0.5.2` of `bytesbuf`
  - Now requires `0.3.4` of `ohno`
  - Now requires `0.3.2` of `ohno_macros`
  - Now requires `0.7.2` of `thread_aware`
  - Now requires `0.7.2` of `thread_aware_macros`
  - Now requires `0.7.1` of `thread_aware_macros_impl`

- ✨ Features

  - release all packages for MSRV increment ([#463](https://github.com/microsoft/oxidizer/pull/463))

- 🐛 Bug Fixes

  - ensure that `cargo test` passes on a clean checkout ([#441](https://github.com/microsoft/oxidizer/pull/441))

- ✔️ Tasks

  - Tidy cargo dependencies to unbreak publishing ([#466](https://github.com/microsoft/oxidizer/pull/466))

## [0.5.1] - 2026-06-01

- 🔧 Maintenance

  - Now requires `0.3.3` of `ohno`
  - Now requires `0.3.1` of `ohno_macros`
  - Now requires `0.7.1` of `thread_aware`
  - Now requires `0.7.1` of `thread_aware_macros`

- 🐛 Bug Fixes

  - ensure that `cargo test` passes on a clean checkout ([#441](https://github.com/microsoft/oxidizer/pull/441))

## [0.5.0] - 2026-05-18

- ⚠️ Breaking

  - update asynchronous I/O byte buffer types to `bytesbuf` 0.5

## [0.3.0] - 2026-02-16

- ✔️ Tasks

  - Improve our crate's repository property. ([#246](https://github.com/microsoft/oxidizer/pull/246))

- 🔄 Continuous Integration

  - automatically publish release notes ([#247](https://github.com/microsoft/oxidizer/pull/247))

## [0.2.0] - 2026-01-28

- 📚 Documentation

  - Add cross-crate link to `bytesbuf` in `bytesbuf_io` ([#186](https://github.com/microsoft/oxidizer/pull/186))

- ✔️ Tasks

  - Update ohno dependency ([#239](https://github.com/microsoft/oxidizer/pull/239))

## [0.1.1] - 2026-01-07

- ✨ Features

  - Migrate bytesbuf_io from private repo ([#181](https://github.com/microsoft/oxidizer/pull/181))

- 🧩 Miscellaneous

  - Reduce keyword count in bytesbuf_io to 5 so crates.io will accept it ([#185](https://github.com/microsoft/oxidizer/pull/185))
