# Changelog

## [0.2.0] - 2026-01-20

Initial release.

- ✨ Features

  - Timeout middleware for canceling long-running operations
  - Retry middleware with constant, linear, and exponential backoff strategies
  - Circuit breaker middleware with health-based failure detection and gradual recovery
  - OpenTelemetry metrics integration (`metrics` feature)
  - Structured logging via tracing (`logs` feature)
  - Shared `Context` for clock and telemetry configuration
