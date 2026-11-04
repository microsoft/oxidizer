# Changelog

## [0.4.0] - 2026-11-02

- Clarify the documentation for compression formats, streaming, flushing, and decompression limits.
  Compression behavior does not change.
- Update the Bytesbuf dependency and require the patched `zstd-safe` release.
- Update the requirement for local package `bytesbuf` from `0.12.0` to `0.13.0`.
- Update the requirement for local package `performables` from `0.3.0` to `0.3.1`.
- Update the requirement for local package `recoverable` from `0.3.0` to `0.3.1`.
- Update the requirement for local package `thread_aware` from `0.12.1` to `0.12.2`.
- Update the requirement for `zstd-safe` from `8.0.0` to `8.2.0`.
- Update the development requirement for `tokio` from `1.48.0` to `1.53.3`.
- Update the development requirement for `zstd-safe` from `8.0.0` to `8.2.0`.

## [0.3.0] - 2026-10-20

- Update exposed Bytesbuf identities for Bytesbuf 0.12; runtime codec behavior remains unchanged.
- Update local dependency requirements: bytesbuf ^0.11.0 -> 0.12.0; performables ^0.2.0 -> 0.3.0; recoverable ^0.2.0 -> 0.3.0; thread_aware ^0.12.0 -> 0.12.1.

## [0.2.0] - 2026-10-05

- ⚠️ Breaking

  - Now requires `0.11.0` of `bytesbuf`
  - Now requires `0.2.0` of `performables`

- ⚡ Performance

  - consolidate v1 and Seismograph improvements ([#764](https://github.com/microsoft/oxidizer/pull/764))

## [0.1.1] - 2026-09-16

- ✨ Features

  - add HTTP response decompression ([#746](https://github.com/microsoft/oxidizer/pull/746))

