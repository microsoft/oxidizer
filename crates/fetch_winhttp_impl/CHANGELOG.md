# Changelog

## [0.4.0] - 2026-10-20

- The public API exposes types from `bytesbuf`; upgrading to its incompatible release changes those public type identities.
- Update local dependency requirements: bytesbuf ^0.11.0 -> 0.12.0; bytesbuf_io ^0.11.0 -> 0.12.0; fetch ^0.18.0 -> 0.19.0; http_extensions ^0.12.0 -> 0.13.0; layered ^0.3.7 -> 0.3.8; observed ^0.28.0 -> 0.29.0; ohno ^0.5.2 -> 0.5.3; plurality ^0.2.2 -> 0.2.3; recoverable ^0.2.0 -> 0.3.0; thread_aware ^0.12.0 -> 0.12.1; tick ^0.7.1 -> 0.8.0.

## [0.3.0] - 2026-10-05

- ⚠️ Breaking

  - Now requires `0.11.0` of `bytesbuf`
  - Now requires `0.11.0` of `bytesbuf_io`
  - Now requires `0.18.0` of `fetch`
  - Now requires `0.12.0` of `http_extensions`
  - Now requires `0.27.0` of `observed`
  - Now requires `0.7.1` of `tick`

- 🐛 Bug Fixes

  - exclude inactive platform mutants ([#777](https://github.com/microsoft/oxidizer/pull/777))

- ⚡ Performance

  - reduce runtime analysis duration ([#767](https://github.com/microsoft/oxidizer/pull/767))

## [0.2.1] - 2026-09-16

- 🔧 Maintenance

  - Now requires `0.17.1` of `fetch`
  - Now requires `0.11.1` of `http_extensions`

## [0.2.0] - 2026-09-10

### Breaking

- adopt thread_aware_core across workspace ([#721](https://github.com/microsoft/oxidizer/pull/721))
- Now requires `0.10.0` of `bytesbuf`
- Now requires `0.10.0` of `bytesbuf_io`
- Now requires `0.17.0` of `fetch`
- Now requires `0.11.0` of `http_extensions`
- Now requires `0.3.7` of `layered`
- Now requires `0.26.0` of `observed`
- Now requires `0.5.2` of `ohno`
- Now requires `0.12.0` of `thread_aware`
- Now requires `0.7.0` of `tick`

### Bug Fixes

- correct enrichment and test-harness behavior ([#712](https://github.com/microsoft/oxidizer/pull/712))

## [0.1.1] - 2026-09-01

- ✨ Features

  - add a WinHTTP-backed fetch transport for Windows ([#687](https://github.com/microsoft/oxidizer/pull/687))
