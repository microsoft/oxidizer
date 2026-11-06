<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Fetch Azure Logo" width="96">

# Fetch Azure

[![crate.io](https://img.shields.io/crates/v/fetch_azure.svg)](https://crates.io/crates/fetch_azure)
[![docs.rs](https://docs.rs/fetch_azure/badge.svg)](https://docs.rs/fetch_azure)
[![MSRV](https://img.shields.io/crates/msrv/fetch_azure)](https://crates.io/crates/fetch_azure)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Use a [`fetch::HttpClient`][__link0] as an Azure SDK HTTP transport.

[`HttpClient`][__link1] implements [`azure_core::http::HttpClient`][__link2] and sends requests
through a [`fetch::HttpClient`][__link3]. Azure SDK clients can use this adapter for
their HTTP requests.

The `anyspawn_azure` crate lets Azure SDK clients run on an
`anyspawn`-based async runtime.

## Example

```rust
use azure_core::http::{ClientOptions, Transport};
use fetch_azure::HttpClient;

// Use a `fetch` client as the Azure SDK transport.
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

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQb2XybXibdNpYbrG-mgnpA7W8bLT5ta1-RWA4boCOuVO_mETlhZIOCamF6dXJlX2NvcmVlMS4xLjCCZWZldGNoZjAuMjEuMIJrZmV0Y2hfYXp1cmVmMC4xMS4w
 [__link0]: https://docs.rs/fetch/0.21.0/fetch/?search=HttpClient
 [__link1]: https://docs.rs/fetch_azure/0.11.0/fetch_azure/?search=HttpClient
 [__link2]: https://docs.rs/azure_core/1.1.0/azure_core/?search=http::HttpClient
 [__link3]: https://docs.rs/fetch/0.21.0/fetch/?search=HttpClient
