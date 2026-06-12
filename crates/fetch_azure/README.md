<div align="center">
 <img src="./logo.png" alt="Fetch Azure Logo" width="96">

# Fetch Azure

[![crate.io](https://img.shields.io/crates/v/fetch_azure.svg)](https://crates.io/crates/fetch_azure)
[![docs.rs](https://docs.rs/fetch_azure/badge.svg)](https://docs.rs/fetch_azure)
[![MSRV](https://img.shields.io/crates/msrv/fetch_azure)](https://crates.io/crates/fetch_azure)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Bundle [`fetch`][__link0] and [`anyspawn`][__link1] as Azure SDK abstractions.

The Azure SDK abstracts its HTTP transport behind the
[`azure_core::http::HttpClient`][__link2] trait and its task spawning, sleeping, and
yielding behind the [`azure_core::async_runtime::AsyncRuntime`][__link3] trait. This
crate provides adapters for both:

* [`FetchHttpClient`][__link4] implements [`HttpClient`][__link5] on top of a
  [`fetch::HttpClient`][__link6], so Azure SDK pipelines run over `fetch` and benefit
  from its resilience and observability.
* [`SpawnerRuntime`][__link7] implements [`AsyncRuntime`][__link8] on top of an
  [`anyspawn::Spawner`][__link9], so the Azure SDK spawns and sleeps on the runtime of
  your choice.

## Example

```rust
use std::sync::Arc;

use anyspawn::Spawner;
use azure_core::async_runtime::{AsyncRuntime, set_async_runtime};
use azure_core::http::HttpClient;
use fetch::HttpClient as FetchClient;
use fetch_azure::{new_async_runtime, new_http_client};
use tick::Clock;

// Adapt a `fetch` client into an Azure SDK transport.
fn transport(client: FetchClient) -> Arc<dyn HttpClient> {
    new_http_client(client)
}

// Install an `anyspawn`-backed async runtime (sleeping on a `tick::Clock`).
fn install_runtime(spawner: Spawner, clock: Clock) {
    let runtime: Arc<dyn AsyncRuntime> = new_async_runtime(spawner, clock);
    let _ = set_async_runtime(runtime);
}
```


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch_azure">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQbKO4oAJVSTNsbaMx08xFfCTQbKvuvqbtNTBYb1G5JrfJCnSBhZISCaGFueXNwYXduZTAuNS4zgmphenVyZV9jb3JlZTEuMC4wgmVmZXRjaGYwLjExLjCCa2ZldGNoX2F6dXJlZTAuMS4w
 [__link0]: https://crates.io/crates/fetch/0.11.0
 [__link1]: https://crates.io/crates/anyspawn/0.5.3
 [__link2]: https://docs.rs/azure_core/1.0.0/azure_core/?search=http::HttpClient
 [__link3]: https://docs.rs/azure_core/1.0.0/azure_core/?search=async_runtime::AsyncRuntime
 [__link4]: https://docs.rs/fetch_azure/0.1.0/fetch_azure/struct.FetchHttpClient.html
 [__link5]: https://docs.rs/azure_core/1.0.0/azure_core/?search=http::HttpClient
 [__link6]: https://docs.rs/fetch/0.11.0/fetch/?search=HttpClient
 [__link7]: https://docs.rs/fetch_azure/0.1.0/fetch_azure/struct.SpawnerRuntime.html
 [__link8]: https://docs.rs/azure_core/1.0.0/azure_core/?search=async_runtime::AsyncRuntime
 [__link9]: https://docs.rs/anyspawn/0.5.3/anyspawn/?search=Spawner
