# Changelog

## [0.5.0] - 2026-11-06

- Breaking: Rename `RecordingPolicy` to `Policy` and `CaptureSource` to `CaptureFn`.
  `EventBufferCapacity::new` now panics for invalid values instead of returning `None`.
  Use `TryFrom<usize>` for fallible construction.
- Breaking: Remove recording-session APIs, the public `system` module, wire event aliases,
  and snapshot-arena helpers.
- Check event category, sampling, suppression, and configuration changes in one recorder
  path. Keep snapshot allocations outside allocator recording without a custom arena.
- Simplify recorder and monitor documentation.

## [0.4.0] - 2026-11-02

- Breaking: Use local Unix sockets or Windows named pipes instead of TCP ports and authentication
  tokens. Operating-system permissions control access.
- Breaking: Use monitor protocol version 8 with the matching CLI. Saved snapshot formats do not
  change.
- Create private discovery registries for each user without administrator setup. These registries
  list available monitors. Reject unsafe existing permissions.
- Add `MonitorIo` so callers can supply I/O behavior, and add support for isolated monitor tests.
- Limit client handshake time, idle connection time, and resources used by concurrent clients.
- Add recorder event kinds for retained-notification waits, waits without an available permit, and
  signals.
- Preserve pointer provenance, the information about a pointer's origin, in recorder code. Simplify
  the handling of recorders after their threads stop.
- Update the requirement for local package `seismograph_protocol` from `0.2.0` to `0.3.0`.

## [0.3.0] - 2026-10-20

- Consolidate monitor startup errors at the crate root and migrate allocator diagnostics to the native-v4 model.

## [0.2.0] - 2026-10-05

- ⚠️ Breaking

  - Now requires `0.2.0` of `seismograph_protocol`

- ⚡ Performance

  - consolidate v1 and Seismograph improvements ([#764](https://github.com/microsoft/oxidizer/pull/764))
  - reduce runtime analysis duration ([#767](https://github.com/microsoft/oxidizer/pull/767))

- 🔄 Continuous Integration

  - complete cargo-anvil adoption ([#759](https://github.com/microsoft/oxidizer/pull/759))

