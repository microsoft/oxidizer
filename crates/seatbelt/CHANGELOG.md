# Changelog

## [0.3.1] - 2026-02-27

- ✨ Features

  - ResilienceContext is now ThreadAware

## [0.3.0] - 2026-02-17

- ✨ Features

  - switch to a new major version of `tick` crate
  - add tower-service compatibility to seatbelt ([#252](https://github.com/microsoft/oxidizer/pull/252))

## [0.2.0] - 2026-01-20

Initial release.

- ✨ Features

  - Timeout middleware for canceling long-running operations
  - Retry middleware with constant, linear, and exponential backoff strategies
  - Circuit breaker middleware with health-based failure detection and gradual recovery
  - OpenTelemetry metrics integration (`metrics` feature)
  - Structured logging via tracing (`logs` feature)
  - Shared `Context` for clock and telemetry configuration
