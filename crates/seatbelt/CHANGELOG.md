# Changelog

## [0.4.5] - 2026-04-22

- 🔧 Maintenance

  - bump `tick` to 0.2.2

## [0.4.4] - 2026-04-01

- ✨ Features

  - introduce chaos latency ([#345](https://github.com/microsoft/oxidizer/pull/345))

## [0.4.3] - 2026-03-24

- ✨ Features

  - introduce chaos injection middleware ([#335](https://github.com/microsoft/oxidizer/pull/335))

## [0.4.2] - 2026-03-10

- ✨ Features

  - add `RetryConfig::handle_unavailable` and `HedgingConfig::handle_unavailable`

## [0.4.1] - 2026-03-10

- ✨ Features

  - expose `seatbelt::Attempt` and obsolete `seatbelt::retry::Attempt` and `seatbelt::hedging::Attempt`

## [0.4.0] - 2026-03-06

- ⚠️ Breaking

  - no more default features ([#303](https://github.com/microsoft/oxidizer/pull/303))

- ✨ Features

  - introduce hedging resilience middleware ([#298](https://github.com/microsoft/oxidizer/pull/298))
  - introduce fallback middleware ([#294](https://github.com/microsoft/oxidizer/pull/294))
  - introduce config for each middleware ([#302](https://github.com/microsoft/oxidizer/pull/302))
  - improve telemetry ([#297](https://github.com/microsoft/oxidizer/pull/297))
  - improve documentation

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
