# Changelog

## [0.5.2] - 2026-06-02

- 🔧 Maintenance

  - Now requires `0.1.4` of `recoverable`
  - Now requires `0.7.2` of `thread_aware`
  - Now requires `0.7.2` of `thread_aware_macros`
  - Now requires `0.7.1` of `thread_aware_macros_impl`
  - Now requires `0.3.2` of `tick`

- ✨ Features

  - release all packages for MSRV increment ([#463](https://github.com/microsoft/oxidizer/pull/463))
  - introduce a new "routing" module ([#389](https://github.com/microsoft/oxidizer/pull/389))

- 🐛 Bug Fixes

  - ensure that `cargo test` passes on a clean checkout ([#441](https://github.com/microsoft/oxidizer/pull/441))

- ✔️ Tasks

  - Tidy cargo dependencies to unbreak publishing ([#466](https://github.com/microsoft/oxidizer/pull/466))
  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))

- 🧩 Miscellaneous

  - Update tool versions ([#462](https://github.com/microsoft/oxidizer/pull/462))

## [0.5.1] - 2026-06-01

- 🔧 Maintenance

  - Now requires `0.3.1` of `layered`
  - Now requires `0.1.3` of `recoverable`
  - Now requires `0.7.1` of `thread_aware`
  - Now requires `0.7.1` of `thread_aware_macros`
  - Now requires `0.3.1` of `tick`

- ✨ Features

  - introduce a new "routing" module ([#389](https://github.com/microsoft/oxidizer/pull/389))

- 🐛 Bug Fixes

  - ensure that `cargo test` passes on a clean checkout ([#441](https://github.com/microsoft/oxidizer/pull/441))

- ✔️ Tasks

  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))

## [0.5.0] - 2026-05-14

- ⚠️ Breaking

  - update the `metrics` feature API to use OpenTelemetry 0.32 types ([#417](https://github.com/microsoft/oxidizer/pull/417))

- ✨ Features

  - Improve thread_aware APIs and anyspawn rt compat. ([#403](https://github.com/microsoft/oxidizer/pull/403))
  - support conversion from std::io::error::ErrorKind ([#370](https://github.com/microsoft/oxidizer/pull/370))

- ✔️ Tasks

  - upgrade opentelemetry crates to 0.32.0 ([#417](https://github.com/microsoft/oxidizer/pull/417))
  - run delta subset of examples on PR build ([#394](https://github.com/microsoft/oxidizer/pull/394))
  - release a new version of tick crate ([#387](https://github.com/microsoft/oxidizer/pull/387))

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
