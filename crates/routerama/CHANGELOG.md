# Changelog

## [0.1.1] - 2026-09-07

### Maintenance

- Now requires `0.2.1` of `http_path_template`
- Now requires `0.1.1` of `routerama_build`
- Now requires `0.1.1` of `routerama_macros`

### Bug Fixes

- declare optional dependencies as dev-dependencies ([#658](https://github.com/microsoft/oxidizer/pull/658))
- migrate to alloc_tracker 0.7 ([#568](https://github.com/microsoft/oxidizer/pull/568))

### Performance

- cut the PR Miri step from 3h16m to about 1h15m ([#674](https://github.com/microsoft/oxidizer/pull/674))
