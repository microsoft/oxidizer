# Changelog

## [0.5.0] - 2026-11-06

- Breaking: Emit allocator source schema 4 and accept schemas 3 and 4 when decoding.
  Native observations no longer depend on recorder sessions or event sampling.
- Use native observation freshness when publishing allocator samples.
- Breaking: Public APIs expose types from packages with incompatible releases:
  `seismograph`.
- Update the local `seismograph` requirement from `0.4.0` to `0.5.0`.

## [0.4.0] - 2026-11-02

- Update the Seismograph dependency to use the 0.4.0 packages. Allocator measurement behavior does
  not change.
- Update the requirement for local package `seismograph` from `0.3.0` to `0.4.0`.

## [0.3.0] - 2026-10-20

- Replace legacy allocator diagnostics with schema 3 native-v4 snapshots and topology.
- Update local dependency requirements: seismograph ^0.2.0 -> 0.3.0.

## [0.2.0] - 2026-10-05

- ⚠️ Breaking

  - Now requires `0.2.0` of `seismograph`

- ⚡ Performance

  - consolidate v1 and Seismograph improvements ([#764](https://github.com/microsoft/oxidizer/pull/764))

- 🔄 Continuous Integration

  - complete cargo-anvil adoption ([#759](https://github.com/microsoft/oxidizer/pull/759))

