# Changelog

## [0.4.0] - 2026-10-29

- ⚠️ Breaking

  - Now requires `0.4.0` of `seismograph`

- ⚡ Performance

  - key the live task registry by task ID ([#879](https://github.com/microsoft/oxidizer/pull/879))

## [0.3.0] - 2026-10-20

- The public API exposes types from `seismograph`; upgrading to its incompatible release changes those public type identities.
- Update local dependency requirements: seismograph ^0.2.0 -> 0.3.0.

## [0.2.0] - 2026-10-05

- ⚠️ Breaking

  - Now requires `0.2.0` of `seismograph`

- ⚡ Performance

  - consolidate v1 and Seismograph improvements ([#764](https://github.com/microsoft/oxidizer/pull/764))
  - reduce runtime analysis duration ([#767](https://github.com/microsoft/oxidizer/pull/767))

- 🔄 Continuous Integration

  - complete cargo-anvil adoption ([#759](https://github.com/microsoft/oxidizer/pull/759))
