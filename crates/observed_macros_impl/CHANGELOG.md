# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial release of `observed_macros_impl`, holding the implementation of the
  `observed` procedural macros (`#[event(...)]` and `#[derive(Enrichment)]`).
  `observed_macros` is now a thin `proc-macro` shim that delegates here. Use the
  re-exports from `observed` rather than depending on this crate directly.

### Fixed

- Both generators now resolve the `observed` runtime crate through
  `proc-macro-crate` instead of emitting a hard-coded `::observed`. A crate that
  renames its dependency (`telemetry = { package = "observed", ... }`) can now
  use `#[event(...)]` and `#[derive(Enrichment)]`.
