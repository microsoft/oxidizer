# Changelog

## [0.13.0] - 2026-07-07

- ⚠️ Breaking

  - Now requires `0.6.0` of `anyspawn`
  - Now requires `0.6.0` of `bytesbuf`
  - Now requires `0.4.6` of `fetch_hyper`
  - Now requires `0.7.0` of `http_extensions`
  - Now requires `0.6.0` of `seatbelt`
  - Now requires `0.4.6` of `seatbelt_http`
  - Now requires `0.8.0` of `thread_aware`
  - Now requires `0.4.0` of `tick`

- ✨ Features

  - report fetch.runtime and fetch.transport telemetry attributes ([#510](https://github.com/microsoft/oxidizer/pull/510))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ⚡ Performance

  - avoid allocations in client_scope for borrowed-'static names ([#514](https://github.com/microsoft/oxidizer/pull/514))

- ✔️ Tasks

  - release a new version of tick crate (and dependents) ([#542](https://github.com/microsoft/oxidizer/pull/542))
  - upgrade alloc_tracker from 0.5.25 to 0.6.0 ([#513](https://github.com/microsoft/oxidizer/pull/513))
  - re-release all packages with LFS-free tarballs ([#531](https://github.com/microsoft/oxidizer/pull/531))
  - release ohno 0.3.7 and cascade dependents ([#524](https://github.com/microsoft/oxidizer/pull/524))

- 🔄 Continuous Integration

  - run cargo udeps with and without --all-targets; remove unused dev-dependencies ([#527](https://github.com/microsoft/oxidizer/pull/527))

## [0.12.2] - 2026-07-01

- 🔧 Maintenance

  - Now requires `0.3.6` of `tick`

- ✨ Features

  - report fetch.runtime and fetch.transport telemetry attributes ([#510](https://github.com/microsoft/oxidizer/pull/510))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ⚡ Performance

  - avoid allocations in client_scope for borrowed-'static names ([#514](https://github.com/microsoft/oxidizer/pull/514))

- ✔️ Tasks

  - upgrade alloc_tracker from 0.5.25 to 0.6.0 ([#513](https://github.com/microsoft/oxidizer/pull/513))
  - re-release all packages with LFS-free tarballs ([#531](https://github.com/microsoft/oxidizer/pull/531))
  - release ohno 0.3.7 and cascade dependents ([#524](https://github.com/microsoft/oxidizer/pull/524))

- 🔄 Continuous Integration

  - run cargo udeps with and without --all-targets; remove unused dev-dependencies ([#527](https://github.com/microsoft/oxidizer/pull/527))

## [0.12.1] - 2026-06-26

- 🔧 Maintenance

  - Now requires `0.5.5` of `anyspawn`
  - Now requires `0.5.6` of `bytesbuf`
  - Now requires `0.12.3` of `data_privacy`
  - Now requires `0.4.4` of `fetch_hyper`
  - Now requires `0.2.3` of `fetch_options`
  - Now requires `0.2.5` of `fetch_tls`
  - Now requires `0.3.4` of `fundle`
  - Now requires `0.6.4` of `http_extensions`
  - Now requires `0.3.5` of `layered`
  - Now requires `0.3.8` of `ohno`
  - Now requires `0.5.9` of `seatbelt`
  - Now requires `0.4.4` of `seatbelt_http`
  - Now requires `0.3.4` of `templated_uri`
  - Now requires `0.7.5` of `thread_aware`
  - Now requires `0.3.5` of `tick`

- ✨ Features

  - report fetch.runtime and fetch.transport telemetry attributes ([#510](https://github.com/microsoft/oxidizer/pull/510))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ⚡ Performance

  - avoid allocations in client_scope for borrowed-'static names ([#514](https://github.com/microsoft/oxidizer/pull/514))

- ✔️ Tasks

  - release ohno 0.3.7 and cascade dependents ([#524](https://github.com/microsoft/oxidizer/pull/524))

## [0.12.0] - 2026-06-24

- 🔧 Maintenance

  - Now requires `0.3.7` of `ohno`
  - Now requires `0.5.8` of `seatbelt`

- ✨ Features

  - report fetch.runtime and fetch.transport telemetry attributes ([#510](https://github.com/microsoft/oxidizer/pull/510))

## [0.11.2] - 2026-06-18

- 🔧 Maintenance

  - Now requires `0.5.5` of `bytesbuf`

## [0.11.1] - 2026-06-11

- 🔧 Maintenance

  - Now requires `0.5.4` of `anyspawn`
  - Now requires `0.5.4` of `bytesbuf`
  - Now requires `0.12.2` of `data_privacy`
  - Now requires `0.1.1` of `data_privacy_core`
  - Now requires `0.10.2` of `data_privacy_macros`
  - Now requires `0.10.2` of `data_privacy_macros_impl`
  - Now requires `0.4.1` of `fetch_hyper`
  - Now requires `0.2.2` of `fetch_options`
  - Now requires `0.2.3` of `fetch_tls`
  - Now requires `0.3.3` of `fundle`
  - Now requires `0.3.3` of `fundle_macros`
  - Now requires `0.3.3` of `fundle_macros_impl`
  - Now requires `0.6.1` of `http_extensions`
  - Now requires `0.3.4` of `layered`
  - Now requires `0.3.6` of `ohno`
  - Now requires `0.3.4` of `ohno_macros`
  - Now requires `0.1.6` of `recoverable`
  - Now requires `0.5.7` of `seatbelt`
  - Now requires `0.4.1` of `seatbelt_http`
  - Now requires `0.3.2` of `templated_uri`
  - Now requires `0.2.4` of `templated_uri_macros`
  - Now requires `0.2.4` of `templated_uri_macros_impl`
  - Now requires `0.7.4` of `thread_aware`
  - Now requires `0.7.4` of `thread_aware_macros`
  - Now requires `0.7.3` of `thread_aware_macros_impl`
  - Now requires `0.3.4` of `tick`

## [0.11.0] - 2026-06-10

- ⚠️ Breaking

  - improve working with response bodies ([#485](https://github.com/microsoft/oxidizer/pull/485))

## [0.10.2] - 2026-06-05

- 🔧 Maintenance

  - bump `fetch_options` to 0.2.1

## [0.10.1] - 2026-06-05

- 🔧 Maintenance

  - bump `seatbelt` to 0.5.5 (transitively updates `recoverable` to 0.1.5)

## [0.10.0] - 2026-06-04

- ✨ Features

  - introduce fetch crate
