# Changelog

## [0.4.0] - 2026-10-26

- 🔧 Maintenance

  - Now requires `0.3.1` of `performables`
  - Now requires `0.4.0` of `seismograph`
  - Now requires `0.3.0` of `seismograph_protocol`
  - Now requires `0.4.0` of `seismograph_rallocator`
  - Now requires `0.4.0` of `seismograph_runtime`

- ⚠️ Breaking

  - replace TCP monitoring with native IPC ([#847](https://github.com/microsoft/oxidizer/pull/847))

- ⚡ Performance

  - optimize synchronization and consolidate ready paths ([#854](https://github.com/microsoft/oxidizer/pull/854))

## [0.3.0] - 2026-10-20

- Replace the Heap view with the Native v4 allocator view and consume schema 3 allocator-state captures.
- Update local dependency requirements: performables ^0.2.0 -> 0.3.0; seismograph ^0.2.0 -> 0.3.0; seismograph_rallocator ^0.2.0 -> 0.3.0; seismograph_runtime ^0.2.0 -> 0.3.0.

## [0.2.0] - 2026-10-05

- 🔧 Maintenance

  - Now requires `0.2.0` of `performables`
  - Now requires `0.2.0` of `seismograph`
  - Now requires `0.2.0` of `seismograph_protocol`
  - Now requires `0.2.0` of `seismograph_rallocator`
  - Now requires `0.2.0` of `seismograph_runtime`

- ⚡ Performance

  - consolidate v1 and Seismograph improvements ([#764](https://github.com/microsoft/oxidizer/pull/764))
  - reduce runtime analysis duration ([#767](https://github.com/microsoft/oxidizer/pull/767))

- 🔄 Continuous Integration

  - complete cargo-anvil adoption ([#759](https://github.com/microsoft/oxidizer/pull/759))

