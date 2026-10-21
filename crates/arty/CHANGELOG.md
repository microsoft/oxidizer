# Changelog

## [0.5.0] - 2026-10-20

- Adopt the breaking `tick` 0.8 timer contract and harden runtime task, shutdown, panic, and worker lifecycle behavior.
- Update local dependency requirements: arty_executor =0.1.3 -> 0.5.0; arty_io_core ^0.2.0 -> 0.3.0; arty_macros =0.4.0 -> 0.5.0; data_privacy ^0.13.0 -> 0.13.1; observed ^0.28.0 -> 0.29.0; performables ^0.2.0 -> 0.3.0; thread_aware ^0.12.0 -> 0.12.1; thread_aware_core ^0.1.1 -> 0.1.2; tick ^0.7.1 -> 0.8.0.

## [0.3.1] - 2026-10-05

- 🔧 Maintenance

  - Now requires `0.7.1` of `tick`

- 🔄 Continuous Integration

  - complete cargo-anvil adoption ([#759](https://github.com/microsoft/oxidizer/pull/759))

## [0.3.0] - 2026-09-10

### Breaking

- Now requires `0.1.1` of `thread_aware_core`
- Now requires `0.7.0` of `tick`

### Features

- introduce runtime facade and core crates ([#727](https://github.com/microsoft/oxidizer/pull/727))
