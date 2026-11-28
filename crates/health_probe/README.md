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

Kubernetes readiness adapters for [`health`][__link0].

The TCP adapter listens on a dedicated port after it observes
[`health::Health::Healthy`][__link1].
It closes the port after it observes another state. The HTTP adapters read a
[`health::Monitor`][__link2] for each
request. They return `200 OK` with JSON `true` for a healthy process. They return
`503 Service Unavailable` with JSON `false` for any other state.

## Features

* `http-readiness-tower` enables the Tower readiness service.
* `http-readiness-request` enables the
  [`http_extensions`][__link3] request handler.
* `tcp` enables the TCP readiness server.

No features are enabled by default.

## Kubernetes configuration

Point a TCP probe at a dedicated readiness port:

```yaml
readinessProbe:
  tcpSocket:
    port: 8081
```

Point an HTTP probe at the route that contains an HTTP adapter:

```yaml
readinessProbe:
  httpGet:
    path: /ready
    port: 8080
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/health_probe">source code</a>.
</sub>

 [__link0]: https://docs.rs/process_health/latest/health/
 [__link1]: https://docs.rs/process_health/latest/health/?search=Health::Healthy
 [__link2]: https://docs.rs/process_health/latest/health/?search=Monitor
 [__link3]: https://docs.rs/http_extensions
