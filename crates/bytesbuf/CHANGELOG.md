# Changelog

## [0.2.0] - 2025-12-30

- ✨ Features

  - continued bytesbuf tidying ([#132](https://github.com/microsoft/oxidizer/pull/132))
  - Replace BytesBuf::inspect() with peek() that returns BytesView ([#128](https://github.com/microsoft/oxidizer/pull/128))

- 📚 Documentation

  - Normalize feature handling for docs.rs ([#153](https://github.com/microsoft/oxidizer/pull/153))
  - Fix the CI badge ([#154](https://github.com/microsoft/oxidizer/pull/154))

- ✔️ Tasks

  - Replace cargo-rdme by cargo-doc2readme ([#148](https://github.com/microsoft/oxidizer/pull/148))
  - cleanup bytesbuf docs ([#122](https://github.com/microsoft/oxidizer/pull/122))

- 🔄 Continuous Integration

  - Add spell checker ([#158](https://github.com/microsoft/oxidizer/pull/158))

- 🧩 Miscellaneous

  - bytes is optional dep
  - Native implementation of From<Vec<u8>>
  - Independent implementation of static_slice.into() BytesView
  - toml uipdate
  - Readme update
  - Format
  - Improve doctests
  - Add ::mem and ::mem::testing modules to bytesbuf for better API documentation structure
  - Better summary sentences in API docs
  - Gate test utilities in bytesbuf behind test-util feature flag

## [0.1.2] - 2025-12-10

- ✔️ Tasks

  - publish bytesbuf 0.1.2 ([#117](https://github.com/microsoft/oxidizer/pull/117))
  - publish bytesbuf 0.1.1 ([#116](https://github.com/microsoft/oxidizer/pull/116))
  - Enable the missing_docs compiler lint. ([#102](https://github.com/microsoft/oxidizer/pull/102))
  - Enable unwrap_used and panic clippy lints ([#67](https://github.com/microsoft/oxidizer/pull/67))

- 🔄 Continuous Integration

  - Always pass some tests when there isn't enough memory available ([#109](https://github.com/microsoft/oxidizer/pull/109))
  - Linting for Cargo.toml files ([#110](https://github.com/microsoft/oxidizer/pull/110))
  - Add license check for dependencies ([#105](https://github.com/microsoft/oxidizer/pull/105))

- 🧩 Miscellaneous

  - Enable the allow_attribute lint and fix warnings. ([#111](https://github.com/microsoft/oxidizer/pull/111))
  - Increase consistency of a few little things here and there ([#65](https://github.com/microsoft/oxidizer/pull/65))
  - byte_sequences -> bytesbuf ([#58](https://github.com/microsoft/oxidizer/pull/58))

