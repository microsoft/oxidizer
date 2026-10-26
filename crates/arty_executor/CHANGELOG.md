# Changelog

## [0.6.0] - 2026-10-26

- ⚠️ Breaking

  - propagate shutdown causes ([#880](https://github.com/microsoft/oxidizer/pull/880))
  - target tasks by worker coordinate ([#866](https://github.com/microsoft/oxidizer/pull/866))
  - Now requires `0.29.1` of `observed`
  - Now requires `0.3.1` of `performables`
  - Now requires `0.1.3` of `thread_aware_core`
  - Now requires `0.8.1` of `tick`

- ⚡ Performance

  - reduce scheduler contention ([#855](https://github.com/microsoft/oxidizer/pull/855))

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
