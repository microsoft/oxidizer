# Changelog

## [0.1.1] - 2026-09-07

### Added

- Implement `ThreadAware` for `&'static str` ([#721](https://github.com/microsoft/oxidizer/pull/721)).

### Fixed

- Prevent runtime owner identities from wrapping and being reused. Clarify
  relocation and coordinate lifetime guarantees. These changes are compatible
  with `0.1.0`; the breaking workspace adoption affects `thread_aware` and its
  consumers, not this core crate ([#721](https://github.com/microsoft/oxidizer/pull/721)).
