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

Build a [`TlsOptions`][__link0] with [`TlsOptions::builder_rustls`][__link1] or
[`TlsOptions::builder_native_tls`][__link2], or wrap a pre-built
[`rustls::ClientConfig`][__link3] /
[`native_tls::TlsConnector`][__link4] via `From`/`Into`.
Materialize a [`TlsBackend`][__link5] with [`TlsOptions::build_backend`][__link6].

## Features

* **`rustls`** — pure-Rust [`rustls`][__link7]. `fetch_tls` does not
  bundle a crypto provider; the caller supplies one (along with a
  server-certificate verifier) via
  [`TlsBackendDefaults::configure_rustls`][__link8].
* **`native-tls`** — platform native TLS (`SChannel` on Windows,
  Security Framework on `macOS`, `OpenSSL` on Linux).

With neither feature enabled, [`TlsOptions::default`][__link9] still constructs but
[`TlsOptions::build_backend`][__link10] returns a [`BackendError`][__link11].


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch_tls">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG2t4QyKGZLjOGzG3afJHTM1YG1p0yPzfBpVtG_wKLWa2KlkuYWSDgmlmZXRjaF90bHNlMC4xLjCCam5hdGl2ZV90bHNmMC4yLjE4gmZydXN0bHNnMC4yMy40MA
 [__link0]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions
 [__link1]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions::builder_rustls
 [__link10]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions::build_backend
 [__link11]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=BackendError
 [__link2]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions::builder_native_tls
 [__link3]: https://docs.rs/rustls/0.23.40/rustls/?search=ClientConfig
 [__link4]: https://docs.rs/native_tls/0.2.18/native_tls/?search=TlsConnector
 [__link5]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsBackend
 [__link6]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions::build_backend
 [__link7]: https://crates.io/crates/rustls/0.23.40
 [__link8]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsBackendDefaults::configure_rustls
 [__link9]: https://docs.rs/fetch_tls/0.1.0/fetch_tls/?search=TlsOptions::default
