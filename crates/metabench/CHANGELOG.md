# Changelog

## [0.2.1] - 2026-10-20

- Published local dependency requirement(s) will move: metabench_macros, ohno.
- Update local dependency requirements: metabench_macros =0.2.0 -> 0.2.1; ohno ^0.5.2 -> 0.5.3.

## [0.2.0] - 2026-10-13

- 🔧 Maintenance

  - Now requires `0.2.0` of `metabench_macros`

- ✨ Features

  - add Intel VTune support alongside Linux perf ([#750](https://github.com/microsoft/oxidizer/pull/750))

- 🐛 Bug Fixes

  - isolate native probes and harden benchmark reporting ([#807](https://github.com/microsoft/oxidizer/pull/807))
  - exclude inactive platform mutants ([#777](https://github.com/microsoft/oxidizer/pull/777))
  - give the observed and metabench `basic` examples unique names ([#773](https://github.com/microsoft/oxidizer/pull/773))

- 🔄 Continuous Integration

  - complete cargo-anvil adoption ([#759](https://github.com/microsoft/oxidizer/pull/759))

## [0.1.1] - 2026-09-10

### Maintenance

- Now requires `0.1.1` of `metabench_macros`.

## [0.1.0]

- Initial integration into the Oxidizer workspace.
