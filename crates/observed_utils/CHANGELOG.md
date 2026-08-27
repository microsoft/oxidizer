# Changelog

## [0.2.0] - 2026-08-27

### Added

- Initial release of `observed_utils`, the optional companion to `observed`
  for consumers that export to OpenTelemetry.
- `any_value_of`, `otel_value_of` and `otel_severity_of` convert an `observed`
  `Value` or `Severity` into its OpenTelemetry counterpart. `observed` itself no
  longer depends on `opentelemetry`, so an exporter needs this conversion and
  should call it rather than copy it.
- `metric_number_of` extracts the `f64` a metric instrument records from a
  `Value`, or `None` when the value is not numeric.
- `format_any_value` renders an OpenTelemetry `AnyValue` in human-readable form
  instead of its noisy `Debug` shape.
- `SensitiveSlice`, a type-erased, heap-allocation-free collection of
  `RedactedDisplay` references that renders at most `N` items.

- ⚠️ Breaking

  - Now requires `0.25.0` of `observed`

- ✨ Features

  - introduce the observed_utils crate ([#677](https://github.com/microsoft/oxidizer/pull/677))

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
