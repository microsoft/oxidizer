# Changelog

## [0.10.1] - 2026-06-10

- ✔️ Tasks

  - technical release

## [0.10.0] - 2026-06-03

- ✨ Features

  - Generated code now uses `&dyn Redactor` instead of `&RedactionEngine` to match updated trait signatures in `data_privacy`.

## [0.9.2] - 2026-06-02

- ✨ Features

  - release all packages for MSRV increment ([#463](https://github.com/microsoft/oxidizer/pull/463))
  - Add named-field struct support and trait bounds in #[classified] ([#261](https://github.com/microsoft/oxidizer/pull/261))

- 🐛 Bug Fixes

  - Replace removed doc_auto_cfg feature with doc_cfg ([#178](https://github.com/microsoft/oxidizer/pull/178))

- 📚 Documentation

  - Normalize feature handling for docs.rs ([#153](https://github.com/microsoft/oxidizer/pull/153))
  - Fix the CI badge ([#154](https://github.com/microsoft/oxidizer/pull/154))

- ✔️ Tasks

  - Tidy cargo dependencies to unbreak publishing ([#466](https://github.com/microsoft/oxidizer/pull/466))
  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - Improve our crate's repository property. ([#246](https://github.com/microsoft/oxidizer/pull/246))
  - Replace cargo-rdme by cargo-doc2readme ([#148](https://github.com/microsoft/oxidizer/pull/148))

## [0.9.1] - 2026-06-01

- ✨ Features

  - Add named-field struct support and trait bounds in #[classified] ([#261](https://github.com/microsoft/oxidizer/pull/261))

- 🐛 Bug Fixes

  - Replace removed doc_auto_cfg feature with doc_cfg ([#178](https://github.com/microsoft/oxidizer/pull/178))

- 📚 Documentation

  - Normalize feature handling for docs.rs ([#153](https://github.com/microsoft/oxidizer/pull/153))
  - Fix the CI badge ([#154](https://github.com/microsoft/oxidizer/pull/154))

- ✔️ Tasks

  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - Improve our crate's repository property. ([#246](https://github.com/microsoft/oxidizer/pull/246))
  - Replace cargo-rdme by cargo-doc2readme ([#148](https://github.com/microsoft/oxidizer/pull/148))

## [0.9.0] - 2025-12-16

- ✨ Features

  - Better serialization and perf.

- 🔄 Continuous Integration

  - Linting for Cargo.toml files ([#110](https://github.com/microsoft/oxidizer/pull/110))
  - Add license check for dependencies ([#105](https://github.com/microsoft/oxidizer/pull/105))

- 🧩 Miscellaneous

  - Make it to_redacted_string to avoid annoying downstream conflicts. ([#143](https://github.com/microsoft/oxidizer/pull/143))
  - Enable the allow_attribute lint and fix warnings. ([#111](https://github.com/microsoft/oxidizer/pull/111))

## [0.7.0] - 2025-12-05

- ✨ Features

  - Improve data_privacy UX ([#89](https://github.com/microsoft/oxidizer/pull/89))
