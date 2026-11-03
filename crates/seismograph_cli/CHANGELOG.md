# Changelog

## [0.4.0] - 2026-11-02

- Breaking: Use Unix sockets or Windows named pipes instead of the former TCP/token protocol.
  Operating-system permissions control access.
- Breaking: Rebuild monitored applications and the CLI for monitor protocol version 8. Offline
  snapshot files remain compatible.
- Find accessible discovery registries, the per-user lists of monitors. Report invalid entries
  without hiding reachable applications.
- Display retained-notification activity in live views and HTML reports.
- Update the requirement for local package `performables` from `0.3.0` to `0.3.1`.
- Update the requirement for local package `seismograph` from `0.3.0` to `0.4.0`.
- Update the requirement for local package `seismograph_protocol` from `0.2.0` to `0.3.0`.
- Update the requirement for local package `seismograph_rallocator` from `0.3.0` to `0.4.0`.
- Update the requirement for local package `seismograph_runtime` from `0.3.0` to `0.4.0`.

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

