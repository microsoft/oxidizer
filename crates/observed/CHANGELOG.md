# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial release of `observed`, a structured telemetry framework with typed
  events, enrichment, redaction, and per-field routing to OpenTelemetry.
- `#[derive(Event)]` and the `emit!` macro for defining and emitting typed
  telemetry events.
- Scoped, stackable enrichment via `#[derive(Enrichment)]` and RAII guards, with
  cross-thread context propagation.
- Data-classification-aware redaction integrated with the `data_privacy` crate.
