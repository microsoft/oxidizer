# Changelog

## [0.2.1] - 2026-11-06

- Update published local dependency requirements. No direct source change.
- Update the local `arty` requirement from `0.6.0` to `0.6.1`.
- Update the local `performables` requirement from `0.3.1` to `0.4.0`.

## [0.2.0] - 2026-11-02

- Breaking: Pass `arty::task::Context` from Arty 0.6.0 to `init`.
- Initialize Tokio context on every fixed Arty worker with an application-owned multi-thread Tokio
  runtime.
- Reject current-thread Tokio runtimes. Clean up failed or canceled initialization attempts before a
  later retry.
- Update the requirement for local package `arty` from `0.5.0` to `0.6.0`.
- Update the requirement for local package `ohno` from `0.5.3` to `0.5.4`.
- Update the requirement for local package `performables` from `0.3.0` to `0.3.1`.
