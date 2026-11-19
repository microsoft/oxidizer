<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Fetch Tls Logo" width="96">

# Fetch Tls

[![crate.io](https://img.shields.io/crates/v/fetch_tls.svg)](https://crates.io/crates/fetch_tls)
[![docs.rs](https://docs.rs/fetch_tls/badge.svg)](https://docs.rs/fetch_tls)
[![MSRV](https://img.shields.io/crates/msrv/fetch_tls)](https://crates.io/crates/fetch_tls)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

TLS configuration for HTTP clients.

Applications use `fetch_tls` to describe their TLS requirements.
HTTP client libraries use those requirements to build a TLS backend.

## Applications

Use [`TlsOptions`][__link0] or [`TlsOptionsBuilder`][__link1] to configure TLS. You can let
the client library select a backend, select a backend, or wrap a backend
that you already built.

## HTTP client libraries

Use [`TlsBackendBuilder`][__link2] to convert [`TlsOptions`][__link3] into [`TlsBackend`][__link4].
The library provides environment-specific values, such as the rustls
crypto provider and default certificate verifier. It also selects the
backend when the application does not select one.

## Cargo features

* `rustls` enables the rustls backend. The application or client library
  must provide a crypto provider and server certificate verifier.
* `native-tls` enables the platform native TLS backend (`SChannel` on
  Windows, Security Framework on `macOS`, `OpenSSL` on Linux).

If neither feature is enabled, backend-specific constructors and builders
are unavailable. Building an automatic TLS backend returns [`BackendError`][__link5].


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch_tls">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbTPYJ-LqZC8kbySegBD6GrN8bDvFGDZLqN_Mbf1Cnh80FpOFhZIGCaWZldGNoX3Rsc2UwLjUuMQ
 [__link0]: https://docs.rs/fetch_tls/0.5.1/fetch_tls/?search=TlsOptions
 [__link1]: https://docs.rs/fetch_tls/0.5.1/fetch_tls/?search=TlsOptionsBuilder
 [__link2]: https://docs.rs/fetch_tls/0.5.1/fetch_tls/?search=TlsBackendBuilder
 [__link3]: https://docs.rs/fetch_tls/0.5.1/fetch_tls/?search=TlsOptions
 [__link4]: https://docs.rs/fetch_tls/0.5.1/fetch_tls/?search=TlsBackend
 [__link5]: https://docs.rs/fetch_tls/0.5.1/fetch_tls/?search=BackendError
