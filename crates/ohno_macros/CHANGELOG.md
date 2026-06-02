# Changelog

## [0.3.1] - 2026-06-01

- ✨ Features

  - improve ergonomics of BytesView::as_read() ([#272](https://github.com/microsoft/oxidizer/pull/272))

- 🐛 Bug Fixes

  - remove redundant `}}` arm in parse_display_template ([#395](https://github.com/microsoft/oxidizer/pull/395))
  - address some correctness and usability findings ([#278](https://github.com/microsoft/oxidizer/pull/278))

- ✔️ Tasks

  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - Improve our crate's repository property. ([#246](https://github.com/microsoft/oxidizer/pull/246))

## [0.3.0] - 2026-01-26

- ✨ Features

  - add AppError::downcast_ref and Into<StdError> ([#225](https://github.com/microsoft/oxidizer/pull/225))

## [0.2.0] - 2025-12-01

- ⚠️ Breaking

  - rename TraceInfo into EnrichmentEntry ([#92](https://github.com/microsoft/oxidizer/pull/92))
  - rename ohno::error_trace into enrich_err ([#86](https://github.com/microsoft/oxidizer/pull/86))

- 🧩 Miscellaneous

  - Enable unwrap_used and panic clippy lints ([#67](https://github.com/microsoft/oxidizer/pull/67))
  - Increase consistency of a few little things here and there ([#65](https://github.com/microsoft/oxidizer/pull/65))

## [0.1.0] - 2025-11-18

- ✨ Features

  - Introduce the ohno and ohno_macros crates ([#53](https://github.com/microsoft/oxidizer/pull/53))

