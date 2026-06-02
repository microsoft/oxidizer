<div align="center">
 <img src="./logo.png" alt="Fetch Hyper Logo" width="96">

# Fetch Hyper

[![crate.io](https://img.shields.io/crates/v/fetch_hyper.svg)](https://crates.io/crates/fetch_hyper)
[![docs.rs](https://docs.rs/fetch_hyper/badge.svg)](https://docs.rs/fetch_hyper)
[![MSRV](https://img.shields.io/crates/msrv/fetch_hyper)](https://crates.io/crates/fetch_hyper)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Hyper-based HTTP transport.

## Internal implementation detail

This crate is an **internal implementation detail** of the `SDK`. It is not
part of the public API surface and must not be re-exported from any other
crate. Consumers should depend on the higher-level `SDK` crates instead of
taking a direct dependency on `fetch_hyper`, and `SDK` crates that do depend
on it must keep its types out of their own public APIs (no `pub use`,
no types appearing in public function signatures, trait bounds, or
associated types).

No stability guarantees are offered: items may be added, renamed, removed,
or have their semantics changed in any release — including patch releases —
without notice.

Narrow scope: just the transport that issues HTTP/1.1 or HTTP/2 requests
over `TLS` (or plain-text). No higher-level pipeline, retry, caching, etc.

The entry points are:

* [`HyperTransportBuilder`][__link0]: generic over a user-supplied [`Connect`][__link1]
  service. Exposes setters for the few knobs driving our own logic plus a
  [`configure_hyper`][__link2] escape
  hatch for `hyper`’s own builder.
* [`HyperTransport`][__link3]: the type-erased [`RequestHandler`][__link4] produced
  by [`HyperTransportBuilder::build`][__link5].

The runtime is supplied entirely by the caller via an
[`anyspawn::Spawner`][__link6] together with any service implementing [`Connect`][__link7].


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch_hyper">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG27JhwJzXkumG9Cgswtvbtk3G11RQ0eI0kW1G87aELyEYbR5YWSDgmhhbnlzcGF3bmUwLjUuMoJrZmV0Y2hfaHlwZXJlMC4yLjCCb2h0dHBfZXh0ZW5zaW9uc2UwLjQuMw
 [__link0]: https://docs.rs/fetch_hyper/0.2.0/fetch_hyper/?search=HyperTransportBuilder
 [__link1]: https://docs.rs/fetch_hyper/0.2.0/fetch_hyper/?search=Connect
 [__link2]: https://docs.rs/fetch_hyper/0.2.0/fetch_hyper/?search=HyperTransportBuilder::configure_hyper
 [__link3]: https://docs.rs/fetch_hyper/0.2.0/fetch_hyper/?search=HyperTransport
 [__link4]: https://docs.rs/http_extensions/0.4.3/http_extensions/?search=RequestHandler
 [__link5]: https://docs.rs/fetch_hyper/0.2.0/fetch_hyper/?search=HyperTransportBuilder::build
 [__link6]: https://docs.rs/anyspawn/0.5.2/anyspawn/?search=Spawner
 [__link7]: https://docs.rs/fetch_hyper/0.2.0/fetch_hyper/?search=Connect
