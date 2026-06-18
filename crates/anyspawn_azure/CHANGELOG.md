# Changelog

## [0.1.1] - 2026-06-18

- 🔧 Maintenance

  - Now requires `0.5.5` of `anyspawn`

## [0.1.0]

- ✨ Features

  - introduce `anyspawn_azure`, adapting Oxidizer primitives to Azure SDK
    runtime abstractions:
    - `Runtime` implements `azure_core::async_runtime::AsyncRuntime` on top
      of an `anyspawn::Spawner` (spawning) and a `tick::Clock` (sleeping).
    - with the optional `azure-identity` feature, `Runtime` also implements
      `azure_identity::Executor`, running credential commands on the
      `anyspawn::Spawner`.
