# Changelog

## [0.6.1] - 2026-11-06

- Simplify driver-hosting documentation and the package description.
- Update the local `observed` requirement from `0.30.0` to `0.30.1`.

## [0.6.0] - 2026-11-02

- Add `DriverHost::has_primary` so a runtime can select how each worker waits for I/O.
- Use version 0.6.0 to match the Arty packages and their updated I/O context requirements.
- Update the requirement for local package `arty_io_core` from `0.3.0` to `0.4.0`.
- Update the requirement for local package `data_privacy` from `0.13.1` to `0.13.2`.
- Update the requirement for local package `observed` from `0.29.0` to `0.30.0`.
- Update the requirement for local package `thread_aware_core` from `0.1.2` to `0.1.3`.

## [0.2.0] - 2026-10-20

- The public API exposes types from `arty_io_core`; upgrading to its incompatible release changes those public type identities.
- Update local dependency requirements: arty_io_core ^0.2.0 -> 0.3.0; data_privacy ^0.13.0 -> 0.13.1; observed ^0.28.0 -> 0.29.0; thread_aware_core ^0.1.1 -> 0.1.2.
