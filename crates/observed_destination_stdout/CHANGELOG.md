# Changelog

## [0.2.0] - 2026-11-24

- Breaking: Return metric-export write failures to the OpenTelemetry SDK instead of panicking.
- Use the local cumulative metric exporter and retain explicit shutdown handling.
- Update the local requirement for `observed_destination_core` from `^0.29.2` to `^0.29.3`.

## [0.1.1] - 2026-11-06

- Update published local dependency requirements. No direct source change.
- Update the local `observed_destination_core` requirement from `0.29.1` to `0.29.2`.
