# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.26.0] - 2026-09-10

### Changed

- Align the macro implementation package with `observed 0.26.0`. This crate
  continues to implement `#[event(...)]` and `#[derive(Enrichment)]` behind the
  `observed_macros` entry points.

### Breaking

- `#[event(...)]` now rejects a field holding a mutable reference (`&mut T` or
  `Option<&mut T>`) while parsing. This also rejects previously valid inputs,
  including an `#[unredacted]` field of type `&mut u64`. Use a shared reference
  or an owned value instead ([#730](https://github.com/microsoft/oxidizer/pull/730)).
