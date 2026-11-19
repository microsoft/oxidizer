# Changelog

## [0.6.1] - 2026-11-06

- Match the version of the `arty` package group. The macro source and
  expansion do not change.
- Require local package `arty_macros_impl` version `0.6.1` instead of `0.6.0`.

## [0.6.0] - 2026-11-02

- Breaking: Macro entry points use `arty::task::Context` instead of `Builtins`.
- Add the `panic = shutdown|ignore`, `monitor = enabled|disabled`, and
  `workers = all` configuration.
- Start one worker by default and use exact worker counts. The `#[arty::test]`
  macro disables monitoring unless the test uses `monitor = enabled`.
- Require local package `arty_macros_impl` version `=0.6.0` instead of `=0.5.0`.

## [0.5.0] - 2026-10-20

- Match the procedural macro package with the Arty 0.5 release.
- Require local package `arty_macros_impl` version `0.5.0` instead of `0.4.0`.
