# Changelog

## [0.1.0]

- ✨ Features

  - introduce `fetch_azure`, bundling two Azure SDK abstractions backed by the
    Oxidizer stack:
    - `FetchHttpClient` implements `azure_core::http::HttpClient` on top of a
      `fetch::HttpClient` transport.
    - `SpawnerRuntime` implements `azure_core::async_runtime::AsyncRuntime` on top
      of an `anyspawn::Spawner` (spawning) and a `tick::Clock` (sleeping).
