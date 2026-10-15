# Changelog

## [0.2.0] - 2026-10-05

- ⚠️ Breaking

  - Now requires `0.2.0` of `allocation_hints`
  - Now requires `0.2.0` of `seismograph`
  - Now requires `0.2.0` of `seismograph_rallocator`

- ✨ Features

  - introduce runtime recording ([#733](https://github.com/microsoft/oxidizer/pull/733))

- 🐛 Bug Fixes

  - use paths for workspace dev-dependencies ([#758](https://github.com/microsoft/oxidizer/pull/758))
  - keep stack-grouped allocations on a single call site ([#748](https://github.com/microsoft/oxidizer/pull/748))

- ⚡ Performance

  - consolidate v1 and Seismograph improvements ([#764](https://github.com/microsoft/oxidizer/pull/764))
  - reduce runtime analysis duration ([#767](https://github.com/microsoft/oxidizer/pull/767))

- 🔄 Continuous Integration

  - complete cargo-anvil adoption ([#759](https://github.com/microsoft/oxidizer/pull/759))
  - update to cargo-anvil 0.7.0 ([#728](https://github.com/microsoft/oxidizer/pull/728))

