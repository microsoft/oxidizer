# Changelog

## [0.4.0] - 2026-10-26

- ⚠️ Breaking

  - replace TCP monitoring with native IPC ([#847](https://github.com/microsoft/oxidizer/pull/847))
  - Now requires `0.3.0` of `seismograph_protocol`

- ⚡ Performance

  - optimize synchronization and consolidate ready paths ([#854](https://github.com/microsoft/oxidizer/pull/854))

## [0.3.0] - 2026-10-20

- Consolidate monitor startup errors at the crate root and migrate allocator diagnostics to the native-v4 model.

## [0.2.0] - 2026-10-05

- ⚠️ Breaking

  - Now requires `0.2.0` of `seismograph_protocol`

- ⚡ Performance

  - consolidate v1 and Seismograph improvements ([#764](https://github.com/microsoft/oxidizer/pull/764))
  - reduce runtime analysis duration ([#767](https://github.com/microsoft/oxidizer/pull/767))

- 🔄 Continuous Integration

  - complete cargo-anvil adoption ([#759](https://github.com/microsoft/oxidizer/pull/759))

