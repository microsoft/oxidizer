<div align="center">
 <img src="./logo.png" alt="Fetch Tls Logo" width="96">

# Fetch Tls

[![crate.io](https://img.shields.io/crates/v/fetch_tls.svg)](https://crates.io/crates/fetch_tls)
[![docs.rs](https://docs.rs/fetch_tls/badge.svg)](https://docs.rs/fetch_tls)
[![MSRV](https://img.shields.io/crates/msrv/fetch_tls)](https://crates.io/crates/fetch_tls)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Backend-agnostic TLS configuration for HTTP clients.

`fetch_tls` separates *what* TLS behavior an application wants from
*which* TLS implementation actually provides it. Applications describe
their TLS requirements once, and the HTTP client (or other consuming
library) decides which backend to materialize at runtime.

## Two perspectives

Applications work with [`TlsOptions`][__link0] (and its builder,
[`TlsOptionsBuilder`][__link1]) to describe what they want: pick a specific
backend, wrap an already-built backend, or leave the choice to the
consuming library.

Libraries that adopt `fetch_tls` use [`TlsBackendBuilder`][__link2] to turn a
[`TlsOptions`][__link3] into a ready-to-use [`TlsBackend`][__link4]. The library
contributes the environment-specific pieces (such as the rustls crypto
provider and default certificate verifier) and decides which backend to
use when the application did not pin one.

## Cargo features

* `rustls` — enables the rustls backend. `fetch_tls` does not bundle a
  crypto provider; the consuming library supplies one along with a
  default server certificate verifier.
* `native-tls` — enables the platform native TLS backend (`SChannel` on
  Windows, Security Framework on `macOS`, `OpenSSL` on Linux).

With neither feature enabled, the API surface is limited to wrapping a
pre-built backend; attempting to build any other configuration returns
a [`BackendError`][__link5].


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch_tls">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG_c_MUhcML6qG2ZZBxJ9qD3qG1HAxyflQ3KlG_zX8IvGvZo9YWSBgmlmZXRjaF90bHNlMC4yLjA
 [__link0]: https://docs.rs/fetch_tls/0.2.0/fetch_tls/?search=TlsOptions
 [__link1]: https://docs.rs/fetch_tls/0.2.0/fetch_tls/?search=TlsOptionsBuilder
 [__link2]: https://docs.rs/fetch_tls/0.2.0/fetch_tls/?search=TlsBackendBuilder
 [__link3]: https://docs.rs/fetch_tls/0.2.0/fetch_tls/?search=TlsOptions
 [__link4]: https://docs.rs/fetch_tls/0.2.0/fetch_tls/?search=TlsBackend
 [__link5]: https://docs.rs/fetch_tls/0.2.0/fetch_tls/?search=BackendError
