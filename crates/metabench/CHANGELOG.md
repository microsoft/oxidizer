# Changelog

## [0.3.0] - 2026-11-02

- Breaking: Replace temporary borrows with owned inputs in native benchmark cases. Gungraun and its
  runner now use version 0.20.0.
- Read Gungraun schema-v7 summaries and accept boolean `--test` arguments to select smoke mode,
  which runs benchmarks without measurements.
- Generated Gungraun native perf callbacks run one at a time in each process. Nested callbacks panic.
- After a callback panics, later callbacks cannot reuse the shared perf controls.
- Outside Linux, register native benchmarks with the adapters used to inspect benchmark cases.
  Gungraun still runs only on Linux.
- Update the requirement for `gungraun` from `=0.19.4` to `=0.20.0`.
- Update the requirement for `gungraun-runner` from `=0.19.4` to `=0.20.0`.
- Update the requirement for local package `metabench_macros` from `=0.2.1` to `=0.3.0`.
- Update the requirement for local package `ohno` from `0.5.3` to `0.5.4`.

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
