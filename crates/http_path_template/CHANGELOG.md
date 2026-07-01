# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial release of `http_path_template`, a dependency-free parser for the
  `google.api.http` path-template grammar.
- `PathTemplate::parse` validates a template string and exposes its structure
  via `PathTemplate::segments` and `PathTemplate::verb`.
- `Segment` and `Variable` model the parsed abstract syntax tree (literals, `*`,
  `**`, and `{field.path=sub-template}` variable bindings).
- `ParseError` / `ParseErrorKind` enumerate every structural parse failure.
