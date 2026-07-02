# Changelog

## [0.2.2] - 2026-07-01

- 🔧 Maintenance

  - Now requires `0.3.6` of `tick`

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - re-release all packages with LFS-free tarballs ([#531](https://github.com/microsoft/oxidizer/pull/531))
  - release ohno 0.3.7 and cascade dependents ([#524](https://github.com/microsoft/oxidizer/pull/524))

## [0.2.1] - 2026-06-26

- 🔧 Maintenance

  - Now requires `0.5.6` of `bytesbuf`
  - Now requires `0.12.1` of `fetch`
  - Now requires `0.3.5` of `layered`

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - release ohno 0.3.7 and cascade dependents ([#524](https://github.com/microsoft/oxidizer/pull/524))

## [0.2.0] - 2026-06-24

- ⚠️ Breaking

  - Now requires `0.12.0` of `fetch`
  - Now requires `0.3.7` of `ohno`
  - Now requires `0.5.8` of `seatbelt`

## [0.1.1] - 2026-06-18

- 🔧 Maintenance

  - Now requires `0.5.5` of `bytesbuf`
  - Now requires `0.11.2` of `fetch`

## [0.1.0]

- ✨ Features

  - introduce `fetch_azure`, adapting a `fetch::HttpClient` into an Azure SDK
    HTTP transport: `HttpClient` implements `azure_core::http::HttpClient`
    on top of a `fetch::HttpClient`.
