# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial release of `routerama_macros`, providing the `router!` procedural
  macro used through the `routerama` crate's `macros` feature. It parses an
  in-source route table and expands it to the same `Route` enum and
  `resolve(method, path)` function the build-time generator produces.
