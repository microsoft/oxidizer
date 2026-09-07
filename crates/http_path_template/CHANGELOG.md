# Changelog

## [0.2.1] - 2026-09-07

### Added

- Initial release of `http_path_template`, a dependency-free parser for the
  `google.api.http` path-template grammar.
- `PathTemplate::parse` validates a template string and exposes its structure
  via `PathTemplate::segments` and `PathTemplate::verb`.
- `Segment` and `Variable` model the parsed abstract syntax tree (literals, `*`,
  `**`, and `{field.path=sub-template}` variable bindings).
- `ParseError` reports every structural parse failure, with `is_*` predicates to
  categorize it.

- ✨ Features

  - add REST transcoding for gRPC services ([#600](https://github.com/microsoft/oxidizer/pull/600))

- 🐛 Bug Fixes

  - declare optional dependencies as dev-dependencies ([#658](https://github.com/microsoft/oxidizer/pull/658))

- 🔄 Continuous Integration

  - update to cargo-anvil 0.7.0 ([#728](https://github.com/microsoft/oxidizer/pull/728))
  - update to cargo-anvil 0.3.0 ([#596](https://github.com/microsoft/oxidizer/pull/596))

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
