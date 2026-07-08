# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial release of `routerama`, a compile-time static HTTP router.
- `scan_segments` splits a path into segment offsets in a stack buffer (SSE2 on
  `x86_64`, NEON on `aarch64`, scalar elsewhere) and `split_verb` peels a trailing
  custom `:verb` — the primitives a generated `resolve` calls.
- `router!` macro (feature `macros`) generates a router inline from a route
  table, with no `build.rs`.
- `build` feature re-exports the `routerama_build` code generator
  (`generate_router`, `Route`, `HttpMethod`, `RouterOptions`, `route_field_name`)
  for use from a `build.rs`.
