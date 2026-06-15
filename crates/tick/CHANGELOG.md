# Changelog

## [0.3.4] - 2026-06-11

- 🔧 Maintenance

  - Now requires `0.7.4` of `thread_aware`
  - Now requires `0.7.4` of `thread_aware_macros`
  - Now requires `0.7.3` of `thread_aware_macros_impl`

## [0.3.3] - 2026-06-04

- 🔧 Maintenance

  - bump `thread_aware` to 0.7.3 (includes derive macro updates via `thread_aware_macros_impl` 0.7.2)

## [0.3.2] - 2026-06-02

- 🔧 Maintenance

  - Now requires `0.7.2` of `thread_aware`
  - Now requires `0.7.2` of `thread_aware_macros`
  - Now requires `0.7.1` of `thread_aware_macros_impl`

- ✨ Features

  - release all packages for MSRV increment ([#463](https://github.com/microsoft/oxidizer/pull/463))

- 🐛 Bug Fixes

  - examples collision ([#455](https://github.com/microsoft/oxidizer/pull/455))

- ✔️ Tasks

  - Tidy cargo dependencies to unbreak publishing ([#466](https://github.com/microsoft/oxidizer/pull/466))
  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))

- 🧩 Miscellaneous

  - Update tool versions ([#462](https://github.com/microsoft/oxidizer/pull/462))

## [0.3.1] - 2026-06-01

- 🔧 Maintenance

  - Now requires `0.7.1` of `thread_aware`
  - Now requires `0.7.1` of `thread_aware_macros`

- 🐛 Bug Fixes

  - examples collision ([#455](https://github.com/microsoft/oxidizer/pull/455))

- ✔️ Tasks

  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))

## [0.3.0] - 2026-05-11

- ⚠️ Breaking

  - new major version of `thread_aware` ([#403](https://github.com/microsoft/oxidizer/pull/403))

## [0.2.2] - 2026-04-22

- 🐛 Bug Fixes

  - fix `Clock` on Tokio no longer ticking after relocation across threads ([#386](https://github.com/microsoft/oxidizer/pull/386))

## [0.2.1] - 2026-02-27

- ✨ Features

  - custom `Debug` implementation for `Clock` and `ClockControl` ([#275](https://github.com/microsoft/oxidizer/pull/275))

## [0.2.0] - 2026-02-13

- ✨ Features

  - implement thread_aware in tick ([#255](https://github.com/microsoft/oxidizer/pull/255))
  - rename `MIN` to `UNIX_EPOCH` ([#262](https://github.com/microsoft/oxidizer/pull/262))

## [0.1.2] - 2026-01-05

- ✨ Features

  - implement display extension for SystemTime

## [0.1.1] - 2025-12-29

- 🐛 Bug Fixes

  - handle timestamps before Unix epoch in ISO8601 formatting

## [0.1.0] - 2025-12-16

- ✨ Features

  - introduce tick crate ([#106](https://github.com/microsoft/oxidizer/pull/106))

- ✔️ Tasks

  - update Rust version and fix the build ([#139](https://github.com/microsoft/oxidizer/pull/139))
