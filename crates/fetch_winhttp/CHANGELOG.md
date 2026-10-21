# Changelog

## [0.4.0] - 2026-10-20

- The public API exposes types from `bytesbuf`; upgrading to its incompatible release changes those public type identities.
- Update local dependency requirements: fetch_winhttp_impl =0.3.0 -> 0.4.0.

## [0.3.0] - 2026-10-05

- ⚠️ Breaking

  - Now requires `0.3.0` of `fetch_winhttp_impl`

- 🐛 Bug Fixes

  - exclude inactive platform mutants ([#777](https://github.com/microsoft/oxidizer/pull/777))

## [0.2.1] - 2026-09-16

- 🔧 Maintenance

  - Now requires `0.2.1` of `fetch_winhttp_impl`

## [0.2.0] - 2026-09-10

### Breaking

- adopt thread_aware_core across workspace ([#721](https://github.com/microsoft/oxidizer/pull/721))
- Now requires `0.2.0` of `fetch_winhttp_impl`

## [0.1.1] - 2026-09-01

- 🔧 Maintenance

  - Now requires `0.1.1` of `fetch_winhttp_impl`

- ✨ Features

  - add a WinHTTP-backed fetch transport for Windows ([#687](https://github.com/microsoft/oxidizer/pull/687))
