# Changelog

## [0.6.0] - 2026-11-02

- Breaking: Generate entry points that use `arty::task::Context` and the Arty 0.6.0 runtime
  configuration.
- Accept panic and monitor policies, exact worker counts, and `workers = all`. Tests that use the
  runtime disable monitoring by default.
- Use version 0.6.0 to match the other Arty packages.

## [0.5.0] - 2026-10-20

- Align the macro implementation with Arty 0.5 and retain renamed-dependency resolution support.
