# Changelog

## [0.3.1] - 2026-11-02

- Update the path-template requirement. Use version 0.3.1 to match the Routerama packages. Code
  generation behavior does not change.
- Update the requirement for local package `http_path_template` from `0.2.2` to `0.2.3`.

## [0.3.0] - 2026-10-20

- Refresh published build-helper documentation and runtime-analysis coverage.
- Update local dependency requirements: http_path_template ^0.2.1 -> 0.2.2.

## [0.1.1] - 2026-09-10

### Maintenance

- Now requires `0.2.1` of `http_path_template`

### Bug Fixes

- declare optional dependencies as dev-dependencies ([#658](https://github.com/microsoft/oxidizer/pull/658))

### Tasks

- raise MSRV to Rust 1.95 ([#716](https://github.com/microsoft/oxidizer/pull/716))
- bump syn to 3.0.2 and align proc-macro dependencies ([#621](https://github.com/microsoft/oxidizer/pull/621))
