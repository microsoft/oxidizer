# Changelog

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
