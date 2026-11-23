<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Oxidizer Health Logo" width="96">

# Oxidizer Health

[![crate.io](https://img.shields.io/crates/v/oxidizer_health.svg)](https://crates.io/crates/oxidizer_health)
[![docs.rs](https://docs.rs/oxidizer_health/badge.svg)](https://docs.rs/oxidizer_health)
[![MSRV](https://img.shields.io/crates/msrv/oxidizer_health)](https://crates.io/crates/oxidizer_health)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Component health reports and HTTP readiness checks for Rust services.

A service reports the worst state among its components: `Healthy`, `Degraded`, or `Unhealthy`.
For example, if a database is degraded and a cache is healthy, the service is degraded.
Use the overall state for readiness checks and [component reports][__link0] to find the reason.

## Quick start

Register a component for the database. Use its [`Publisher`][__link1] to report slow queries:

```rust
use oxidizer_health::{Aggregator, Health};

let health = Aggregator::new();
let database = health.component("database");
let mut publisher = database.publisher();

publisher.degraded("slow queries");
assert_eq!(publisher.signal().state(), Health::Degraded);
```

Publishing updates the local signal immediately. [`Aggregator::state`][__link2] reads the latest
aggregate, so it can briefly report the previous state. A component without publishers is
`Unhealthy`, as is a service without components.

## HTTP readiness

Health reporting works without HTTP. Enable `http` to use `http::service` with Tower,
or `request-handler` to use `http::request_handler` with `http_extensions`.
Both return HTTP 200 with JSON `true` for `Healthy` and HTTP 503 with JSON `false` otherwise.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/oxidizer_health">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQb7K8rHXuppx4bs238ldC7gsQbXYHdH_f8_EIbK4lBPHKMputhZIGCb294aWRpemVyX2hlYWx0aGUwLjEuMA
 [__link0]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Report
 [__link1]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Publisher
 [__link2]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator::state
