<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Health Probe Logo" width="96">

# Health Probe

[![crate.io](https://img.shields.io/crates/v/health_probe.svg)](https://crates.io/crates/health_probe)
[![docs.rs](https://docs.rs/health_probe/badge.svg)](https://docs.rs/health_probe)
[![MSRV](https://img.shields.io/crates/msrv/health_probe)](https://crates.io/crates/health_probe)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Kubernetes HTTP readiness adapters for [`health`][__link0].

Both adapters read a [`health::Monitor`][__link1] for each request. A healthy process returns
status `200 OK` with the JSON body `true`. A degraded or unhealthy process returns
`503 Service Unavailable` with the JSON body `false`. Responses contain no component details
and disable caching.

## Features

* `http` enables the Tower readiness service.
* `request-handler` enables the
  [`http_extensions`][__link2] request handler.

No features are enabled by default.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/health_probe">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbpXoqhIfze7YbZ5tYlnsi31obkA52rwh4jx4bi-pazXJ7D4FhZIGCZmhlYWx0aGUwLjEuMA
 [__link0]: https://crates.io/crates/health/0.1.0
 [__link1]: https://docs.rs/health/0.1.0/health/?search=Monitor
 [__link2]: https://docs.rs/http_extensions
