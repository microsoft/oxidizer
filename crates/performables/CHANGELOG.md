# Changelog

## [0.3.1] - 2026-11-02

- Add `Notify` for blocking, timed, and async waits. It retains at most one notification permit, a
  stored right to complete a wait.
- Use `NotifyWait::enable` to register before examining work protected by separate synchronization.
  Canceling a notified wait transfers or retains its permit.
- Reduce the work needed for locks, channels, and operations that are ready to complete. Poisoning,
  timeout limits, and notification behavior do not change.
- Avoid cloning an `Arc` handle again when relocating it to a previously used destination.
- Emit Seismograph events for notification waits, waits without an available permit, and signals.
- Update the requirement for local package `seismograph` from `0.3.0` to `0.4.0`.
- Update the requirement for local package `thread_aware_core` from `0.1.2` to `0.1.3`.
- Update the development requirement for local package `metabench` from `0.2.1` to `0.3.0`.

## [0.3.0] - 2026-10-20

- Consolidate diagnostics into `performables::Error`, flatten synchronization modes, return opaque acquisition futures, and support unsized `Arc::from_std` values.
- Update local dependency requirements: seismograph ^0.2.0 -> 0.3.0; thread_aware_core ^0.1.1 -> 0.1.2; metabench ^0.2.0 -> 0.2.1.

## [0.2.0] - 2026-10-05

- ⚠️ Breaking

  - Now requires `0.2.0` of `seismograph`

- ⚡ Performance

  - consolidate v1 and Seismograph improvements ([#764](https://github.com/microsoft/oxidizer/pull/764))
  - reduce runtime analysis duration ([#767](https://github.com/microsoft/oxidizer/pull/767))

- 🔄 Continuous Integration

  - complete cargo-anvil adoption ([#759](https://github.com/microsoft/oxidizer/pull/759))

