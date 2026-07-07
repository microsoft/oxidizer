# Changelog

## [0.1.3] - 2026-07-07

- 🔧 Maintenance

  - Now requires `0.6.0` of `anyspawn`
  - Now requires `0.4.0` of `tick`

- ✨ Features

  - adapt fetch HttpClient to Azure's HttpClient ([#494](https://github.com/microsoft/oxidizer/pull/494))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - release a new version of tick crate (and dependents) ([#542](https://github.com/microsoft/oxidizer/pull/542))
  - re-release all packages with LFS-free tarballs ([#531](https://github.com/microsoft/oxidizer/pull/531))

## [0.1.2] - 2026-07-01

- 🔧 Maintenance

  - Now requires `0.3.6` of `tick`

- ✨ Features

  - adapt fetch HttpClient to Azure's HttpClient ([#494](https://github.com/microsoft/oxidizer/pull/494))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

- ✔️ Tasks

  - re-release all packages with LFS-free tarballs ([#531](https://github.com/microsoft/oxidizer/pull/531))

## [0.1.1] - 2026-06-26

- 🔧 Maintenance

  - Now requires `0.5.5` of `anyspawn`
  - Now requires `0.3.5` of `tick`

- ✨ Features

  - adapt fetch HttpClient to Azure's HttpClient ([#494](https://github.com/microsoft/oxidizer/pull/494))

- 🐛 Bug Fixes

  - exclude non-source artifacts from published crates via include allowlist ([#526](https://github.com/microsoft/oxidizer/pull/526))

## [0.1.0]

- ✨ Features

  - introduce `anyspawn_azure`, adapting Oxidizer primitives to Azure SDK
    runtime abstractions:
    - `Runtime` implements `azure_core::async_runtime::AsyncRuntime` on top
      of an `anyspawn::Spawner` (spawning) and a `tick::Clock` (sleeping).
    - with the optional `azure-identity` feature, `Runtime` also implements
      `azure_identity::Executor`, running credential commands on the
      `anyspawn::Spawner`.
