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

This crate is an internal implementation detail of the `SDK`. It is not part
of the public API surface, must not be re-exported, and offers no stability
guarantees: anything may change in any release, including patch releases.

Scope is narrow: just the transport that issues HTTP/1.1 or HTTP/2 requests
over `TLS` (or plain-text). No higher-level pipeline, retry, or caching.

The entry points are:

* [`HyperTransportBuilder`][__link0]: builds a transport from a user-supplied
  [`Connect`][__link1] service and a [`fetch_options::TransportOptions`][__link2].
* [`HyperTransport`][__link3]: the type-erased [`RequestHandler`][__link4] produced by
  [`HyperTransportBuilder::build`][__link5].

The runtime is supplied by the caller via an [`anyspawn::Spawner`][__link6].


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch_hyper">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQb1PafKPRqQnMbycEo89Tdc1Ibs7VR1QL49eUbNVExL_GkLMNhZISCaGFueXNwYXduZTAuNS40gmtmZXRjaF9oeXBlcmUwLjQuMoJtZmV0Y2hfb3B0aW9uc2UwLjIuMoJvaHR0cF9leHRlbnNpb25zZTAuNi4y
 [__link0]: https://docs.rs/fetch_hyper/0.4.2/fetch_hyper/?search=HyperTransportBuilder
 [__link1]: https://docs.rs/fetch_hyper/0.4.2/fetch_hyper/?search=Connect
 [__link2]: https://docs.rs/fetch_options/0.2.2/fetch_options/?search=TransportOptions
 [__link3]: https://docs.rs/fetch_hyper/0.4.2/fetch_hyper/?search=HyperTransport
 [__link4]: https://docs.rs/http_extensions/0.6.2/http_extensions/?search=RequestHandler
 [__link5]: https://docs.rs/fetch_hyper/0.4.2/fetch_hyper/?search=HyperTransportBuilder::build
 [__link6]: https://docs.rs/anyspawn/0.5.4/anyspawn/?search=Spawner
