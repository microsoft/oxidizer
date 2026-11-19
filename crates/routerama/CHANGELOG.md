# Changelog

## [0.3.1] - 2026-11-02

- Update path-template, macro, and benchmark requirements. Use version 0.3.1 for all Routerama
  packages. Routing behavior does not change.
- Update the requirement for local package `http_path_template` from `0.2.2` to `0.2.3`.
- Update the requirement for local package `routerama_build` from `=0.3.0` to `=0.3.1`.
- Update the requirement for local package `routerama_macros` from `=0.3.0` to `=0.3.1`.
- Update the development requirement for `gungraun` from `=0.19.4` to `=0.20.0`.
- Update the development requirement for `tokio` from `1.48.0` to `1.53.3`.

## [0.3.0] - 2026-10-20

- Refresh published routing documentation and benchmark integration without changing the supported API.
- Update local dependency requirements: http_path_template ^0.2.1 -> 0.2.2; routerama_build =0.1.1 -> 0.3.0; routerama_macros =0.1.1 -> 0.3.0.

## [0.1.1] - 2026-09-10

### Maintenance

- Now requires `0.2.1` of `http_path_template`
- Now requires `0.1.1` of `routerama_build`
- Now requires `0.1.1` of `routerama_macros`

### Bug Fixes

- declare optional dependencies as dev-dependencies ([#658](https://github.com/microsoft/oxidizer/pull/658))
