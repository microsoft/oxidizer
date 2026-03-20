# Changelog

## [0.11.0] - 2026-03-20

- ✨ Features

  - Support redaction suppression.
  - Add named-field struct support and trait bounds in #[classified] ([#261](https://github.com/microsoft/oxidizer/pull/261))

## [0.10.1] - 2026-01-14

- 🐛 Bug Fixes

  - Replace removed doc_auto_cfg feature with doc_cfg ([#178](https://github.com/microsoft/oxidizer/pull/178))

- 📚 Documentation

  - Normalize feature handling for docs.rs ([#153](https://github.com/microsoft/oxidizer/pull/153))
  - Fix the CI badge ([#154](https://github.com/microsoft/oxidizer/pull/154))

- ✔️ Tasks

  - Replace cargo-rdme by cargo-doc2readme ([#148](https://github.com/microsoft/oxidizer/pull/148))

- 🔄 Continuous Integration

  - Add spell checker ([#158](https://github.com/microsoft/oxidizer/pull/158))

## [0.10.0] - 2025-12-17

- ✨ Features

  - Better serialization and perf. ([#133](https://github.com/microsoft/oxidizer/pull/133))

- 🐛 Bug Fixes

  - Strip links in readme.md generation because cargo-rdme has problems with link comprehension ([#146](https://github.com/microsoft/oxidizer/pull/146))

## [0.9.0] - 2025-12-16

- 🧩 Miscellaneous

  - Make it to_redacted_string to avoid annoying downstream conflicts. ([#143](https://github.com/microsoft/oxidizer/pull/143))

## [0.8.0] - 2025-12-10

- ✨ Features

  - Add a redactor that uses RapidHash ([#114](https://github.com/microsoft/oxidizer/pull/114))

- 🔄 Continuous Integration

  - Linting for Cargo.toml files ([#110](https://github.com/microsoft/oxidizer/pull/110))
  - Add license check for dependencies ([#105](https://github.com/microsoft/oxidizer/pull/105))

- 🧩 Miscellaneous

  - Enable the allow_attribute lint and fix warnings. ([#111](https://github.com/microsoft/oxidizer/pull/111))

## [0.7.0] - 2025-12-05

- ✨ Features

  - Improve data_privacy UX ([#89](https://github.com/microsoft/oxidizer/pull/89))

- ✔️ Tasks

  - Enable the missing_docs compiler lint. ([#102](https://github.com/microsoft/oxidizer/pull/102))
  - enable docs.rs documentation for feature-gated code ([#99](https://github.com/microsoft/oxidizer/pull/99))

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

