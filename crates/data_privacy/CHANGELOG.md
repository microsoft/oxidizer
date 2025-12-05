# Changelog

## [0.7.0] - 2025-12-05

- ✔️ Tasks

  - Enable the missing_docs compiler lint. ([#102](https://github.com/microsoft/oxidizer/pull/102))
  - enable docs.rs documentation for feature-gated code ([#99](https://github.com/microsoft/oxidizer/pull/99))

- 🧩 Miscellaneous

  - Fix import.
  - Document more items.
  - Address missing documentation.
  - Use inherent method.
  - Fix after rebase.
  - Add declassification methods.
  - Add derive documentation.
  - Add module docs
  - Formatter.
  - Make Redactor non-public.
  - Make non-public.
  - Don't make Redactors public.
  - Add documentation.
  - Update README.
  - Format.
  - Don't make std types redacted, annotate fields with `#[unredacted]` instead.
  - Disable various tests for miri.
  - More CI complaints.
  - Fix CI complaints.
  - Format again.
  - Remove `RedactedToString` derive, since it was replaced by blanket trait impl.
  - Increase coverage.
  - Fix doc tests.
  - Format.
  - Fix clippy
  - Fix documentation.
  - Fix more tests and warnings.
  - Move out integration tests.
  - Fix some rendering bugs and more UX.
  - Rename wrapper and improve UX working with DataClasses
  - Move macros into module for documentation.
  - Also align data privacy with Rust Guidelines and clean up module structure.
  - Re-enable tests.
  - Update main app example and also log entire type.
  - Fix impl issues and make it work with std.
  - More design work.
  - Change #[classified] macro to implement its own derives.
  - Cleanup before reworking macro
  - Refactoring plan.
  - In middle of rework.

## [0.6.0] - 2025-11-27

- ✨ Features

  - Major improvements in the data_privacy crate ([#50](https://github.com/microsoft/oxidizer/pull/50))
  - Introduce the #[classified] macro ([#48](https://github.com/microsoft/oxidizer/pull/48))
  - Add fundle ([#39](https://github.com/microsoft/oxidizer/pull/39))

- 📚 Documentation

  - A few doc-related fixes ([#80](https://github.com/microsoft/oxidizer/pull/80))
  - Add logos and favicons to our crate docs ([#44](https://github.com/microsoft/oxidizer/pull/44))

- ✔️ Tasks

  - Enable unwrap_used and panic clippy lints ([#67](https://github.com/microsoft/oxidizer/pull/67))

## [0.5.0] - 2025-10-06

- 🐛 Bug Fixes

  - Fix buffer handling optimization as per issue #36 ([#37](https://github.com/microsoft/oxidizer/pull/37))

- ✔️ Tasks

  - A few fixes ([#22](https://github.com/microsoft/oxidizer/pull/22))
  - Add add-crate.ps1 which initializes a new crate folder in the repo ([#19](https://github.com/microsoft/oxidizer/pull/19))
  - Get CHANGELOG and version bump handling under control ([#17](https://github.com/microsoft/oxidizer/pull/17))

- 🔄 Continuous Integration

  - Add support for 'cargo check-external-types' ([#35](https://github.com/microsoft/oxidizer/pull/35))

## [0.4.0] - 2025-09-22

- ✨ Features

  - Make RedactionEngine clonable. ([#13](https://github.com/microsoft/oxidizer/pull/13))

- 📚 Documentation

  - Add some panache to the repo ([#14](https://github.com/microsoft/oxidizer/pull/14))

## [0.3.0] - 2025-09-15

- 🧩 Miscellaneous

  - Fixes to enable producing a proper crates.io version ([#11](https://github.com/microsoft/oxidizer/pull/11))
  - Organize changelogs ([#8](https://github.com/microsoft/oxidizer/pull/8))

## [0.2.11] - 2025-09-12

- 🧩 Miscellaneous

  - Bump data_privacy
  - One more bump
  - Bump privacy macros

## [0.2.9] - 2025-09-11

- ✔️ Tasks

  - Release
  - Release

- 🧩 Miscellaneous

  - Bump version for packages: -p data_privacy_macros -p data_privacy
  - One more bump
  - Another bump
  - Another bump
  - Bump another time
  - One more version bump
  - Bump to alpha version
  - Another version bump
  - More version changes
  - Version changes
  - Improve code coverage ([#6](https://github.com/microsoft/oxidizer/pull/6))
  - Few automation/CI improvements ([#5](https://github.com/microsoft/oxidizer/pull/5))

## [0.1.0] - 2025-08-22

- 🧩 Miscellaneous

  - Finish repo setup and checkin data_privacy crate ([#4](https://github.com/microsoft/oxidizer/pull/4))

