# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial release of `observed_macros`, the procedural macros backing the
  `observed` crate (`#[derive(Event)]`, `#[derive(Enrichment)]`, and related
  attributes). Use the re-exports from `observed` rather than depending on this
  crate directly.
