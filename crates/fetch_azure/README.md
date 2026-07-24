<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Fetch Azure Logo" width="96">

# Fetch Azure

[![crate.io](https://img.shields.io/crates/v/fetch_azure.svg)](https://crates.io/crates/fetch_azure)
[![docs.rs](https://docs.rs/fetch_azure/badge.svg)](https://docs.rs/fetch_azure)
[![MSRV](https://img.shields.io/crates/msrv/fetch_azure)](https://crates.io/crates/fetch_azure)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Adapt a [`fetch::HttpClient`][__link0] into an Azure SDK HTTP transport.

The Azure SDK abstracts its HTTP transport behind the
[`azure_core::http::HttpClient`][__link1] trait. [`HttpClient`][__link2] implements it on top
of a `fetch` client, so Azure SDK pipelines run over `fetch` and benefit
from its resilience and observability.

To run the Azure SDK on an `anyspawn`-backed async runtime, see the
`anyspawn_azure` crate.

## Example

```rust
use azure_core::http::{ClientOptions, Transport};
use fetch_azure::HttpClient;

// Wire a `fetch` client in as the transport for an Azure SDK client.
let transport = Transport::new(HttpClient::from(client).into());
let options = ClientOptions {
    transport: Some(transport),
    ..Default::default()
};
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch_azure">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbbmImiFehOUcbSWj7AJ0Zo0QbEU03KDZjzxUbIR9apRz2JO5hZIOCamF6dXJlX2NvcmVlMS4wLjCCZWZldGNoZjAuMTQuMIJrZmV0Y2hfYXp1cmVlMC40LjA
 [__link0]: https://docs.rs/fetch/0.14.0/fetch/?search=HttpClient
 [__link1]: https://docs.rs/azure_core/1.0.0/azure_core/?search=http::HttpClient
 [__link2]: https://docs.rs/fetch_azure/0.4.0/fetch_azure/?search=HttpClient
