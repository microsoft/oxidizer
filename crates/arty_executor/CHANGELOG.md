# Changelog

## [0.6.1] - 2026-11-06

- Add the optional `inspection` feature. A runtime can request bounded task descriptions
  from native storage and enable poll measurements during an observation window.
- Add execution-state queries for the Arty scheduler. Keep inspection disabled in Arty.
  This package remains a private implementation API.
- Update the local `observed` requirement from `0.30.0` to `0.30.1`.
- Update the local `performables` requirement from `0.3.1` to `0.4.0`.
- Update the local `tick` requirement from `0.9.0` to `0.9.1`.

## [0.6.0] - 2026-11-02

- Breaking: Supply a `TaskCompletion` target when registering a task. This replaces the executor's
  join handles and remote result channels.
- Breaking: Use a shared `Shutdown` handle and `ShutdownCause` instead of `begin_shutdown`.
  `execute_cycle` now requires mutable access.
- Add thread configuration and `PanicPolicy` to select whether a task panic stops the executor.
- Reduce competition for locks when waking and completing tasks. Wakers, the callbacks that request
  task execution, remain harmless after their tasks complete.
- Add a requirement for local package `ohno` at `0.5.4`.
- Add a requirement for local package `performables` at `0.3.1`.
- Add a requirement for local package `thread_aware_core` at `0.1.3`.
- Update the requirement for local package `data_privacy` from `0.13.1` to `0.13.2`.
- Update the requirement for local package `observed` from `0.29.0` to `0.30.0`.
- Update the requirement for local package `plurality` from `0.2.3` to `0.2.4`.
- Update the requirement for local package `tick` from `0.8.0` to `0.9.0`.
- Update the development requirement for `gungraun` from `=0.19.4` to `=0.20.0`.

## [0.5.0] - 2026-10-20

- Harden executor lifecycle, make construction and retained wakers safe, and update exposed `tick` identities through the Arty version group.
- Update local dependency requirements: data_privacy ^0.13.0 -> 0.13.1; observed ^0.28.0 -> 0.29.0; plurality ^0.2.2 -> 0.2.3; tick ^0.7.1 -> 0.8.0.

## [0.1.3] - 2026-10-05

- 🔧 Maintenance

  - Now requires `0.7.1` of `tick`

- ⚡ Performance

  - reduce task and wake processing overhead ([#779](https://github.com/microsoft/oxidizer/pull/779))

- 🔄 Continuous Integration

  - complete cargo-anvil adoption ([#759](https://github.com/microsoft/oxidizer/pull/759))

## [0.1.2] - 2026-09-10

### Maintenance

- Now requires `0.7.0` of `tick`

## [0.1.1] - 2026-08-27

- 🔧 Maintenance

  - Now requires `0.6.0` of `tick`

- ✨ Features

  - migrate executor from ox-sdk ([#647](https://github.com/microsoft/oxidizer/pull/647))

- 🐛 Bug Fixes

  - declare the foldhash std feature it uses ([#699](https://github.com/microsoft/oxidizer/pull/699))

- ✔️ Tasks

  - bump all_the_time to 0.6.2 ([#690](https://github.com/microsoft/oxidizer/pull/690))
