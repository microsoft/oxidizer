# Changelog

## [0.3.0] - 2026-10-20

- Consolidate diagnostics into `performables::Error`, flatten synchronization modes, return opaque acquisition futures, and support unsized `Arc::from_std` values.
- Update local dependency requirements: seismograph ^0.2.0 -> 0.3.0; thread_aware_core ^0.1.1 -> 0.1.2; metabench ^0.2.0 -> 0.2.1.

## [0.2.0] - 2026-10-05

- ⚠️ Breaking

  - Now requires `0.2.0` of `seismograph`

- ⚡ Performance

  - consolidate v1 and Seismograph improvements ([#764](https://github.com/microsoft/oxidizer/pull/764))
  - reduce runtime analysis duration ([#767](https://github.com/microsoft/oxidizer/pull/767))

- 🔄 Continuous Integration

  - complete cargo-anvil adoption ([#759](https://github.com/microsoft/oxidizer/pull/759))

