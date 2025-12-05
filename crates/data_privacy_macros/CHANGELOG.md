# Changelog

## [0.7.0] - 2025-12-05

- 🧩 Miscellaneous

  - Don't make std types redacted, annotate fields with `#[unredacted]` instead.
  - Fix CI complaints.
  - Format again.
  - Remove `RedactedToString` derive, since it was replaced by blanket trait impl.
  - Fix documentation.
  - Also align data privacy with Rust Guidelines and clean up module structure.
  - Align macro crate with existing structure.
  - Re-enable tests.
  - Update main app example and also log entire type.
  - Fix impl issues and make it work with std.
  - Implement derives.
  - More design work.
  - Change #[classified] macro to implement its own derives.

## [0.4.0] - 2025-11-27

- ✨ Features

  - Major improvements in the data_privacy crate ([#50](https://github.com/microsoft/oxidizer/pull/50))
  - Introduce the ohno and ohno_macros crates ([#53](https://github.com/microsoft/oxidizer/pull/53))
  - Introduce the #[classified] macro ([#48](https://github.com/microsoft/oxidizer/pull/48))
  - Make RedactionEngine clonable. ([#13](https://github.com/microsoft/oxidizer/pull/13))

- 📚 Documentation

  - A few doc-related fixes ([#80](https://github.com/microsoft/oxidizer/pull/80))
  - Add logos and favicons to our crate docs ([#44](https://github.com/microsoft/oxidizer/pull/44))
  - Add some panache to the repo ([#14](https://github.com/microsoft/oxidizer/pull/14))

- ✔️ Tasks

  - A few fixes ([#22](https://github.com/microsoft/oxidizer/pull/22))
  - Add add-crate.ps1 which initializes a new crate folder in the repo ([#19](https://github.com/microsoft/oxidizer/pull/19))
  - Get CHANGELOG and version bump handling under control ([#17](https://github.com/microsoft/oxidizer/pull/17))

- 🧩 Miscellaneous

  - Increase consistency of a few little things here and there ([#65](https://github.com/microsoft/oxidizer/pull/65))

## [0.3.0] - 2025-09-15

- 🧩 Miscellaneous

  - Fixes to enable producing a proper crates.io version ([#11](https://github.com/microsoft/oxidizer/pull/11))
  - Organize changelogs ([#8](https://github.com/microsoft/oxidizer/pull/8))

## [0.2.9] - 2025-09-11

- 🧩 Miscellaneous

  - Bump version for packages: -p data_privacy_macros -p data_privacy
  - One more bump

## [0.2.8] - 2025-09-11

- 🧩 Miscellaneous

  - Bump version for packages: -p data_privacy_macros
  - Another bump

## [0.2.7] - 2025-09-10

- 🧩 Miscellaneous

  - Bump version for packages: -p data_privacy_macros
  - Another bump

## [0.2.6] - 2025-09-10

- ✔️ Tasks

  - Release
  - Release

- 🧩 Miscellaneous

  - Bump version for packages: -p data_privacy_macros
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

