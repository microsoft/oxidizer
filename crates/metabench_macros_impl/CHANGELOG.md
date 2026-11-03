# Changelog

## [0.3.0] - 2026-11-02

- Breaking: Generate native registration for Gungraun 0.20 and its changed callback requirements.
- Protect native perf callbacks with a shared lock. Outside Linux, reuse the adapters that inspect
  benchmark cases.
- Use version 0.3.0 to match the Metabench packages.

## [0.2.1] - 2026-10-20

- Classify the macro implementation as private API and align it with Metabench 0.2.1.

## [0.2.0] - 2026-10-13

- ✨ Features

  - add Intel VTune support alongside Linux perf ([#750](https://github.com/microsoft/oxidizer/pull/750))

- 🐛 Bug Fixes

  - isolate native probes and harden benchmark reporting ([#807](https://github.com/microsoft/oxidizer/pull/807))

- 🔄 Continuous Integration

  - complete cargo-anvil adoption ([#759](https://github.com/microsoft/oxidizer/pull/759))

## [0.1.1] - 2026-09-10

### Maintenance

- Synchronize the generated-code compatibility boundary with `metabench 0.1.1`.
