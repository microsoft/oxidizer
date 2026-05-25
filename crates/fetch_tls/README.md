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

`fetch_tls` separates *what* TLS behavior a consumer wants from *which*
TLS implementation actually provides it. This lets application code
describe its TLS requirements once and lets the HTTP client (or other
library) decide which backend to materialize at runtime.

## Overview

The core type is [`TlsOptions`][__link0], a backend-agnostic description of a TLS
client configuration. It can be constructed in a few ways:

* **Explicit backend selection** — use [`TlsOptions::new_rustls`][__link1] or
  [`TlsOptions::new_native_tls`][__link2] when the consumer specifically wants
  `rustls` or `native-tls`. For additional customization (client
  identity, custom verifier, supported HTTP versions, etc.), use the
  corresponding builders [`TlsOptions::builder_rustls`][__link3] /
  [`TlsOptions::builder_native_tls`][__link4] (which return a
  [`TlsOptionsBuilder`][__link5]).
* **Wrapping a pre-built configuration** — convert an existing
  [`rustls::ClientConfig`][__link6] or
  [`native_tls::TlsConnector`][__link7] into a
  [`TlsOptions`][__link8] via `From`/`Into`.
* **Default construction** — [`TlsOptions::default`][__link9] produces options
  that do not pin a specific backend; the choice is deferred to the
  library consuming the [`TlsOptions`][__link10].

## User vs. consumer perspective

From an **end-user / application** perspective, only [`TlsOptions`][__link11] and
[`TlsOptionsBuilder`][__link12] are relevant. Users describe their TLS requirements
and hand the resulting [`TlsOptions`][__link13] to a library (for example, an HTTP
client).

From a **library / client** perspective that adopts `fetch_tls`, the
[`TlsOptions`][__link14] is materialized into a concrete [`TlsBackend`][__link15] (backed by
a specific TLS implementation) via [`TlsOptions::build_backend`][__link16]. The
library supplies a [`TlsBackendDefaults`][__link17] that:

* provides the information required to actually build the backend (for
  example, a `rustls` crypto provider and server-certificate verifier via
  [`TlsBackendDefaults::configure_rustls`][__link18]), and
* decides which backend to use when the [`TlsOptions`][__link19] does not pin one
  (i.e. when the consumer does not care about the underlying TLS
  technology).

## Features

* **`rustls`** — pure-Rust [`rustls`][__link20]. `fetch_tls` does not
  bundle a crypto provider; the adopting library supplies one (along
  with a server-certificate verifier) via
  [`TlsBackendDefaults::configure_rustls`][__link21].
* **`native-tls`** — platform native TLS (`SChannel` on Windows,
  Security Framework on `macOS`, `OpenSSL` on Linux).

With neither feature enabled, [`TlsOptions::default`][__link22] still constructs
but [`TlsOptions::build_backend`][__link23] returns a [`BackendError`][__link24].


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch_tls">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG-IIjNGJ6OaRG__WcJFM0c4mG4L_zMzYrKYvG_1_yZWFbpC2YWSDgmlmZXRjaF90bHNlMC4xLjCCam5hdGl2ZV90bHNmMC4yLjE4gmZydXN0bHNnMC4yMy40MA
 [__link0]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions
 [__link1]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions::new_rustls
 [__link10]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions
 [__link11]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions
 [__link12]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptionsBuilder
 [__link13]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions
 [__link14]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions
 [__link15]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsBackend
 [__link16]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions::build_backend
 [__link17]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsBackendDefaults
 [__link18]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsBackendDefaults::configure_rustls
 [__link19]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions
 [__link2]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions::new_native_tls
 [__link20]: https://crates.io/crates/rustls/0.23.40
 [__link21]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsBackendDefaults::configure_rustls
 [__link22]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions::default
 [__link23]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions::build_backend
 [__link24]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=BackendError
 [__link3]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions::builder_rustls
 [__link4]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions::builder_native_tls
 [__link5]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptionsBuilder
 [__link6]: https://docs.rs/rustls/0.23.40/rustls/?search=ClientConfig
 [__link7]: https://docs.rs/native_tls/0.2.18/native_tls/?search=TlsConnector
 [__link8]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions
 [__link9]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions::default
