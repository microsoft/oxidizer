# Changelog

## [0.5.0] - 2026-11-06

- Breaking: Replace per-task diagnostic registration and lifetime counters with bounded
  requests to native task providers. Remove `TaskHandle` instrumentation APIs.
- Add `RuntimeHandle::capture` and `CaptureOptions` for collection limits, timeouts, and
  optional poll-measurement windows.
- Breaking: Make task creation times optional, remove source-derived creation stacks,
  and report task collection availability and measurement scope explicitly.
- Breaking: Emit runtime source schema 8. Continue reading schemas 2 through 6 and 8.
- Remove the `backtrace` requirement.
- Breaking: Public APIs expose types from packages with incompatible releases:
  `seismograph`.
- Update the local `seismograph` requirement from `0.4.0` to `0.5.0`.
- Add the local `tick` requirement at `0.9.1`.

## [0.4.0] - 2026-11-02

- Store live tasks by task ID so removing a completed task no longer requires a scan of every live
  task. Snapshots remain sorted by task ID.
- Update the Seismograph dependency to use the 0.4.0 packages.
- Update the requirement for local package `seismograph` from `0.3.0` to `0.4.0`.

## [0.3.0] - 2026-10-20

- The public API exposes types from `seismograph`; upgrading to its incompatible release changes those public type identities.
- Update local dependency requirements: seismograph ^0.2.0 -> 0.3.0.

## [0.2.0] - 2026-10-05

- ⚠️ Breaking

  - Now requires `0.2.0` of `seismograph`

- ⚡ Performance

  - consolidate v1 and Seismograph improvements ([#764](https://github.com/microsoft/oxidizer/pull/764))
  - reduce runtime analysis duration ([#767](https://github.com/microsoft/oxidizer/pull/767))

- 🔄 Continuous Integration

  - complete cargo-anvil adoption ([#759](https://github.com/microsoft/oxidizer/pull/759))
