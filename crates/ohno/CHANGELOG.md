# Changelog

## [0.3.5] - 2026-06-04

- 🔧 Maintenance

  - bump `ohno_macros` to 0.3.3

## [0.3.4] - 2026-06-02

- 🔧 Maintenance

  - Now requires `0.3.2` of `ohno_macros`

- ✨ Features

  - release all packages for MSRV increment ([#463](https://github.com/microsoft/oxidizer/pull/463))
  - Improve thread_aware APIs and anyspawn rt compat. ([#403](https://github.com/microsoft/oxidizer/pull/403))
  - fine grained error labels ([#382](https://github.com/microsoft/oxidizer/pull/382))

- ✔️ Tasks

  - Tidy cargo dependencies to unbreak publishing ([#466](https://github.com/microsoft/oxidizer/pull/466))

- 🧩 Miscellaneous

  - Update tool versions ([#462](https://github.com/microsoft/oxidizer/pull/462))

## [0.3.3] - 2026-06-01

- 🔧 Maintenance

  - Now requires `0.3.1` of `ohno_macros`

- ✨ Features

  - Improve thread_aware APIs and anyspawn rt compat. ([#403](https://github.com/microsoft/oxidizer/pull/403))
  - fine grained error labels ([#382](https://github.com/microsoft/oxidizer/pull/382))

## [0.3.2] - 2026-04-15

- ✨ Features

  - introduce `ohno::ErrorLabel` ([#366](https://github.com/microsoft/oxidizer/pull/366))

## [0.3.1] - 2026-02-17

- 🐛 Bug Fixes

  - capture actual caller location instead of ohno internals ([#260](https://github.com/microsoft/oxidizer/pull/260))

## [0.3.0] - 2026-01-26

- ✨ Features

  - add AppError::downcast_ref and Into<StdError> ([#225](https://github.com/microsoft/oxidizer/pull/225))

## [0.2.1] - 2026-01-16

- ✨ Features

  - add AppError type for application level errors ([#192](https://github.com/microsoft/oxidizer/pull/192))

## [0.2.0] - 2025-12-01

- ⚠️ Breaking

  - rename TraceInfo into EnrichmentEntry ([#92](https://github.com/microsoft/oxidizer/pull/92))
  - rename ohno::error_trace into enrich_err ([#86](https://github.com/microsoft/oxidizer/pull/86))

- ✨ Features

  - make OhnoCore cloneable ([#79](https://github.com/microsoft/oxidizer/pull/79))

- 🧩 Miscellaneous

  - remove recursive dev dependency in ohno crate ([#69](https://github.com/microsoft/oxidizer/pull/69))
  - Enable unwrap_used and panic clippy lints ([#67](https://github.com/microsoft/oxidizer/pull/67))
  - Increase consistency of a few little things here and there ([#65](https://github.com/microsoft/oxidizer/pull/65))

## [0.1.0] - 2025-11-18

- ✨ Features

  - Introduce the ohno and ohno_macros crates ([#53](https://github.com/microsoft/oxidizer/pull/53))
