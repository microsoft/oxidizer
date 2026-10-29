# Changelog

## [0.6.0] - 2026-10-29

- ⚠️ Breaking

  - soft-stabilize public APIs ([#632](https://github.com/microsoft/oxidizer/pull/632))
  - propagate shutdown causes ([#880](https://github.com/microsoft/oxidizer/pull/880))
  - target tasks by worker coordinate ([#866](https://github.com/microsoft/oxidizer/pull/866))
  - Now requires `0.6.0` of `arty_executor`
  - Now requires `0.4.0` of `arty_io_core`
  - Now requires `0.3.0` of `arty_io_host`
  - Now requires `0.6.0` of `arty_macros`
  - Now requires `0.30.0` of `observed`
  - Now requires `0.3.1` of `performables`
  - Now requires `0.4.0` of `seismograph`
  - Now requires `0.12.2` of `thread_aware`
  - Now requires `0.1.3` of `thread_aware_core`
  - Now requires `0.9.0` of `tick`

- ✨ Features

  - add panic policy configuration ([#898](https://github.com/microsoft/oxidizer/pull/898))
  - add monitor configuration plumbing ([#885](https://github.com/microsoft/oxidizer/pull/885))
  - integrate I/O drivers ([#829](https://github.com/microsoft/oxidizer/pull/829))

- 🐛 Bug Fixes

  - default to one worker and relocate spawn results ([#875](https://github.com/microsoft/oxidizer/pull/875))
  - honor exact worker counts ([#873](https://github.com/microsoft/oxidizer/pull/873))

- ⚡ Performance

  - reduce scheduler contention ([#855](https://github.com/microsoft/oxidizer/pull/855))
  - optimize synchronization and consolidate ready paths ([#854](https://github.com/microsoft/oxidizer/pull/854))

- 📚 Documentation

  - add repository agent policies ([#865](https://github.com/microsoft/oxidizer/pull/865))

- ✔️ Tasks

  - upgrade the Gungraun toolchain to 0.20.0 ([#793](https://github.com/microsoft/oxidizer/pull/793))

- ♻️ Code Refactoring

  - unify task completion and shutdown ([#899](https://github.com/microsoft/oxidizer/pull/899))

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
