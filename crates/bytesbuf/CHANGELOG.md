# Changelog

## [0.4.2] - 2026-04-09

- ✨ Features

  - add bytesbuf_global_pool_instances_total metric ([#352](https://github.com/microsoft/oxidizer/pull/352))

## [0.4.1] - 2026-03-20

- ✔️ Tasks

  - implement `From<BytesView>` for `Bytes`

## [0.4.0] - 2026-02-26

- ✨ Features

  - add BytesBuf::split_off_remaining ([#286](https://github.com/microsoft/oxidizer/pull/286))
  - reduce unnecessary reference count jiggling ([#283](https://github.com/microsoft/oxidizer/pull/283))
  - add missing Eq implementation for BytesView ([#281](https://github.com/microsoft/oxidizer/pull/281))
  - impl ThreadAware for GlobalPool ([#273](https://github.com/microsoft/oxidizer/pull/273))
  - clamp minimum reservation size in BytesBufWriter ([#277](https://github.com/microsoft/oxidizer/pull/277))
  - improve ergonomics of BytesView::as_read() ([#272](https://github.com/microsoft/oxidizer/pull/272))
  - make BytesBufWriter owning via into_writer() ([#274](https://github.com/microsoft/oxidizer/pull/274))

## [0.3.0] - 2026-02-13

- ✨ Features

  - GlobalPool uses smaller memory blocks for smaller reservation requests ([#254](https://github.com/microsoft/oxidizer/pull/254))
  - Improve how memory block metadata is exposed in API ([#248](https://github.com/microsoft/oxidizer/pull/248))
  - remove implicit `Sized` requirement from `BytesBuf::as_write` method ([#249](https://github.com/microsoft/oxidizer/pull/249))

- ✔️ Tasks

  - Improve our crate's repository property. ([#246](https://github.com/microsoft/oxidizer/pull/246))

- 🔄 Continuous Integration

  - automatically publish release notes ([#247](https://github.com/microsoft/oxidizer/pull/247))

## [0.2.2] - 2026-01-15

- 📚 Documentation

  - A few doc-related fixes ([#198](https://github.com/microsoft/oxidizer/pull/198))

## [0.2.1] - 2026-01-07

- ✨ Features

  - Migrate bytesbuf_io from private repo ([#181](https://github.com/microsoft/oxidizer/pull/181))

- 🐛 Bug Fixes

  - Replace removed doc_auto_cfg feature with doc_cfg ([#178](https://github.com/microsoft/oxidizer/pull/178))

- ✔️ Tasks

  - Bump bytesbuf and bytesbuf_io version numbers to re-trigger publishing ([#188](https://github.com/microsoft/oxidizer/pull/188))

## [0.2.0] - 2026-01-02

- ✨ Features

  - Next iteration of bytesbuf tinkering ([#171](https://github.com/microsoft/oxidizer/pull/171))
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

  - Add bytesbuf vs bytes benchmark suite for read/write operations ([#162](https://github.com/microsoft/oxidizer/pull/162))

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
