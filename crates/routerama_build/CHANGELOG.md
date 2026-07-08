# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial release of `routerama_build`, the build-time code generator for the
  `routerama` static HTTP router.
- `generate_router` lowers a set of `Route`s (HTTP method + path template + name)
  into a `Route` enum and a `resolve(method, path)` function — a compile-time
  trie with no per-request allocation — and reports ambiguous routes as a
  `compile_error!`.
- `HttpMethod` and `Route` describe the route set fed to the generator.
