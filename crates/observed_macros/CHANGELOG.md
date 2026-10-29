# Changelog

## [0.30.0] - 2026-10-29

- 🔧 Maintenance

  - Now requires `0.30.0` of `observed_macros_impl`

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.29.0] - 2026-10-20

- Align the procedural macro package with the Observed 0.29 release.
- Update local dependency requirements: observed_macros_impl =0.28.0 -> 0.29.0.
- Version alignment: group observed [observed, observed_macros, observed_macros_impl] aligns to 0.29.0.

## [0.26.0] - 2026-09-10

### Breaking

- `#[event(...)]` now rejects mutable-reference fields, including previously
  valid unredacted fields. Use shared references or owned values instead.

### Changed

- Delegate macro expansion to `observed_macros_impl`, leaving this crate as a
  thin procedural-macro entry point ([#686](https://github.com/microsoft/oxidizer/pull/686)).
- Align the macro family with `observed 0.26.0`.
- Now requires `0.26.0` of `observed_macros_impl`.
