# Changelog

## [0.6.0] - 2026-11-02

- Breaking: Use `arty::task::Context` instead of `Builtins` in macro entry points.
- Add `panic = shutdown|ignore`, `monitor = enabled|disabled`, and `workers = all` configuration.
- Start one worker by default and honor exact worker counts. Tests that use the runtime disable
  monitoring unless explicitly enabled.
- Update the requirement for local package `arty_macros_impl` from `=0.5.0` to `=0.6.0`.

## [0.5.0] - 2026-10-20

- Align the procedural macro package with the Arty 0.5 release.
- Update local dependency requirements: arty_macros_impl =0.4.0 -> 0.5.0.
