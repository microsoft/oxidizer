# Changelog

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

