# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial release of `observed_rt`, a context-propagating task spawner that wraps
  `anyspawn` so spawned tasks automatically inherit `observed` enrichment state.
- `observed_rt::tokio` builds a Tokio-backed spawner that forwards enrichment to
  every spawned async and blocking task.
