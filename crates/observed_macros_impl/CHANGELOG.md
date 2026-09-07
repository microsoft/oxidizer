# Changelog

## [0.25.0] - 2026-09-07

### Added

- Initial release of `observed_macros_impl`, holding the implementation of the
  `observed` procedural macros (`#[event(...)]` and `#[derive(Enrichment)]`).
  `observed_macros` is now a thin `proc-macro` shim that delegates here. Use the
  re-exports from `observed` rather than depending on this crate directly.

### Breaking

- `#[event(...)]` now rejects a field holding a mutable reference (`&mut T` or
  `Option<&mut T>`) while parsing. This also rejects previously valid inputs,
  including an `#[unredacted]` field of type `&mut u64`. Use a shared reference
  or an owned value instead.

- 🐛 Bug Fixes

  - reject mutable-reference event fields ([#730](https://github.com/microsoft/oxidizer/pull/730))

- ⚡ Performance

  - parallelize scheduled Miri and reduce resource outliers ([#706](https://github.com/microsoft/oxidizer/pull/706))

- 🔄 Continuous Integration

  - update to cargo-anvil 0.7.0 ([#728](https://github.com/microsoft/oxidizer/pull/728))

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
