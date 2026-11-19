# Changelog

## [0.5.0] - 2026-11-06

- Breaking: Allow I/O completion events after recording restarts, even when their start
  event was not retained. Completion is no longer restricted to the starting session.
- Breaking: Public APIs expose types from packages with incompatible releases:
  `seismograph`.
- Update the local `seismograph` requirement from `0.4.0` to `0.5.0`.

## [0.4.0] - 2026-11-02

- Update the Seismograph dependency to use the 0.4.0 packages. I/O event APIs do not change.
- Update the requirement for local package `seismograph` from `0.3.0` to `0.4.0`.

## [0.3.0] - 2026-10-20

- The public API exposes types from `seismograph`; upgrading to its incompatible release changes those public type identities.
- Update local dependency requirements: seismograph ^0.2.0 -> 0.3.0.

## [0.2.0] - 2026-10-05

- ⚠️ Breaking

  - Now requires `0.2.0` of `seismograph`

- ⚡ Performance

  - consolidate v1 and Seismograph improvements ([#764](https://github.com/microsoft/oxidizer/pull/764))

- 🔄 Continuous Integration

  - complete cargo-anvil adoption ([#759](https://github.com/microsoft/oxidizer/pull/759))
