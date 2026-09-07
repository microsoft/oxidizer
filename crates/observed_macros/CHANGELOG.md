# Changelog

## [0.25.0] - 2026-09-07

### Breaking

- `#[event(...)]` now rejects mutable-reference fields, including previously
  valid unredacted fields. Use shared references or owned values instead.

### Added

- Initial release of `observed_macros`, the procedural macros backing the
  `observed` crate (`#[event(...)]`, `#[derive(Enrichment)]`, and related
  attributes). Use the re-exports from `observed` rather than depending on this
  crate directly.

- 🔧 Maintenance

  - Now requires `0.25.0` of `observed_macros_impl`

- ♻️ Code Refactoring

  - split the implementation into observed_macros_impl ([#686](https://github.com/microsoft/oxidizer/pull/686))

- 🔄 Continuous Integration

  - update to cargo-anvil 0.7.0 ([#728](https://github.com/microsoft/oxidizer/pull/728))

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
