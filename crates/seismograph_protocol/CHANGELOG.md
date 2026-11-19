# Changelog

## [0.3.0] - 2026-11-02

- Breaking: Use protocol version 8 over Unix sockets or local Windows named pipes instead of
  TCP/token transport. Operating-system permissions control access.
- Breaking: Update monitor descriptors and constructors for discovery and transport. Descriptors
  identify a process without tokens, and constructors require explicit `MonitorIo` values.
- Create and discover private registries for each user. These registries list monitors. Reject
  unsafe permissions and stale or invalid descriptors.
- Add `test-util` mock I/O with controlled clocks, partial transfers, and injected failures.
- Add isolated resources for tests that use native IPC (communication between processes).
- Add `read_control_response` to enforce a 64 KiB payload limit before allocating memory. Snapshot
  responses retain a separate larger limit.

## [0.2.0] - 2026-10-05

- 🐛 Bug Fixes

  - update rustls for RUSTSEC-2026-0285 ([#761](https://github.com/microsoft/oxidizer/pull/761))

- ⚡ Performance

  - consolidate v1 and Seismograph improvements ([#764](https://github.com/microsoft/oxidizer/pull/764))

- 🔄 Continuous Integration

  - complete cargo-anvil adoption ([#759](https://github.com/microsoft/oxidizer/pull/759))

