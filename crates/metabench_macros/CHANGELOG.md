# Changelog

## [0.3.0] - 2026-11-02

- Breaking: Replace temporary borrows with owned native inputs. The macros generate adapters for
  Gungraun 0.20.
- Update the exact implementation requirement to protect native perf callbacks with a lock and reuse
  adapters outside Linux.
- Update the requirement for local package `metabench_macros_impl` from `=0.2.1` to `=0.3.0`.

## [0.2.1] - 2026-10-20

- Classify the procedural macro as private API and align it with Metabench 0.2.1.
- Update local dependency requirements: metabench_macros_impl =0.2.0 -> 0.2.1.

## [0.2.0] - 2026-10-13

- 🔧 Maintenance

  - Now requires `0.2.0` of `metabench_macros_impl`

- ✨ Features

  - add Intel VTune support alongside Linux perf ([#750](https://github.com/microsoft/oxidizer/pull/750))

- 🐛 Bug Fixes

  - isolate native probes and harden benchmark reporting ([#807](https://github.com/microsoft/oxidizer/pull/807))

- 🔄 Continuous Integration

  - complete cargo-anvil adoption ([#759](https://github.com/microsoft/oxidizer/pull/759))

## [0.1.1] - 2026-09-10

### Maintenance

- Now requires `0.1.1` of `metabench_macros_impl`.
