# Changelog

## [0.1.0]

- ✨ Features

  - introduce `fetch_azure`, adapting a `fetch::HttpClient` into an Azure SDK
    HTTP transport: `AzureHttpClient` implements `azure_core::http::HttpClient`
    on top of a `fetch::HttpClient`.
