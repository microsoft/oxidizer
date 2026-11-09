# Changelog

## [0.5.0] - 2026-11-06

- Update published local dependency requirements. No direct source change.
- Breaking: Public APIs expose types from packages with incompatible releases:
  `bytesbuf`, `compressors`, `http_extensions`, `seismograph`, `seismograph_io`.
- Update the local `bytesbuf` requirement from `0.13.0` to `0.14.0`.
- Update the local `compressors` requirement from `0.4.0` to `0.5.0`.
- Update the local `http_extensions` requirement from `0.14.0` to `0.15.0`.
- Update the local `seatbelt` requirement from `0.12.0` to `0.12.1`.

## [0.4.0] - 2026-11-02

- Update compression middleware dependencies to use Bytesbuf, Compressors, HTTP Extensions, and
  Seatbelt. Compression behavior does not change.
- Update the requirement for local package `bytesbuf` from `0.12.0` to `0.13.0`.
- Update the requirement for local package `compressors` from `0.3.0` to `0.4.0`.
- Update the requirement for local package `http_extensions` from `0.13.0` to `0.14.0`.
- Update the requirement for local package `layered` from `0.3.8` to `0.3.9`.
- Update the requirement for local package `ohno` from `0.5.3` to `0.5.4`.
- Update the requirement for local package `seatbelt` from `0.11.0` to `0.12.0`.
- Update the development requirement for `tokio` from `1.48.0` to `1.53.3`.

## [0.3.0] - 2026-10-20

- Bound decompressed HTTP bodies to 64 MiB and 1,024 streams by default, with explicit finite overrides and unbounded opt-outs.
- Update local dependency requirements: bytesbuf ^0.11.0 -> 0.12.0; compressors ^0.2.0 -> 0.3.0; http_extensions ^0.12.0 -> 0.13.0; layered ^0.3.7 -> 0.3.8; ohno ^0.5.2 -> 0.5.3; seatbelt ^0.10.0 -> 0.11.0.

## [0.2.0] - 2026-10-05

- ⚠️ Breaking

  - Now requires `0.11.0` of `bytesbuf`
  - Now requires `0.2.0` of `compressors`
  - Now requires `0.12.0` of `http_extensions`
  - Now requires `0.10.0` of `seatbelt`

## [0.1.1] - 2026-09-16

- 🔧 Maintenance

  - Now requires `0.1.1` of `compressors`
  - Now requires `0.11.1` of `http_extensions`

- ✨ Features

  - add HTTP response decompression ([#746](https://github.com/microsoft/oxidizer/pull/746))

