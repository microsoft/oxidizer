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

Adapt a [`fetch::HttpClient`][__link0] into an Azure SDK HTTP transport.

The Azure SDK abstracts its HTTP transport behind the
[`azure_core::http::HttpClient`][__link1] trait. [`AzureHttpClient`][__link2] implements that
trait on top of a [`fetch::HttpClient`][__link3], so Azure SDK pipelines run over
`fetch` and benefit from its resilience and observability.

To run the Azure SDK on an [`anyspawn`][__link4]-backed async runtime, see the
`anyspawn_azure` crate.

## Example

```rust
use std::sync::Arc;

use azure_core::http::HttpClient;
use fetch::HttpClient as FetchClient;
use fetch_azure::AzureHttpClient;

// Adapt a `fetch` client into an Azure SDK transport.
fn transport(client: FetchClient) -> Arc<dyn HttpClient> {
    AzureHttpClient::from(client).into()
}
```


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch_azure">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQbIDHZEzF7TqQbEgXgpwz2qz4bFYd2Uq2wVpQbG0lvoB-LAzVhZISCaGFueXNwYXduZTAuNS4zgmphenVyZV9jb3JlZTEuMC4wgmVmZXRjaGYwLjExLjCCa2ZldGNoX2F6dXJlZTAuMS4w
 [__link0]: https://docs.rs/fetch/0.11.0/fetch/?search=HttpClient
 [__link1]: https://docs.rs/azure_core/1.0.0/azure_core/?search=http::HttpClient
 [__link2]: https://docs.rs/fetch_azure/0.1.0/fetch_azure/?search=AzureHttpClient
 [__link3]: https://docs.rs/fetch/0.11.0/fetch/?search=HttpClient
 [__link4]: https://crates.io/crates/anyspawn/0.5.3
