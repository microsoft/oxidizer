# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.26.0] - 2026-09-10

### Breaking

- `#[event(...)]` now rejects mutable-reference fields, including previously
  valid unredacted fields. Use shared references or owned values instead.

### Changed

- Delegate macro expansion to `observed_macros_impl`, leaving this crate as a
  thin procedural-macro entry point ([#686](https://github.com/microsoft/oxidizer/pull/686)).
- Align the macro family with `observed 0.26.0`.
- Now requires `0.26.0` of `observed_macros_impl`.
